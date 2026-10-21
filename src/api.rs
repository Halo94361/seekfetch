use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::config::Config;
use crate::credentials;

const MAX_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY_MS: u64 = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    #[serde(default, deserialize_with = "empty_to_none")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, deserialize_with = "empty_to_none")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, deserialize_with = "empty_to_none")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<i64>,
    #[serde(default, deserialize_with = "empty_to_none")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    #[serde(default, deserialize_with = "empty_to_none")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    #[serde(default)]
    pub is_available: bool,
    #[serde(default)]
    pub balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    #[serde(default)]
    pub currency: String,
    #[serde(default, deserialize_with = "string_or_number")]
    pub total_balance: String,
    #[serde(default, deserialize_with = "string_or_number")]
    pub granted_balance: String,
    #[serde(default, deserialize_with = "string_or_number")]
    pub topped_up_balance: String,
}

/// Classifies an API failure; drives the CLI exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Network,
    Timeout,
    Auth,
    Payment,
    RateLimited,
    Api,
}

#[derive(Debug)]
pub struct ApiError {
    kind: ErrorKind,
    message: String,
}

impl ApiError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

pub struct DeepSeekClient {
    http: reqwest::Client,
    config: Config,
    api_key: String,
}

impl DeepSeekClient {
    pub fn from_config() -> Result<Self> {
        let config = Config::load()?;
        let api_key = credentials::load_key()?;
        Self::new(config, api_key)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn new(config: Config, api_key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_s))
            .user_agent(concat!("seekfetch/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            config,
            api_key,
        })
    }

    pub async fn search(
        &self,
        query: &str,
        count: u8,
        need_content: bool,
    ) -> Result<Vec<SearchResult>> {
        #[derive(Serialize)]
        struct SearchRequest<'a> {
            query: &'a str,
            count: u8,
            need_content: bool,
        }

        #[derive(Deserialize)]
        struct SearchResponse {
            #[serde(default)]
            results: Vec<SearchResult>,
        }

        let body = SearchRequest {
            query,
            count,
            need_content,
        };
        let response: SearchResponse = self.post_json("/v1/tools/search", &body).await?;
        let mut results = response.results;
        for item in &mut results {
            item.summary = item
                .summary
                .take()
                .map(|s| crate::clean::clean_markdown(&s));
            item.content = item
                .content
                .take()
                .map(|c| crate::clean::clean_markdown(&c));
        }
        Ok(results)
    }

    pub async fn fetch(&self, url: &str) -> Result<FetchResult> {
        #[derive(Serialize)]
        struct FetchRequest<'a> {
            url: &'a str,
        }

        #[derive(Deserialize)]
        struct FetchResponse {
            #[serde(default)]
            title: String,
            #[serde(default)]
            content: String,
        }

        let body = FetchRequest { url };
        let response: FetchResponse = self.post_json("/v1/tools/fetch", &body).await?;
        Ok(FetchResult {
            title: response.title,
            url: url.to_string(),
            content: crate::clean::clean_markdown(&response.content),
        })
    }

    pub async fn balance(&self) -> Result<Balance> {
        self.get_json("/user/balance").await
    }

    async fn post_json<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let body = serde_json::to_value(body).context("failed to encode the request body")?;
        self.request_json(reqwest::Method::POST, path, Some(body))
            .await
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request_json(reqwest::Method::GET, path, None).await
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), path);

        for attempt in 1..=MAX_ATTEMPTS {
            let mut request = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(&self.api_key);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let result = request.send().await;

            let response = match result {
                Ok(response) => response,
                Err(err) => {
                    if attempt < MAX_ATTEMPTS && is_retryable_transport(&err) {
                        backoff(attempt).await;
                        continue;
                    }
                    if err.is_timeout() {
                        return Err(ApiError::new(
                            ErrorKind::Timeout,
                            format!(
                                "the DeepSeek API did not respond within {}s (timeout_s)",
                                self.config.timeout_s
                            ),
                        )
                        .into());
                    }
                    return Err(ApiError::new(
                        ErrorKind::Network,
                        format!("failed to reach the DeepSeek API: {err}"),
                    )
                    .into());
                }
            };

            let status = response.status();
            if status.is_success() {
                return response.json().await.map_err(|err| {
                    anyhow::Error::new(ApiError::new(
                        ErrorKind::Api,
                        format!("failed to decode the DeepSeek API response: {err}"),
                    ))
                });
            }

            let detail = error_detail(response, &self.api_key).await;
            if attempt < MAX_ATTEMPTS && is_retryable_status(status) {
                backoff(attempt).await;
                continue;
            }
            let (kind, message) = map_error(status, &detail);
            return Err(ApiError::new(kind, message).into());
        }

        unreachable!("the loop always returns")
    }
}

