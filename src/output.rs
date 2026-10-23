use std::path::Path;

use anyhow::{Context, Result};

use crate::api::{Balance, FetchResult, SearchResult};
use crate::opencode::UsageReport;

/// Write rendered output to stdout, or overwrite the given file.
/// The "wrote" notice goes to stderr so stdout stays pure data.
pub fn emit(text: &str, output: Option<&Path>) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, text)
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!("wrote {}", path.display());
            Ok(())
        }
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

pub fn render_search(results: &[SearchResult], max_chars: usize, json: bool) -> Result<String> {
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(results)?));
    }

    if results.is_empty() {
        return Ok("no results\n".to_string());
    }

    let mut out = String::new();
    for (index, item) in results.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", index + 1, item.title));
        out.push_str(&format!("   {}\n", item.url));
        if let Some(summary) = &item.summary {
            out.push_str(&format!("   {}\n", truncate(summary, max_chars)));
        }
        if let Some(content) = &item.content {
            out.push_str(&format!("   {}\n", truncate(content, max_chars)));
        }
        out.push('\n');
    }
    Ok(out)
}

/// One entry of a (possibly batch) fetch: the requested URL and either the
/// fetched page or the error message.
pub struct FetchPage {
    pub url: String,
    pub result: Result<FetchResult, String>,
}

pub fn render_fetch(pages: &[FetchPage], max_chars: usize, json: bool) -> Result<String> {
    if json {
        let mut items = Vec::with_capacity(pages.len());
        for page in pages {
            match &page.result {
                Ok(result) => {
                    let mut result = result.clone();
                    result.content = truncate(&result.content, max_chars);
                    items.push(serde_json::to_value(&result)?);
                }
                Err(err) => items.push(serde_json::json!({"url": page.url, "error": err})),
            }
        }
        // A single fetch keeps the historical object shape; batches use an array.
        let value = if items.len() == 1 {
            items.pop().unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Array(items)
        };
        return Ok(format!("{}\n", serde_json::to_string_pretty(&value)?));
    }

    let mut out = String::new();
    let mut first = true;
    for page in pages {
        // Failed fetches are reported on stderr by the caller, not inline.
        let Ok(result) = &page.result else {
            continue;
        };
        if !first {
            out.push_str("\n---\n\n");
        }
        first = false;
        out.push_str(&result.title);
        out.push('\n');
        out.push_str(&result.url);
        out.push_str("\n\n");
        out.push_str(&truncate(&result.content, max_chars));
        out.push('\n');
    }
    Ok(out)
}

pub fn render_balance(balance: &Balance, json: bool) -> Result<String> {
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(balance)?));
    }

    let mut out = format!(
        "available: {}\n",
        if balance.is_available { "yes" } else { "no" }
    );
    if balance.balance_infos.is_empty() {
        out.push_str("no balance information\n");
        return Ok(out);
    }

    for info in &balance.balance_infos {
        out.push_str(&format!("{}\n", info.currency));
        out.push_str(&format!(
            "  {:<19}{}\n",
            "total balance:", info.total_balance
        ));
        out.push_str(&format!(
            "  {:<19}{}\n",
            "granted balance:", info.granted_balance
        ));
        out.push_str(&format!(
            "  {:<19}{}\n",
            "topped-up balance:", info.topped_up_balance
        ));
    }
    Ok(out)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn render_opencode_usage(report: &UsageReport, json: bool) -> Result<String> {
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(report)?));
    }

    let mut out = String::new();
    for (name, window) in [
        ("rolling", &report.usage.rolling),
        ("weekly", &report.usage.weekly),
        ("monthly", &report.usage.monthly),
    ] {
        match window {
            Some(window) => {
                let status = if window.status.is_empty() {
                    "-"
                } else {
                    window.status.as_str()
                };
                let reset = window
                    .resets_at
                    .as_deref()
                    .map(format_reset)
                    .unwrap_or_else(|| "-".to_string());
                out.push_str(&format!(
                    "{:<8}{:>4}  {}  resets {}\n",
                    name,
                    format_percent(window.percent),
                    status,
                    reset
                ));
            }
            None => out.push_str(&format!("{:<8}    -  -\n", name)),
        }
    }
    Ok(out)
}

fn format_percent(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

/// `2026-10-23T10:53:28.000Z` -> `2026-10-23 10:53 UTC`.
fn format_reset(value: &str) -> String {
    let utc = value.ends_with('Z');
    let base = value.trim_end_matches('Z');
    let base = base.split('.').next().unwrap_or(base);
    match base.split_once('T') {
        Some((date, time)) => {
            let time: String = time.chars().take(5).collect();
            if utc {
                format!("{date} {time} UTC")
            } else {
                format!("{date} {time}")
            }
        }
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode::{UsageWindow, UsageWindows};

    fn window(percent: f64, resets_at: &str) -> UsageWindow {
        UsageWindow {
            status: "ok".to_string(),
            percent,
            resets_at: Some(resets_at.to_string()),
        }
    }

    #[test]
    fn render_opencode_usage_lists_all_windows() {
        let report = UsageReport {
            usage: UsageWindows {
                rolling: Some(window(7.0, "2026-10-23T10:53:28.000Z")),
                weekly: Some(window(3.5, "2026-10-26T00:00:00.000Z")),
                monthly: None,
            },
        };

        let text = render_opencode_usage(&report, false).unwrap();
        assert!(text.contains("rolling   7%"));
        assert!(text.contains("resets 2026-10-23 10:53 UTC"));
        assert!(text.contains("weekly  3.5%"));
        assert!(text.contains("monthly     -"));
    }

    #[test]
    fn render_opencode_usage_json_keeps_payload_shape() {
        let report = UsageReport {
            usage: UsageWindows {
                rolling: Some(window(7.0, "2026-10-23T10:53:28.000Z")),
                weekly: None,
                monthly: None,
            },
        };

        let text = render_opencode_usage(&report, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["usage"]["rolling"]["percent"], 7.0);
        assert_eq!(
            value["usage"]["rolling"]["resetsAt"],
            "2026-10-23T10:53:28.000Z"
        );
    }

    #[test]
    fn format_reset_handles_offsets_and_plain_values() {
        assert_eq!(
            format_reset("2026-10-23T10:53:28.000Z"),
            "2026-10-23 10:53 UTC"
        );
        assert_eq!(
            format_reset("2026-10-23T10:53:28+08:00"),
            "2026-10-23 10:53"
        );
        assert_eq!(format_reset("unknown"), "unknown");
    }

    #[test]
    fn format_percent_drops_a_zero_fraction() {
        assert_eq!(format_percent(7.0), "7%");
        assert_eq!(format_percent(3.5), "3.5%");
    }
}
