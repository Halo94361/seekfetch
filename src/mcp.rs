use anyhow::Result;
use serde_json::{json, Value};

use crate::api::{DeepSeekClient, SearchFilter};
use crate::cache::{self, Cache, Cached};
use crate::config::Config;
use crate::usage;

const PROTOCOL_VERSION: &str = "2024-11-05";
const FETCH_DEFAULT_MAX_CHARS: usize = 20_000;
/// Batch fetches through MCP are capped so a single call cannot run away
/// with the (billed) API quota.
const MAX_BATCH_URLS: usize = 20;

pub async fn run() -> Result<()> {
    use std::io::{BufRead, Write};

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(text) => {
                    if tx.blocking_send(text).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let config = Config::load()?;
    let mut stdout = std::io::stdout();
    while let Some(line) = rx.recv().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line) {
            Ok(message) => handle_message(message, &config).await,
            Err(_) => Some(error_response(Value::Null, -32700, "parse error")),
        };
        if let Some(response) = response {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

async fn handle_message(message: Value, config: &Config) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str);
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let (id, method) = match (id, method) {
        (Some(id), Some(method)) => (id, method),
        // Requests without a method are invalid; notifications without one are noise.
        (Some(id), None) => return Some(error_response(id, -32600, "invalid request")),
        (None, _) => return None,
    };

    if method.starts_with("notifications/") {
        return None;
    }

    match dispatch(method, params, config).await {
        Ok(result) => Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
        Err(error) => Some(error_response(id, error.code, &error.message)),
    }
}

#[derive(Debug)]
struct McpError {
    code: i64,
    message: String,
}

impl McpError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

async fn dispatch(method: &str, params: Value, config: &Config) -> Result<Value, McpError> {
    match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions(config)})),
        "tools/call" => call_tool(params, config).await,
        _ => Err(McpError {
            code: -32601,
            message: format!("method not found: {method}"),
        }),
    }
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": requested,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "seekfetch",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn tool_definitions(config: &Config) -> Value {
    json!([
        {
            "name": "deepseek_search",
            "description": "Search the web through the DeepSeek Search API. Each result has title, url, summary, optional content, rerank_score (>= 1.5 is high quality) and authority (>= 3 is highly authoritative, -1 unknown).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query; supports operators such as site:, -site:, lang:, after:, before: and quoted phrases",
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": config.default_count,
                        "description": "Maximum number of results (1-50)",
                    },
                    "need_content": {
                        "type": "boolean",
                        "default": false,
                        "description": "Include each result's page content (slower, larger output)",
                    },
                    "max_chars": {
                        "type": "integer",
                        "default": config.default_max_chars,
                        "description": "Per-result character cap for summary and content",
                    },
                    "min_rerank": {
                        "type": "number",
                        "description": "Keep only results with a rerank score of at least this value",
                    },
                    "min_authority": {
                        "type": "integer",
                        "description": "Keep only results with an authority of at least this value",
                    },
                    "domain": {
                        "type": "string",
                        "description": "Keep only results from this domain and its subdomains",
                    },
                    "no_cache": {
                        "type": "boolean",
                        "default": false,
                        "description": "Bypass the local response cache for this call",
                    },
                    "refresh": {
                        "type": "boolean",
                        "default": false,
                        "description": "Force a fresh request and overwrite the cached entry",
                    },
                },
                "required": ["query"],
            },
        },
        {
            "name": "deepseek_fetch",
            "description": "Fetch one or more web pages through the DeepSeek Fetch API and return them as Markdown. Pass either `url` or `urls`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP/HTTPS URL to fetch",
                    },
                    "urls": {
                        "type": "array",
                        "items": {"type": "string"},
                        "maxItems": MAX_BATCH_URLS,
                        "description": "Batch mode: URLs to fetch sequentially; failures are reported per URL and do not abort the batch",
                    },
                    "max_chars": {
                        "type": "integer",
                        "default": FETCH_DEFAULT_MAX_CHARS,
                        "description": "Character cap for each returned content",
                    },
                    "no_cache": {
                        "type": "boolean",
                        "default": false,
                        "description": "Bypass the local response cache for this call",
                    },
                    "refresh": {
                        "type": "boolean",
                        "default": false,
                        "description": "Force a fresh request and overwrite the cached entry",
                    },
                },
            },
        },
        {
            "name": "deepseek_balance",
            "description": "Show the DeepSeek account balance. Returns is_available and balance_infos with currency, total_balance, granted_balance and topped_up_balance.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            },
        },
        {
            "name": "opencode_usage",
            "description": "Show OpenCode Go quota usage. Returns the percentage used in the rolling (5-hour), weekly and monthly windows, plus each window's status and reset time.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            },
        },
    ])
}