async fn backoff(attempt: u32) {
    let delay = RETRY_BASE_DELAY_MS.saturating_mul(1u64 << (attempt - 1));
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

fn is_retryable_transport(err: &reqwest::Error) -> bool {
    // A timeout means the target did not answer within the budget; a retry would not either.
    !err.is_timeout() && (err.is_connect() || err.is_request())
}

fn is_retryable_status(status: StatusCode) -> bool {
    // 504 means the gateway already gave up on the target page; retrying just waits again.
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || (status.is_server_error() && status != StatusCode::GATEWAY_TIMEOUT)
}

struct ErrorInfo {
    message: String,
    code: Option<String>,
}

async fn error_detail(response: reqwest::Response, api_key: &str) -> ErrorInfo {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: ErrorDetail,
    }

    #[derive(Deserialize)]
    struct ErrorDetail {
        message: String,
        code: Option<String>,
        r#type: Option<String>,
    }

    let text = response.text().await.unwrap_or_default();
    let text = scrub_secrets(&text, api_key);
    match serde_json::from_str::<ErrorBody>(&text) {
        Ok(body) => ErrorInfo {
            message: body.error.message,
            code: body.error.code.or(body.error.r#type),
        },
        Err(_) => {
            let trimmed = text.trim();
            const MAX_DETAIL_CHARS: usize = 200;
            let message = if trimmed.chars().count() > MAX_DETAIL_CHARS {
                format!(
                    "{}…",
                    trimmed.chars().take(MAX_DETAIL_CHARS).collect::<String>()
                )
            } else {
                trimmed.to_string()
            };
            ErrorInfo {
                message,
                code: None,
            }
        }
    }
}

fn map_error(status: StatusCode, info: &ErrorInfo) -> (ErrorKind, String) {
    let code = match &info.code {
        Some(code) => format!(" [{code}]"),
        None => String::new(),
    };
    let detail = &info.message;
    match status {
        StatusCode::UNAUTHORIZED => (
            ErrorKind::Auth,
            format!("authentication failed (401){code}: {detail}; check the API key"),
        ),
        StatusCode::PAYMENT_REQUIRED => (
            ErrorKind::Payment,
            format!("insufficient DeepSeek balance (402){code}: {detail}; top up the account"),
        ),
        StatusCode::FORBIDDEN => (
            ErrorKind::Auth,
            format!("request rejected by security policy (403){code}: {detail}"),
        ),
        StatusCode::NOT_FOUND => (
            ErrorKind::Api,
            format!("endpoint not found or no usable content at this URL (404){code}: {detail}"),
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            ErrorKind::RateLimited,
            format!("rate limited after {MAX_ATTEMPTS} attempts (429){code}: {detail}"),
        ),
        StatusCode::GATEWAY_TIMEOUT => (
            ErrorKind::Api,
            format!("the target page did not answer in time (504 gateway timeout){code}: {detail}"),
        ),
        _ => (
            ErrorKind::Api,
            format!("DeepSeek API error ({status}){code}: {detail}"),
        ),
    }
}

fn empty_to_none<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    Ok(value.filter(|text| !text.trim().is_empty()))
}

// The official API sends amounts as strings, but compatible gateways send numbers.
fn string_or_number<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(serde_json::Number),
    }

    let value: Option<StringOrNumber> = Option::deserialize(deserializer)?;
    Ok(match value {
        Some(StringOrNumber::String(text)) => text,
        Some(StringOrNumber::Number(number)) => number.to_string(),
        None => String::new(),
    })
}

