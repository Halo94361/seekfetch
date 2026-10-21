//! Local usage log: one JSON line per API call, for cost awareness.
//! Append-only; recording and reading problems never fail a request.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const KIND_SEARCH: &str = "search";
pub const KIND_FETCH: &str = "fetch";
pub const KIND_BALANCE: &str = "balance";

/// CNY per 1000 successful non-cached search/fetch calls. Failed calls and
/// cache hits are free; balance queries are not billed.
const COST_PER_1000_CNY: f64 = 5.0;

#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
    pub ts: u64,
    pub kind: String,
    pub ok: bool,
    #[serde(default)]
    pub cached: bool,
}

pub fn path() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("cannot locate the platform data directory")?
        .join("seekfetch")
        .join("usage.jsonl"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Record one call. Silent on any failure; honors `[usage] enabled = false`.
pub fn record(config: &Config, kind: &str, ok: bool, cached: bool) {
    if !config.usage.enabled {
        return;
    }
    let Ok(path) = path() else {
        return;
    };
    let event = Event {
        ts: now_secs(),
        kind: kind.to_string(),
        ok,
        cached,
    };
    let _ = append(&path, &event);
}

fn append(path: &Path, event: &Event) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Inclusive lower / exclusive upper bound on event timestamps.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeFilter {
    pub since: Option<u64>,
    pub until: Option<u64>,
}

impl TimeFilter {
    /// Build a filter from CLI arguments. Dates are UTC `YYYY-MM-DD`;
    /// `--until` includes the whole day. `--last` uses fixed-length units.
    pub fn from_args(since: Option<&str>, until: Option<&str>, last: Option<&str>) -> Result<Self> {
        if last.is_some() && (since.is_some() || until.is_some()) {
            bail!("--last cannot be combined with --since or --until");
        }

        let mut filter = Self::default();
        if let Some(value) = last {
            let seconds = parse_period(value)?;
            filter.since = Some(now_secs().saturating_sub(seconds));
        }
        if let Some(value) = since {
            filter.since = Some(parse_date(value)?);
        }
        if let Some(value) = until {
            let start = parse_date(value)?;
            filter.until = Some(
                start
                    .checked_add(86_400)
                    .context("--until date is out of range")?,
            );
        }
        if let (Some(since), Some(until)) = (filter.since, filter.until) {
            if since >= until {
                bail!("--since must be earlier than --until");
            }
        }
        Ok(filter)
    }

    fn includes(&self, ts: u64) -> bool {
        self.since.map_or(true, |since| ts >= since) && self.until.map_or(true, |until| ts < until)
    }
}

/// How `usage --group-by` buckets events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    Day,
    Week,
    Month,
}

impl GroupBy {
    fn bucket_start(self, ts: u64) -> u64 {
        match self {
            Self::Day => ts - ts % 86_400,
            Self::Week => {
                let days = ts / 86_400;
                let weekday = (days + 3) % 7; // Monday = 0; 1970-01-01 was Thursday
                days.saturating_sub(weekday) * 86_400
            }
            Self::Month => {
                let (year, month) = year_month(ts);
                days_from_civil(year, month, 1) as u64 * 86_400
            }
        }
    }

    fn bucket_end(self, start: u64) -> u64 {
        match self {
            Self::Day => start + 86_400,
            Self::Week => start + 7 * 86_400,
            Self::Month => {
                let (year, month) = year_month(start);
                let (year, month) = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                days_from_civil(year, month, 1) as u64 * 86_400
            }
        }
    }

