use std::path::Path;

use anyhow::{Context, Result};

use crate::api::{Balance, FetchResult, SearchResult};

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
