use anyhow::Result;
use serde_json::{json, Value};

use crate::api::DeepSeekClient;
use crate::config::Config;

const PROTOCOL_VERSION: &str = "2024-11-05";
const FETCH_DEFAULT_MAX_CHARS: usize = 20_000;

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
                },
                "required": ["query"],
            },
        },
        {
            "name": "deepseek_fetch",
            "description": "Fetch a web page through the DeepSeek Fetch API and return it as Markdown.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP/HTTPS URL to fetch",
                    },
                    "max_chars": {
                        "type": "integer",
                        "default": FETCH_DEFAULT_MAX_CHARS,
                        "description": "Character cap for the returned content",
                    },
                },
                "required": ["url"],
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

            match run_search(&query, count, need_content, max_chars).await {
                Ok(output) => Ok(tool_success(output)),
                Err(err) => Ok(tool_error_result(err)),
            }
        }
        "deepseek_fetch" => {
            let url = string_argument(&arguments, "url")?;
            let max_chars =
                integer_argument(&arguments, "max_chars", FETCH_DEFAULT_MAX_CHARS as u64)? as usize;

            match run_fetch(&url, max_chars).await {
                Ok(output) => Ok(tool_success(output)),
                Err(err) => Ok(tool_error_result(err)),
            }
        }
        "deepseek_balance" => match run_balance().await {
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

async fn run_search(query: &str, count: u8, need_content: bool, max_chars: usize) -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let results = client.search(query, count, need_content).await?;
    Ok(search_output(&results, max_chars))
}

async fn run_fetch(url: &str, max_chars: usize) -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let page = client.fetch(url).await?;
    Ok(fetch_output(
        &page.title,
        &page.url,
        &page.content,
        max_chars,
    ))
}

async fn run_balance() -> Result<Value> {
    let client = DeepSeekClient::from_config()?;
    let balance = client.balance().await?;
    Ok(serde_json::to_value(balance)?)
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
            ["deepseek_search", "deepseek_fetch", "deepseek_balance"]
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