    fn label(self, start: u64) -> String {
        match self {
            Self::Month => format_date(start)[..7].to_string(),
            _ => format_date(start),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Bucket {
    pub bucket: String,
    pub start_ts: u64,
    pub end_ts: u64,
    #[serde(flatten)]
    pub summary: Summary,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
    pub search_calls: u64,
    pub fetch_calls: u64,
    pub balance_calls: u64,
    pub failed_calls: u64,
    pub cached_hits: u64,
    pub billable_calls: u64,
    pub estimated_cost_cny: f64,
}

impl Summary {
    fn add(&mut self, event: &Event) {
        self.first_ts = Some(self.first_ts.map_or(event.ts, |ts| ts.min(event.ts)));
        self.last_ts = Some(self.last_ts.map_or(event.ts, |ts| ts.max(event.ts)));
        match event.kind.as_str() {
            KIND_SEARCH => self.search_calls += 1,
            KIND_FETCH => self.fetch_calls += 1,
            KIND_BALANCE => self.balance_calls += 1,
            _ => {}
        }
        if !event.ok {
            self.failed_calls += 1;
        } else if event.cached {
            self.cached_hits += 1;
        } else if matches!(event.kind.as_str(), KIND_SEARCH | KIND_FETCH) {
            self.billable_calls += 1;
        }
    }

    fn finalize(mut self) -> Self {
        self.estimated_cost_cny = self.billable_calls as f64 * COST_PER_1000_CNY / 1000.0;
        self
    }
}

pub fn summarize_filtered(filter: TimeFilter) -> Result<Summary> {
    summarize_from(&path()?, filter)
}

fn summarize_from(path: &Path, filter: TimeFilter) -> Result<Summary> {
    let mut summary = Summary::default();
    if !path.exists() {
        return Ok(summary);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        if filter.includes(event.ts) {
            summary.add(&event);
        }
    }
    Ok(summary.finalize())
}

pub fn summarize_by(filter: TimeFilter, group_by: GroupBy) -> Result<Vec<Bucket>> {
    summarize_by_from(&path()?, filter, group_by)
}

fn summarize_by_from(path: &Path, filter: TimeFilter, group_by: GroupBy) -> Result<Vec<Bucket>> {
    let mut buckets: BTreeMap<u64, Summary> = BTreeMap::new();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        if !filter.includes(event.ts) {
            continue;
        }
        let start = group_by.bucket_start(event.ts);
        buckets.entry(start).or_default().add(&event);
    }

    Ok(buckets
        .into_iter()
        .map(|(start, summary)| Bucket {
            bucket: group_by.label(start),
            start_ts: start,
            end_ts: group_by.bucket_end(start),
            summary: summary.finalize(),
        })
        .collect())
}

pub fn render(summary: &Summary, json: bool) -> Result<String> {
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(summary)?));
    }

    let mut out = String::new();
    match (summary.first_ts, summary.last_ts) {
        (Some(first), Some(last)) => out.push_str(&format!(
            "period: {} .. {}\n",
            format_date(first),
            format_date(last)
        )),
        _ => out.push_str("no usage recorded\n"),
    }
    out.push_str(&format!("search calls: {}\n", summary.search_calls));
    out.push_str(&format!("fetch calls: {}\n", summary.fetch_calls));
    out.push_str(&format!("balance calls: {}\n", summary.balance_calls));
    out.push_str(&format!("failed calls: {}\n", summary.failed_calls));
    out.push_str(&format!("cache hits (free): {}\n", summary.cached_hits));
    out.push_str(&format!("billable calls: {}\n", summary.billable_calls));
    out.push_str(&format!(
        "estimated cost: ¥{:.3}\n",
        summary.estimated_cost_cny
    ));
    out.push_str(&format!("log: {}\n", path()?.display()));
    Ok(out)
}

pub fn render_buckets(buckets: &[Bucket], json: bool) -> Result<String> {
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(buckets)?));
    }
    if buckets.is_empty() {
        return Ok("no usage recorded in range\n".to_string());
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<12} {:>7} {:>7} {:>8} {:>7} {:>7} {:>8} {:>9}\n",
        "bucket", "search", "fetch", "balance", "failed", "cache", "billable", "cost"
    ));
    for bucket in buckets {
        out.push_str(&format!(
            "{:<12} {:>7} {:>7} {:>8} {:>7} {:>7} {:>8} {:>9}\n",
            bucket.bucket,
            bucket.summary.search_calls,
            bucket.summary.fetch_calls,
            bucket.summary.balance_calls,
            bucket.summary.failed_calls,
            bucket.summary.cached_hits,
            bucket.summary.billable_calls,
            format!("¥{:.3}", bucket.summary.estimated_cost_cny),
        ));
    }

    let search_calls: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.search_calls)
        .sum();
    let fetch_calls: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.fetch_calls)
        .sum();
    let balance_calls: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.balance_calls)
        .sum();
    let failed_calls: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.failed_calls)
        .sum();
    let cached_hits: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.cached_hits)
        .sum();
    let billable_calls: u64 = buckets
        .iter()
        .map(|bucket| bucket.summary.billable_calls)
        .sum();
    let cost: f64 = buckets
        .iter()
        .map(|bucket| bucket.summary.estimated_cost_cny)
        .sum();
    out.push_str(&format!(
        "{:<12} {:>7} {:>7} {:>8} {:>7} {:>7} {:>8} {:>9}\n",
        "total",
        search_calls,
        fetch_calls,
        balance_calls,
        failed_calls,
        cached_hits,
        billable_calls,
        format!("¥{cost:.3}"),
    ));
    Ok(out)
}