/// Strip credentials from text surfaced by the API before it reaches users or
/// logs: the configured key itself, plus anything shaped like an `sk-…` token.
pub fn scrub_secrets(text: &str, api_key: &str) -> String {
    let text = if api_key.chars().count() >= 8 {
        text.replace(api_key, "***")
    } else {
        text.to_string()
    };
    scrub_sk_tokens(&text)
}

fn scrub_sk_tokens(text: &str) -> String {
    const MIN_TOKEN_LEN: usize = 16;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("sk-") {
        let tail = &rest[pos + 3..];
        let run = tail
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        if run >= MIN_TOKEN_LEN {
            out.push_str(&rest[..pos]);
            out.push_str("sk-***");
            rest = &tail[run..];
        } else {
            out.push_str(&rest[..pos + 3]);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// Post-filter for search results (`--min-rerank`, `--min-authority`, `--domain`).
#[derive(Debug, Default, Clone)]
pub struct SearchFilter {
    pub min_rerank: Option<f64>,
    pub min_authority: Option<i64>,
    pub domains: Vec<String>,
}

impl SearchFilter {
    pub fn is_empty(&self) -> bool {
        self.min_rerank.is_none() && self.min_authority.is_none() && self.domains.is_empty()
    }

    /// Drop non-matching results; returns how many were removed.
    /// Results without a score fail a `--min-rerank`/`--min-authority` check.
    pub fn apply(&self, results: &mut Vec<SearchResult>) -> usize {
        let before = results.len();
        results.retain(|item| self.matches(item));
        before - results.len()
    }

    fn matches(&self, item: &SearchResult) -> bool {
        if let Some(min) = self.min_rerank {
            if item.rerank_score.unwrap_or(f64::NEG_INFINITY) < min {
                return false;
            }
        }
        if let Some(min) = self.min_authority {
            if item.authority.unwrap_or(i64::MIN) < min {
                return false;
            }
        }
        if !self.domains.is_empty() {
            let host = url_host(&item.url).to_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host);
            let matched = self.domains.iter().any(|domain| {
                let domain = domain.to_lowercase();
                let domain = domain.strip_prefix("www.").unwrap_or(&domain);
                host == domain || host.ends_with(&format!(".{domain}"))
            });
            if !matched {
                return false;
            }
        }
        true
    }
}

/// Extract the host part of an http(s) URL, without userinfo or port.
/// Search results are ordinary web URLs, so IPv6 literals are not handled.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let without_user = authority.rsplit('@').next().unwrap_or(authority);
    without_user.split(':').next().unwrap_or(without_user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct MockServer {
        base_url: String,
        hits: Arc<AtomicUsize>,
        request_lines: Arc<Mutex<Vec<String>>>,
    }

    impl MockServer {
        fn serve(responses: Vec<(&'static str, &'static str)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let thread_hits = hits.clone();
            let request_lines = Arc::new(Mutex::new(Vec::new()));
            let thread_lines = request_lines.clone();

            std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    thread_hits.fetch_add(1, Ordering::SeqCst);

                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    let mut first_line = true;
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) if line.trim().is_empty() => break,
                            Ok(_) => {
                                if std::mem::take(&mut first_line) {
                                    thread_lines.lock().unwrap().push(line.trim().to_string());
                                }
                            }
                        }
                    }

                    let raw = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let mut writer = stream;
                    let _ = writer.write_all(raw.as_bytes());
                    let _ = writer.flush();
                }
            });

            Self {
                base_url: format!("http://127.0.0.1:{port}"),
                hits,
                request_lines,
            }
        }

        fn hanging() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let thread_hits = hits.clone();
            let request_lines = Arc::new(Mutex::new(Vec::new()));

            std::thread::spawn(move || {
                while let Ok((stream, _)) = listener.accept() {
                    thread_hits.fetch_add(1, Ordering::SeqCst);
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) if line.trim().is_empty() => break,
                            Ok(_) => {}
                        }
                    }
                    // Never respond; the client must give up on its own timeout.
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });

            Self {
                base_url: format!("http://127.0.0.1:{port}"),
                hits,
                request_lines,
            }
        }

        fn client(&self) -> DeepSeekClient {
            self.client_with_timeout(10)
        }

        fn client_with_timeout(&self, timeout_s: u64) -> DeepSeekClient {
            let config = Config {
                base_url: self.base_url.clone(),
                timeout_s,
                ..Config::default()
            };
            DeepSeekClient::new(config, "sk-test".to_string()).unwrap()
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn request_lines(&self) -> Vec<String> {
            self.request_lines.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn search_maps_results() {
        let server = MockServer::serve(vec![(
            "200 OK",
            r#"{"results":[{"title":"A","url":"https://a.example","summary":"s","content":"","rerank_score":2.4,"authority":3,"site_name":"Site","date":"2026-06-01T00:00:00+00:00"}]}"#,
        )]);

        let results = server.client().search("q", 5, false).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A");
        assert_eq!(results[0].content, None);
        assert_eq!(results[0].authority, Some(3));
        assert_eq!(results[0].site_name.as_deref(), Some("Site"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn fetch_fills_url_from_request() {
        let server = MockServer::serve(vec![(
            "200 OK",
            r##"{"title":"Page","content":"# Hello"}"##,
        )]);

        let page = server
            .client()
            .fetch("https://example.com/article")
            .await
            .unwrap();
        assert_eq!(page.title, "Page");
        assert_eq!(page.url, "https://example.com/article");
        assert_eq!(page.content, "# Hello");
    }

    #[tokio::test]
    async fn retries_on_429_then_succeeds() {
        let server = MockServer::serve(vec![
            (
                "429 Too Many Requests",
                r#"{"error":{"message":"slow down"}}"#,
            ),
            ("200 OK", r#"{"results":[]}"#),
        ]);

        let results = server.client().search("q", 5, false).await.unwrap();
        assert!(results.is_empty());
        assert_eq!(server.hits(), 2);
    }

    #[tokio::test]
    async fn does_not_retry_400() {
        let server = MockServer::serve(vec![(
            "400 Bad Request",
            r#"{"error":{"message":"'count' must be between 1 and 50"}}"#,
        )]);

        let err = server.client().search("q", 5, false).await.unwrap_err();
        assert!(err.to_string().contains("'count' must be between 1 and 50"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn maps_401_to_auth_error() {
        let server = MockServer::serve(vec![(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid key"}}"#,
        )]);

        let err = server.client().search("q", 5, false).await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"));
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_on_500() {
        let responses: Vec<_> = (0..MAX_ATTEMPTS)
            .map(|_| {
                (
                    "500 Internal Server Error",
                    r#"{"error":{"message":"boom"}}"#,
                )
            })
            .collect();
        let server = MockServer::serve(responses);

        let err = server.client().search("q", 5, false).await.unwrap_err();
        assert!(err.to_string().contains("500"));
        assert_eq!(server.hits(), MAX_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn does_not_retry_504_gateway_timeout() {
        let server = MockServer::serve(vec![
            (
                "504 Gateway Timeout",
                r#"{"error":{"message":"Request timeout","code":"gateway_timeout_error"}}"#,
            ),
            ("200 OK", r#"{"results":[]}"#),
        ]);

        let err = server.client().search("q", 5, false).await.unwrap_err();
        assert!(err.to_string().contains("504"));
        assert!(err.to_string().contains("gateway_timeout_error"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn timeout_is_not_retried() {
        let server = MockServer::hanging();

        let err = server
            .client_with_timeout(1)
            .search("q", 5, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not respond within 1s"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn error_message_surfaces_api_code() {
        let server = MockServer::serve(vec![(
            "404 Not Found",
            r#"{"error":{"message":"failed to fetch URL","type":"not_found_error","code":"invalid_request_error"}}"#,
        )]);

        let err = server
            .client()
            .fetch("https://x.example")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no usable content"));
        assert!(err.to_string().contains("invalid_request_error"));
    }

    #[tokio::test]
    async fn balance_maps_infos() {
        let server = MockServer::serve(vec![(
            "200 OK",
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"93.80","granted_balance":"0.00","topped_up_balance":"93.80"}]}"#,
        )]);

        let balance = server.client().balance().await.unwrap();
        assert!(balance.is_available);
        assert_eq!(balance.balance_infos.len(), 1);
        let info = &balance.balance_infos[0];
        assert_eq!(info.currency, "CNY");
        assert_eq!(info.total_balance, "93.80");
        assert_eq!(info.granted_balance, "0.00");
        assert_eq!(info.topped_up_balance, "93.80");
        assert_eq!(server.request_lines()[0], "GET /user/balance HTTP/1.1");
    }

    #[tokio::test]
    async fn balance_accepts_numeric_amounts() {
        let server = MockServer::serve(vec![(
            "200 OK",
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":93.5,"granted_balance":0,"topped_up_balance":93.5}]}"#,
        )]);

        let balance = server.client().balance().await.unwrap();
        let info = &balance.balance_infos[0];
        assert_eq!(info.total_balance, "93.5");
        assert_eq!(info.granted_balance, "0");
        assert_eq!(info.topped_up_balance, "93.5");
    }

    #[tokio::test]
    async fn balance_tolerates_missing_and_null_fields() {
        let server = MockServer::serve(vec![
            (
                "200 OK",
                r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":null}]}"#,
            ),
            ("200 OK", r#"{"is_available":true}"#),
        ]);

        let with_null = server.client().balance().await.unwrap();
        assert!(!with_null.is_available);
        assert_eq!(with_null.balance_infos.len(), 1);
        assert_eq!(with_null.balance_infos[0].total_balance, "");
        assert_eq!(with_null.balance_infos[0].granted_balance, "");

        let without_infos = server.client().balance().await.unwrap();
        assert!(without_infos.balance_infos.is_empty());
    }

    #[tokio::test]
    async fn balance_maps_401_to_auth_error() {
        let server = MockServer::serve(vec![(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid key"}}"#,
        )]);

        let err = server.client().balance().await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn error_detail_redacts_key_material() {
        let server = MockServer::serve(vec![(
            "400 Bad Request",
            r#"{"error":{"message":"invalid key sk-0123456789abcdef0123456789abcdef provided"}}"#,
        )]);

        let err = server.client().search("q", 5, false).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("sk-***"));
        assert!(!message.contains("0123456789abcdef"));
    }

    #[test]
    fn scrub_secrets_redacts_key_and_tokens() {
        let key = "sk-abcdef0123456789abcdef0123456789";
        let scrubbed = scrub_secrets(&format!("invalid key {key} for account"), key);
        assert!(!scrubbed.contains(key));
        assert!(scrubbed.contains("***"));

        let other = scrub_secrets(
            "token sk-0123456789abcdef0123456789abcdef rejected",
            "sk-short",
        );
        assert_eq!(other, "token sk-*** rejected");

        let untouched = scrub_secrets("sk-tooshort stays", "sk-x");
        assert_eq!(untouched, "sk-tooshort stays");
    }

    #[test]
    fn search_filter_applies_thresholds_and_domains() {
        let make = |url: &str, rerank: Option<f64>, authority: Option<i64>| SearchResult {
            title: "t".into(),
            url: url.into(),
            summary: None,
            content: None,
            snippet: None,
            rerank_score: rerank,
            authority,
            site_name: None,
            date: None,
        };
        let mut results = vec![
            make("https://docs.example.com/a", Some(2.0), Some(3)),
            make("https://other.org/b", Some(0.5), Some(1)),
            make("https://example.com/c", None, None),
        ];

        let filter = SearchFilter {
            min_rerank: Some(1.5),
            min_authority: None,
            domains: vec!["example.com".into()],
        };
        let removed = filter.apply(&mut results);
        assert_eq!(removed, 2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://docs.example.com/a");

        let mut copy = results.clone();
        assert_eq!(SearchFilter::default().apply(&mut copy), 0);
        assert!(SearchFilter::default().is_empty());
    }

    #[test]
    fn error_kind_maps_exit_categories() {
        let (kind, _) = map_error(
            StatusCode::UNAUTHORIZED,
            &ErrorInfo {
                message: "m".into(),
                code: None,
            },
        );
        assert_eq!(kind, ErrorKind::Auth);
        let (kind, _) = map_error(
            StatusCode::PAYMENT_REQUIRED,
            &ErrorInfo {
                message: "m".into(),
                code: None,
            },
        );
        assert_eq!(kind, ErrorKind::Payment);
        let (kind, _) = map_error(
            StatusCode::TOO_MANY_REQUESTS,
            &ErrorInfo {
                message: "m".into(),
                code: None,
            },
        );
        assert_eq!(kind, ErrorKind::RateLimited);
    }
}
