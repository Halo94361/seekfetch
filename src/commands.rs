//! Implementations behind the CLI subcommands, kept out of main.rs.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::api::{DeepSeekClient, SearchFilter};
use crate::cache::{self, Cache, Cached};
use crate::cli::CacheCommand;
use crate::config::Config;
use crate::credentials;
use crate::output::{self, FetchPage};
use crate::usage;

/// Exit code when a batch fetch completes but some URLs failed.
pub const EXIT_PARTIAL_FAILURE: i32 = 9;

#[allow(clippy::too_many_arguments)]
pub async fn search(
    query: Option<String>,
    count: u8,
    need_content: bool,
    max_chars: usize,
    json: bool,
    min_rerank: Option<f64>,
    min_authority: Option<i64>,
    domain: Vec<String>,
    no_cache: bool,
    refresh: bool,
    output: Option<PathBuf>,
) -> Result<i32> {
    let query = resolve_query(query)?;
    let client = DeepSeekClient::from_config()?;
    let cache = Cache::from_config(client.config());

    let outcome = cache::search(
        &client,
        cache.as_ref(),
        &query,
        count,
        need_content,
        cache::CacheMode::from_flags(no_cache, refresh),
    )
    .await;
    record(&client, usage::KIND_SEARCH, &outcome);
    let outcome = outcome?;
    if outcome.from_cache {
        eprintln!("cache hit");
    } else if refresh && outcome.wrote_cache {
        eprintln!("cache refreshed");
    }

    let mut results = outcome.value;
    let filter = SearchFilter {
        min_rerank,
        min_authority,
        domains: domain,
    };
    if !filter.is_empty() {
        let removed = filter.apply(&mut results);
        if removed > 0 {
            eprintln!("filtered out {removed} result(s)");
        }
    }

    let text = output::render_search(&results, max_chars, json)?;
    output::emit(&text, output.as_deref())?;
    Ok(0)
}

pub async fn fetch(
    urls: Vec<String>,
    input: Option<PathBuf>,
    max_chars: usize,
    json: bool,
    no_cache: bool,
    refresh: bool,
    output: Option<PathBuf>,
) -> Result<i32> {
    let urls = collect_urls(urls, input)?;
    let client = DeepSeekClient::from_config()?;
    let cache = Cache::from_config(client.config());
    let mode = cache::CacheMode::from_flags(no_cache, refresh);

    if urls.len() == 1 {
        let outcome = cache::fetch(&client, cache.as_ref(), &urls[0], mode).await;
        record(&client, usage::KIND_FETCH, &outcome);
        let outcome = outcome?;
        if outcome.from_cache {
            eprintln!("cache hit");
        } else if refresh && outcome.wrote_cache {
            eprintln!("cache refreshed");
        }
        let pages = vec![FetchPage {
            url: urls[0].clone(),
            result: Ok(outcome.value),
        }];
        let text = output::render_fetch(&pages, max_chars, json)?;
        output::emit(&text, output.as_deref())?;
        return Ok(0);
    }

    // Sequential on purpose: the API allows 5 QPS and batch failures must not
    // abort the rest of the list.
    let mut pages = Vec::with_capacity(urls.len());
    let mut failures = 0usize;
    for url in &urls {
        let outcome = cache::fetch(&client, cache.as_ref(), url, mode).await;
        record(&client, usage::KIND_FETCH, &outcome);
        match outcome {
            Ok(outcome) => pages.push(FetchPage {
                url: url.clone(),
                result: Ok(outcome.value),
            }),
            Err(err) => {
                failures += 1;
                eprintln!("error: {url}: {err:#}");
                pages.push(FetchPage {
                    url: url.clone(),
                    result: Err(format!("{err:#}")),
                });
            }
        }
    }

    let text = output::render_fetch(&pages, max_chars, json)?;
    output::emit(&text, output.as_deref())?;
    if failures > 0 {
        eprintln!("{failures}/{} fetches failed", urls.len());
        return Ok(EXIT_PARTIAL_FAILURE);
    }
    Ok(0)
}

pub async fn balance(json: bool, output: Option<PathBuf>) -> Result<i32> {
    let client = DeepSeekClient::from_config()?;
    let result = client.balance().await;
    usage::record(client.config(), usage::KIND_BALANCE, result.is_ok(), false);
    let balance = result?;
    let text = output::render_balance(&balance, json)?;
    output::emit(&text, output.as_deref())?;
    Ok(0)
}

