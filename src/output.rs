use anyhow::Result;

use crate::api::{Balance, FetchResult, SearchResult};

pub fn print_search(results: &[SearchResult], max_chars: usize, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
        return Ok(());
    }

    if results.is_empty() {
        println!("no results");
        return Ok(());
    }

    for (index, item) in results.iter().enumerate() {
        println!("{}. {}", index + 1, item.title);
        println!("   {}", item.url);
        if let Some(summary) = &item.summary {
            println!("   {}", truncate(summary, max_chars));
        }
        if let Some(content) = &item.content {
            println!("   {}", truncate(content, max_chars));
        }
        println!();
    }
    Ok(())
}

pub fn print_fetch(page: &FetchResult, max_chars: usize, json: bool) -> Result<()> {
    if json {
        let mut page = page.clone();
        page.content = truncate(&page.content, max_chars);
        println!("{}", serde_json::to_string_pretty(&page)?);
        return Ok(());
    }

    println!("{}", page.title);
    println!("{}", page.url);
    println!();
    println!("{}", truncate(&page.content, max_chars));
    Ok(())
}

pub fn print_balance(balance: &Balance, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(balance)?);
        return Ok(());
    }

    println!(
        "available: {}",
        if balance.is_available { "yes" } else { "no" }
    );
    if balance.balance_infos.is_empty() {
        println!("no balance information");
        return Ok(());
    }

    for info in &balance.balance_infos {
        println!("{}", info.currency);
        println!("  {:<19}{}", "total balance:", info.total_balance);
        println!("  {:<19}{}", "granted balance:", info.granted_balance);
        println!("  {:<19}{}", "topped-up balance:", info.topped_up_balance);
    }
    Ok(())
}

fn truncate(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count >= max_chars {
            out.push_str("…");
            break;
        }
        out.push(ch);
    }
    out
}