async fn call_tool(params: Value, config: &Config) -> Result<Value, McpError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params("tools/call requires a string `name`"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match name {
        "deepseek_search" => {
            let query = string_argument(&arguments, "query")?;
            let count = integer_argument(&arguments, "count", config.default_count as u64)?;
            let need_content = arguments
                .get("need_content")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let max_chars =
                integer_argument(&arguments, "max_chars", config.default_max_chars as u64)?
                    as usize;
            let count = u8::try_from(count.clamp(1, 50))
                .map_err(|_| McpError::invalid_params("`count` must be between 1 and 50"))?;
            let filter = SearchFilter {
                min_rerank: arguments.get("min_rerank").and_then(Value::as_f64),
                min_authority: arguments.get("min_authority").and_then(Value::as_i64),
                domains: arguments
                    .get("domain")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .into_iter()
                    .collect(),
            };
            let mode = cache_mode_argument(&arguments);

            match run_search(
                config,
                &query,
                count,
                need_content,
                max_chars,
                &filter,
                mode,
            )
            .await
            {
                Ok(output) => Ok(tool_success(output)),
                Err(err) => Ok(tool_error_result(err)),
            }
        }
        "deepseek_fetch" => {
            let max_chars =
                integer_argument(&arguments, "max_chars", FETCH_DEFAULT_MAX_CHARS as u64)? as usize;
            let mode = cache_mode_argument(&arguments);
            let url = arguments
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string);
            let urls = urls_argument(&arguments)?;

            match (url, urls) {
                (Some(url), None) => match run_fetch(config, &url, max_chars, mode).await {
                    Ok(output) => Ok(tool_success(output)),
                    Err(err) => Ok(tool_error_result(err)),
                },
                (None, Some(urls)) => {
                    if urls.is_empty() {
                        return Err(McpError::invalid_params("`urls` must not be empty"));
                    }
                    if urls.len() > MAX_BATCH_URLS {
                        return Err(McpError::invalid_params(format!(
                            "`urls` accepts at most {MAX_BATCH_URLS} URLs"
                        )));
                    }
                    // Sequential on purpose: the API allows 5 QPS and batch
                    // failures must not abort the rest of the list.
                    let mut outputs = Vec::with_capacity(urls.len());
                    for url in urls {
                        match run_fetch(config, &url, max_chars, mode).await {
                            Ok(output) => outputs.push(output),
                            Err(err) => outputs.push(json!({
                                "url": url,
                                "error": format!("{err:#}"),
                            })),
                        }
                    }
                    Ok(tool_success(Value::Array(outputs)))
                }
                _ => Err(McpError::invalid_params(
                    "provide exactly one of `url` or `urls`",
                )),
            }
        }
        "deepseek_balance" => match run_balance(config).await {
            Ok(output) => Ok(tool_success(output)),
            Err(err) => Ok(tool_error_result(err)),
        },
        "opencode_usage" => match run_opencode_usage().await {
            Ok(output) => Ok(tool_success(output)),
            Err(err) => Ok(tool_error_result(err)),
        },
        _ => Err(McpError::invalid_params(format!("unknown tool: {name}"))),
    }
}

