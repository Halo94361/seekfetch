use std::time::Duration;

use anyhow::{bail, Context, Result};
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
        Ok(response.results)
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
            content: response.content,
        })
    }

    async fn post_json<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), path);

        for attempt in 1..=MAX_ATTEMPTS {
            let result = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await;

            let response = match result {
                Ok(response) => response,
                Err(err) => {
                    if attempt < MAX_ATTEMPTS && is_retryable_transport(&err) {
                        backoff(attempt).await;
                        continue;
                    }
                    if err.is_timeout() {
                        return Err(err).context(format!(
                            "the DeepSeek API did not respond within {}s (timeout_s); for fetch this usually means the target page could not be fetched",
                            self.config.timeout_s
                        ));
                    }
                    return Err(err).context("failed to reach the DeepSeek API");
                }
            };

            let status = response.status();
            if status.is_success() {
                return response
                    .json()
                    .await
                    .context("failed to decode the DeepSeek API response");
            }

            let detail = error_detail(response).await;
            if attempt < MAX_ATTEMPTS && is_retryable_status(status) {
                backoff(attempt).await;
                continue;
            }
            bail!(map_error(status, &detail));
        }

        unreachable!("the loop always returns or bails")
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

async fn error_detail(response: reqwest::Response) -> ErrorInfo {
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

fn map_error(status: StatusCode, info: &ErrorInfo) -> String {
    let code = match &info.code {
        Some(code) => format!(" [{code}]"),
        None => String::new(),
    };
    let detail = &info.message;
    match status {
        StatusCode::UNAUTHORIZED => {
            format!("authentication failed (401){code}: {detail}; check the API key")
        }
        StatusCode::PAYMENT_REQUIRED => {
            format!("insufficient DeepSeek balance (402){code}: {detail}; top up the account")
        }
        StatusCode::FORBIDDEN => {
            format!("request rejected by security policy (403){code}: {detail}")
        }
        StatusCode::NOT_FOUND => format!("no usable content at this URL (404){code}: {detail}"),
        StatusCode::TOO_MANY_REQUESTS => {
            format!("rate limited after {MAX_ATTEMPTS} attempts (429){code}: {detail}")
        }
        StatusCode::GATEWAY_TIMEOUT => {
            format!("the target page did not answer in time (504 gateway timeout){code}: {detail}")
        }
        _ => format!("DeepSeek API error ({status}){code}: {detail}"),
    }
}

fn empty_to_none<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    Ok(value.filter(|text| !text.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MockServer {
        base_url: String,
        hits: Arc<AtomicUsize>,
    }

    impl MockServer {
        fn serve(responses: Vec<(&'static str, &'static str)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let thread_hits = hits.clone();

            std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    thread_hits.fetch_add(1, Ordering::SeqCst);

                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) if line.trim().is_empty() => break,
                            Ok(_) => {}
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
            }
        }

        fn hanging() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let thread_hits = hits.clone();

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
            }
        }

        fn client(&self) -> DeepSeekClient {
            self.client_with_timeout(10)
        }

        fn client_with_timeout(&self, timeout_s: u64) -> DeepSeekClient {
            let config = Config {
                base_url: self.base_url.clone(),
                timeout_s,
                default_count: 10,
                default_max_chars: 300,
            };
            DeepSeekClient::new(config, "sk-test".to_string()).unwrap()
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
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
}