pub fn cache_command(command: &CacheCommand) -> Result<i32> {
    let config = Config::load()?;
    // Cache management works even when caching is disabled in the config.
    let cache = Cache::new(config.cache.dir()?, config.cache.ttl_s);
    match command {
        CacheCommand::Stats => print!("{}", cache.render_stats()),
        CacheCommand::Clear => {
            let removed = cache.clear()?;
            println!("removed {removed} cache entries");
        }
        CacheCommand::Path => println!("{}", cache.dir().display()),
        CacheCommand::List { keyword } => print!("{}", cache.render_list(keyword.as_deref())),
        CacheCommand::Show { keyword, max_chars } => {
            print!("{}", cache.render_show(keyword, *max_chars))
        }
    }
    Ok(0)
}

pub fn usage_command(json: bool) -> Result<i32> {
    let summary = usage::summarize()?;
    let text = usage::render(&summary, json)?;
    print!("{text}");
    Ok(0)
}

pub fn completions(shell: clap_complete::Shell) -> Result<i32> {
    use clap::CommandFactory;

    let mut command = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut command, "seekfetch", &mut std::io::stdout());
    Ok(0)
}

pub fn config_paths() -> Result<i32> {
    println!("config: {}", Config::path()?.display());
    println!("credentials: {}", credentials::path()?.display());
    Ok(0)
}

pub fn show_config() -> Result<i32> {
    let config = Config::load()?;
    println!("config: {}", Config::path()?.display());
    println!("credentials: {}", credentials::path()?.display());
    println!("base_url: {}", config.base_url);
    println!("timeout_s: {}", config.timeout_s);
    println!("default_count: {}", config.default_count);
    println!("default_max_chars: {}", config.default_max_chars);
    println!(
        "cache: enabled={} ttl_s={} dir={}",
        config.cache.enabled,
        config.cache.ttl_s,
        display_path(config.cache.dir())
    );
    println!(
        "usage: enabled={} log={}",
        config.usage.enabled,
        display_path(usage::path())
    );
    match credentials::load_key() {
        Ok(key) => {
            let from_env = std::env::var(credentials::KEY_ENV)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            let source = if from_env {
                format!("environment {}", credentials::KEY_ENV)
            } else {
                "credential file".to_string()
            };
            println!("api key: {} ({source})", credentials::mask_key(&key));
        }
        Err(err) => println!("api key: {err}"),
    }
    Ok(0)
}

pub async fn doctor() -> Result<i32> {
    let config = Config::load()?;
    println!("config: {}", Config::path()?.display());
    println!("credentials: {}", credentials::path()?.display());
    println!("base_url: {}", config.base_url);
    match config.cache.dir() {
        Ok(dir) => println!(
            "cache dir: {} (enabled={}, ttl={}s)",
            dir.display(),
            config.cache.enabled,
            config.cache.ttl_s
        ),
        Err(err) => println!("cache dir: {err}"),
    }

    match credentials::load_key() {
        Ok(_) => println!("api key: configured"),
        Err(err) => {
            println!("api key: {err}");
            return Ok(1);
        }
    }

    // The connectivity probe bypasses the cache on purpose: doctor must
    // exercise the real API.
    let client = DeepSeekClient::from_config()?;
    match client.search("deepseek", 1, false).await {
        Ok(_) => {
            usage::record(&config, usage::KIND_SEARCH, true, false);
            println!("connectivity: ok");
            Ok(0)
        }
        Err(err) => {
            usage::record(&config, usage::KIND_SEARCH, false, false);
            println!("connectivity: {err}");
            Ok(1)
        }
    }
}

fn record<T>(client: &DeepSeekClient, kind: &str, outcome: &Result<Cached<T>>) {
    usage::record(
        client.config(),
        kind,
        outcome.is_ok(),
        outcome.as_ref().map(|o| o.from_cache).unwrap_or(false),
    );
}

fn display_path(path: Result<PathBuf>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unavailable".to_string())
}

fn resolve_query(query: Option<String>) -> Result<String> {
    match query {
        Some(query) if !query.trim().is_empty() => Ok(query.trim().to_string()),
        _ if !std::io::stdin().is_terminal() => {
            let query = read_stdin()?.trim().to_string();
            if query.is_empty() {
                bail!("query is required (pass it as an argument or pipe it via stdin)");
            }
            Ok(query)
        }
        _ => bail!("query is required (pass it as an argument or pipe it via stdin)"),
    }
}

fn collect_urls(urls: Vec<String>, input: Option<PathBuf>) -> Result<Vec<String>> {
    let mut out: Vec<String> = urls;
    if let Some(path) = input {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        out.extend(url_lines(&text));
    }
    if out.is_empty() && !std::io::stdin().is_terminal() {
        out.extend(url_lines(&read_stdin()?));
    }
    let out: Vec<String> = out
        .into_iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect();
    if out.is_empty() {
        bail!("no URLs given (pass them as arguments, via --input, or pipe them via stdin)");
    }
    Ok(out)
}

/// One URL per line; blank lines and `#` comments are ignored.
fn url_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("failed to read from stdin")?;
    Ok(buf)
}