/// UTC calendar date for a unix timestamp, without pulling in chrono.
fn format_date(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

fn year_month(ts: u64) -> (i64, u32) {
    let date = format_date(ts);
    let year = date[0..4].parse().unwrap_or(1970);
    let month = date[5..7].parse().unwrap_or(1);
    (year, month)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let month_index = if month > 2 { month - 3 } else { month + 9 } as u64;
    let doy = (153 * month_index + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parse a UTC `YYYY-MM-DD` date into a unix timestamp at 00:00:00.
fn parse_date(value: &str) -> Result<u64> {
    if !value.is_ascii() || value.len() != 10 || &value[4..5] != "-" || &value[7..8] != "-" {
        bail!("invalid date {value:?}; expected YYYY-MM-DD");
    }
    let year: i64 = value[0..4]
        .parse()
        .with_context(|| format!("invalid year in {value:?}"))?;
    let month: u32 = value[5..7]
        .parse()
        .with_context(|| format!("invalid month in {value:?}"))?;
    let day: u32 = value[8..10]
        .parse()
        .with_context(|| format!("invalid day in {value:?}"))?;
    if !(1..=12).contains(&month) {
        bail!("invalid month in {value:?}");
    }
    if day == 0 || day > days_in_month(year, month) {
        bail!("invalid day in {value:?}");
    }
    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)
        .context("date is out of range")?;
    u64::try_from(seconds).context("date must be on or after 1970-01-01")
}

/// Parse `30d`, `4w`, `6m` or `1y` into seconds.
fn parse_period(value: &str) -> Result<u64> {
    if !value.is_ascii() {
        bail!("invalid period {value:?}; use e.g. 30d, 4w, 6m, 1y");
    }
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let count: u64 = number
        .parse()
        .with_context(|| format!("invalid period {value:?}; use e.g. 30d, 4w, 6m, 1y"))?;
    if count == 0 {
        bail!("period must be greater than zero");
    }
    let seconds = match unit {
        "d" => 86_400,
        "w" => 7 * 86_400,
        "m" => 30 * 86_400,
        "y" => 365 * 86_400,
        _ => bail!("invalid period unit in {value:?}; use d, w, m or y"),
    };
    count.checked_mul(seconds).context("period is too large")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> PathBuf {
        std::env::temp_dir().join(format!(
            "seekfetch-usage-test-{}-{}.jsonl",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn event(ts: u64, kind: &str, ok: bool, cached: bool) -> Event {
        Event {
            ts,
            kind: kind.to_string(),
            ok,
            cached,
        }
    }

    #[test]
    fn summarize_counts_kinds_and_cost() {
        let path = temp_log();
        append(&path, &event(1_700_000_000, KIND_SEARCH, true, false)).unwrap();
        append(&path, &event(1_700_000_000, KIND_SEARCH, true, true)).unwrap();
        append(&path, &event(1_700_000_000, KIND_FETCH, false, false)).unwrap();
        append(&path, &event(1_700_000_000, KIND_BALANCE, true, false)).unwrap();

        let summary = summarize_from(&path, TimeFilter::default()).unwrap();
        assert_eq!(summary.search_calls, 2);
        assert_eq!(summary.fetch_calls, 1);
        assert_eq!(summary.balance_calls, 1);
        assert_eq!(summary.failed_calls, 1);
        assert_eq!(summary.cached_hits, 1);
        assert_eq!(summary.billable_calls, 1);
        assert!((summary.estimated_cost_cny - 0.005).abs() < f64::EPSILON);
        assert_eq!(summary.first_ts, Some(1_700_000_000));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_log_is_an_empty_summary() {
        let path = temp_log();
        let summary = summarize_from(&path, TimeFilter::default()).unwrap();
        assert_eq!(summary.search_calls, 0);
        assert!(summary.first_ts.is_none());
    }

    #[test]
    fn time_filter_is_inclusive_at_start_and_exclusive_at_end() {
        let filter = TimeFilter {
            since: Some(100),
            until: Some(200),
        };
        assert!(!filter.includes(99));
        assert!(filter.includes(100));
        assert!(filter.includes(199));
        assert!(!filter.includes(200));
    }

    #[test]
    fn summarize_respects_time_filter() {
        let path = temp_log();
        let day_one = parse_date("2023-11-14").unwrap();
        let day_two = parse_date("2023-11-15").unwrap();
        append(&path, &event(day_one + 10, KIND_SEARCH, true, false)).unwrap();
        append(&path, &event(day_two + 10, KIND_FETCH, true, false)).unwrap();

        let summary = summarize_from(
            &path,
            TimeFilter {
                since: Some(day_two),
                until: None,
            },
        )
        .unwrap();
        assert_eq!(summary.search_calls, 0);
        assert_eq!(summary.fetch_calls, 1);
        assert_eq!(summary.first_ts, Some(day_two + 10));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_date_round_trips_and_rejects_bad_dates() {
        for date in ["1970-01-01", "2023-11-14", "2024-02-29", "2026-10-21"] {
            assert_eq!(format_date(parse_date(date).unwrap()), date);
        }
        assert_eq!(parse_date("1970-01-01").unwrap(), 0);
        assert!(parse_date("2023-02-29").is_err());
        assert!(parse_date("2023-13-01").is_err());
        assert!(parse_date("2023-1-01").is_err());
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("1969-12-31").is_err());
    }

    #[test]
    fn parse_period_supports_fixed_units() {
        assert_eq!(parse_period("30d").unwrap(), 30 * 86_400);
        assert_eq!(parse_period("4w").unwrap(), 28 * 86_400);
        assert_eq!(parse_period("6m").unwrap(), 180 * 86_400);
        assert_eq!(parse_period("1y").unwrap(), 365 * 86_400);
        assert!(parse_period("0d").is_err());
        assert!(parse_period("30x").is_err());
        assert!(parse_period("d").is_err());
    }

    #[test]
    fn groups_events_by_day_week_and_month() {
        let path = temp_log();
        let day_one = parse_date("2023-11-14").unwrap();
        let day_two = parse_date("2023-11-15").unwrap();
        let day_three = parse_date("2023-12-01").unwrap();
        append(&path, &event(day_one, KIND_SEARCH, true, false)).unwrap();
        append(&path, &event(day_one + 100, KIND_FETCH, true, false)).unwrap();
        append(&path, &event(day_two, KIND_SEARCH, true, true)).unwrap();
        append(&path, &event(day_three, KIND_SEARCH, false, false)).unwrap();

        let days = summarize_by_from(&path, TimeFilter::default(), GroupBy::Day).unwrap();
        assert_eq!(days.len(), 3);
        assert_eq!(days[0].bucket, "2023-11-14");
        assert_eq!(days[0].start_ts, day_one);
        assert_eq!(days[0].end_ts, day_one + 86_400);
        assert_eq!(days[0].summary.search_calls, 1);
        assert_eq!(days[0].summary.fetch_calls, 1);
        assert_eq!(days[0].summary.billable_calls, 2);
        assert_eq!(days[1].bucket, "2023-11-15");
        assert_eq!(days[1].summary.cached_hits, 1);

        let weeks = summarize_by_from(&path, TimeFilter::default(), GroupBy::Week).unwrap();
        assert_eq!(weeks.len(), 2);
        assert_eq!(weeks[0].bucket, "2023-11-13");
        assert_eq!(weeks[1].bucket, "2023-11-27");

        let months = summarize_by_from(&path, TimeFilter::default(), GroupBy::Month).unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(months[0].bucket, "2023-11");
        assert_eq!(months[0].summary.search_calls, 2);
        assert_eq!(months[0].summary.fetch_calls, 1);
        assert_eq!(months[1].bucket, "2023-12");
        assert_eq!(months[1].summary.failed_calls, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn format_date_renders_utc_calendar() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_date(1_735_689_600), "2025-01-01");
    }
}