fn string_argument(arguments: &Value, name: &str) -> Result<String, McpError> {
    let value = arguments.get(name).and_then(Value::as_str).ok_or_else(|| {
        McpError::invalid_params(format!("missing or invalid string argument `{name}`"))
    })?;
    if value.trim().is_empty() {
        return Err(McpError::invalid_params(format!(
            "argument `{name}` must not be empty"
        )));
    }
    Ok(value.to_string())
}

fn integer_argument(arguments: &Value, name: &str, default: u64) -> Result<u64, McpError> {
    match arguments.get(name) {
        None => Ok(default),
        Some(value) => value.as_u64().ok_or_else(|| {
            McpError::invalid_params(format!("argument `{name}` must be a non-negative integer"))
        }),
    }
}

fn cache_mode_argument(arguments: &Value) -> cache::CacheMode {
    cache::CacheMode::from_flags(
        arguments
            .get("no_cache")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        arguments
            .get("refresh")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

fn urls_argument(arguments: &Value) -> Result<Option<Vec<String>>, McpError> {
    match arguments.get("urls") {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut urls = Vec::with_capacity(items.len());
            for item in items {
                let Some(url) = item.as_str() else {
                    return Err(McpError::invalid_params("`urls` items must be strings"));
                };
                let url = url.trim();
                if url.is_empty() {
                    return Err(McpError::invalid_params("`urls` items must not be empty"));
                }
                urls.push(url.to_string());
            }
            Ok(Some(urls))
        }
        Some(_) => Err(McpError::invalid_params(
            "`urls` must be an array of strings",
        )),
    }
}

async fn run_search(
    config: &Config,
    query: &str,
    count: u8,
    need_content: bool,
    max_chars: usize,
    filter: &SearchFilter,
    mode: cache::CacheMode,
) -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let cache = Cache::from_config(client.config());
    let outcome = cache::search(&client, cache.as_ref(), query, count, need_content, mode).await;
    record_usage(config, usage::KIND_SEARCH, &outcome);
    let mut results = outcome?.value;
    filter.apply(&mut results);
    Ok(search_output(&results, max_chars))
}

async fn run_fetch(
    config: &Config,
    url: &str,
    max_chars: usize,
    mode: cache::CacheMode,
) -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let cache = Cache::from_config(client.config());
    let outcome = cache::fetch(&client, cache.as_ref(), url, mode).await;
    record_usage(config, usage::KIND_FETCH, &outcome);
    let page = outcome?.value;
    Ok(fetch_output(
        &page.title,
        &page.url,
        &page.content,
        max_chars,
    ))
}

async fn run_balance(config: &Config) -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let result = client.balance().await;
    usage::record(config, usage::KIND_BALANCE, result.is_ok(), false);
    let balance = result?;
    Ok(serde_json::to_value(balance)?)
}

async fn run_opencode_usage() -> Result<Value> {
    let client = crate::opencode::Client::from_config()?;
    let report = client.usage().await?;
    Ok(serde_json::to_value(report)?)
}

fn record_usage<T>(config: &Config, kind: &str, outcome: &Result<Cached<T>>) {
    usage::record(
        config,
        kind,
        outcome.is_ok(),
        outcome.as_ref().map(|o| o.from_cache).unwrap_or(false),
    );
}

fn tool_success(output: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default()}],
    })
}

// Tool-level failures are delivered as isError results, not protocol errors.
fn tool_error_result(err: anyhow::Error) -> Value {
    json!({
        "content": [{"type": "text", "text": format!("error: {err:#}")}],
        "isError": true,
    })
}

