//! Local usage log: one JSON line per API call, for cost awareness.
//! Append-only; recording and reading problems never fail a request.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
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

/// Record one call. Silent on any failure; honors `[usage] enabled = false`.
pub fn record(config: &Config, kind: &str, ok: bool, cached: bool) {
    if !config.usage.enabled {
        return;
    }
    let Ok(path) = path() else {
        return;
    };
    let event = Event {
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
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

pub fn summarize() -> Result<Summary> {
    summarize_from(&path()?)
}

fn summarize_from(path: &Path) -> Result<Summary> {
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
        summary.first_ts = Some(summary.first_ts.map_or(event.ts, |ts| ts.min(event.ts)));
        summary.last_ts = Some(summary.last_ts.map_or(event.ts, |ts| ts.max(event.ts)));
        match event.kind.as_str() {
            KIND_SEARCH => summary.search_calls += 1,
            KIND_FETCH => summary.fetch_calls += 1,
            KIND_BALANCE => summary.balance_calls += 1,
            _ => {}
        }
        if !event.ok {
            summary.failed_calls += 1;
        } else if event.cached {
            summary.cached_hits += 1;
        } else if matches!(event.kind.as_str(), KIND_SEARCH | KIND_FETCH) {
            summary.billable_calls += 1;
        }
    }
    summary.estimated_cost_cny = summary.billable_calls as f64 * COST_PER_1000_CNY / 1000.0;
    Ok(summary)
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
        _ => out.push_str("no usage recorded yet\n"),
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

    #[test]
    fn summarize_counts_kinds_and_cost() {
        let path = temp_log();
        let event = |kind: &str, ok: bool, cached: bool| Event {
            ts: 1_700_000_000,
            kind: kind.to_string(),
            ok,
            cached,
        };
        append(&path, &event(KIND_SEARCH, true, false)).unwrap();
        append(&path, &event(KIND_SEARCH, true, true)).unwrap();
        append(&path, &event(KIND_FETCH, false, false)).unwrap();
        append(&path, &event(KIND_BALANCE, true, false)).unwrap();

        let summary = summarize_from(&path).unwrap();
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
        let summary = summarize_from(&path).unwrap();
        assert_eq!(summary.search_calls, 0);
        assert!(summary.first_ts.is_none());
    }

    #[test]
    fn format_date_renders_utc_calendar() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_date(1_735_689_600), "2025-01-01");
    }
}