fn search_output(results: &[crate::api::SearchResult], max_chars: usize) -> Value {
    let items: Vec<Value> = results
        .iter()
        .map(|item| {
            let mut entry = json!({"title": item.title, "url": item.url});
            if let Some(summary) = &item.summary {
                entry["summary"] = json!(truncate(summary, max_chars).0);
            }
            if let Some(content) = &item.content {
                entry["content"] = json!(truncate(content, max_chars).0);
            }
            if let Some(rerank_score) = item.rerank_score {
                entry["rerank_score"] = json!(rerank_score);
            }
            if let Some(authority) = item.authority {
                entry["authority"] = json!(authority);
            }
            entry
        })
        .collect();
    Value::Array(items)
}

fn fetch_output(title: &str, url: &str, content: &str, max_chars: usize) -> Value {
    let (content, truncated) = truncate(content, max_chars);
    json!({
        "title": title,
        "url": url,
        "content": content,
        "truncated": truncated,
    })
}

fn truncate(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    (out, true)
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_requested_version() {
        let result = initialize_result(&json!({"protocolVersion": "2025-03-26"}));
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "seekfetch");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_defaults_version() {
        let result = initialize_result(&Value::Null);
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn tools_list_exposes_all_tools() {
        let result = dispatch("tools/list", Value::Null, &Config::default())
            .await
            .unwrap();
        let names: Vec<&str> = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(
            names,
            [
                "deepseek_search",
                "deepseek_fetch",
                "deepseek_balance",
                "opencode_usage"
            ]
        );
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let error = dispatch("resources/list", Value::Null, &Config::default())
            .await
            .unwrap_err();
        assert_eq!(error.code, -32601);
    }

    #[tokio::test]
    async fn tools_call_rejects_unknown_tool() {
        let error = dispatch(
            "tools/call",
            json!({"name": "nope", "arguments": {}}),
            &Config::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, -32602);
    }

    #[tokio::test]
    async fn tools_call_requires_query() {
        let error = dispatch(
            "tools/call",
            json!({"name": "deepseek_search", "arguments": {}}),
            &Config::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("query"));
    }

    #[tokio::test]
    async fn fetch_requires_exactly_one_of_url_or_urls() {
        for arguments in [
            json!({}),
            json!({"url": "https://a.example", "urls": ["https://b.example"]}),
        ] {
            let error = dispatch(
                "tools/call",
                json!({"name": "deepseek_fetch", "arguments": arguments}),
                &Config::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, -32602);
            assert!(error.message.contains("url"));
        }
    }

    #[tokio::test]
    async fn fetch_rejects_bad_urls_argument() {
        for arguments in [
            json!({"urls": "https://a.example"}),
            json!({"urls": ["https://a.example", 42]}),
            json!({"urls": ["  "]}),
            json!({"urls": []}),
        ] {
            let error = dispatch(
                "tools/call",
                json!({"name": "deepseek_fetch", "arguments": arguments}),
                &Config::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, -32602, "arguments: {arguments}");
        }
    }

    #[tokio::test]
    async fn fetch_rejects_oversized_batch() {
        let urls: Vec<String> = (0..MAX_BATCH_URLS + 1)
            .map(|i| format!("https://{i}.example"))
            .collect();
        let error = dispatch(
            "tools/call",
            json!({"name": "deepseek_fetch", "arguments": {"urls": urls}}),
            &Config::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("at most"));
    }

    #[tokio::test]
    async fn notifications_yield_no_response() {
        let message = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle_message(message, &Config::default()).await.is_none());
    }

    #[tokio::test]
    async fn request_without_method_is_invalid() {
        let message = json!({"jsonrpc": "2.0", "id": 1});
        let response = handle_message(message, &Config::default()).await.unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn truncate_flags_only_real_truncation() {
        let (text, truncated) = truncate("hello", 10);
        assert_eq!(text, "hello");
        assert!(!truncated);

        let (text, truncated) = truncate("hello world", 5);
        assert_eq!(text, "hello…");
        assert!(truncated);
    }

    #[test]
    fn fetch_output_marks_truncated() {
        let output = fetch_output("T", "https://example.com", "abcdefghij", 4);
        assert_eq!(output["content"], "abcd…");
        assert_eq!(output["truncated"], true);
    }
}
