//! OpenCode Go quota client.
//!
//! The upstream endpoint reports only the percentage used in three windows
//! (rolling 5 hours, weekly, monthly). It is not billed, so nothing is cached
//! and no local usage event is recorded. The API key uses the same encrypted
//! credential mechanism as the DeepSeek key.

use std::time::Duration;

use anyhow::Result;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, ErrorKind};
use crate::config::Config;
use crate::credentials;

/// Origin of the public OpenCode proxy. Local proxies (e.g. a session proxy on
/// port 58787) do not expose this route, so requests go to the upstream host.
pub const BASE_URL: &str = "https://opencode.ai";
pub const USAGE_PATH: &str = "/zen/go/v1/usage";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub usage: UsageWindows,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageWindows {
    #[serde(default)]
    pub rolling: Option<UsageWindow>,
    #[serde(default)]
    pub weekly: Option<UsageWindow>,
    #[serde(default)]
    pub monthly: Option<UsageWindow>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageWindow {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub percent: f64,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<String>,
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    timeout_s: u64,
}

impl Client {
    pub fn from_config() -> Result<Self> {
        let config = Config::load()?;
        let api_key = credentials::KeyStore::OPENCODE.load()?;
        Self::new(BASE_URL, config.timeout_s, api_key)
    }

    pub fn new(base_url: impl Into<String>, timeout_s: u64, api_key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_s))
            .user_agent(concat!("seekfetch/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            api_key,
            timeout_s,
        })
    }

    pub async fn usage(&self) -> Result<UsageReport> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), USAGE_PATH);
        let response = self.http.get(&url).bearer_auth(&self.api_key).send().await;

        let response = match response {
            Ok(response) => response,
            Err(err) if err.is_timeout() => {
                return Err(ApiError::new(
                    ErrorKind::Timeout,
                    format!(
                        "the OpenCode usage endpoint did not respond within {}s (timeout_s)",
                        self.timeout_s
                    ),
                )
                .into());
            }
            Err(err) => {
                return Err(ApiError::new(
                    ErrorKind::Network,
                    format!("failed to reach the OpenCode usage endpoint: {err}"),
                )
                .into());
            }
        };

        let status = response.status();
        if status.is_success() {
            return response.json().await.map_err(|err| {
                anyhow::Error::new(ApiError::new(
                    ErrorKind::Api,
                    format!("failed to decode the OpenCode usage response: {err}"),
                ))
            });
        }

        let detail = error_detail(response, &self.api_key).await;
        let (kind, message) = map_error(status, &detail);
        Err(ApiError::new(kind, message).into())
    }
}

/// Pull a readable message out of an error body, redacting key material.
async fn error_detail(response: reqwest::Response, api_key: &str) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Option<Detail>,
        message: Option<String>,
    }

    #[derive(Deserialize)]
    struct Detail {
        message: Option<String>,
        code: Option<String>,
    }

    let text = crate::api::scrub_secrets(&response.text().await.unwrap_or_default(), api_key);
    let body = serde_json::from_str::<Body>(&text).ok();
    let message = body
        .as_ref()
        .and_then(|body| {
            body.error
                .as_ref()
                .and_then(|error| error.message.clone())
                .or_else(|| body.message.clone())
        })
        .unwrap_or_else(|| text.trim().to_string());
    let code = body
        .as_ref()
        .and_then(|body| body.error.as_ref())
        .and_then(|error| error.code.clone());

    let message = truncate(&message, 200);
    match code {
        Some(code) => format!("[{code}] {message}"),
        None => message,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn map_error(status: StatusCode, detail: &str) -> (ErrorKind, String) {
    let detail = detail.trim();
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => (
            ErrorKind::Auth,
            format!(
                "OpenCode Go rejected the API key ({status}){suffix}; store a valid key with `seekfetch opencode set-key`"
            ),
        ),
        StatusCode::PAYMENT_REQUIRED => (
            ErrorKind::Payment,
            format!("OpenCode Go quota exhausted ({status}){suffix}"),
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            ErrorKind::RateLimited,
            format!("OpenCode Go rate limited the request ({status}){suffix}"),
        ),
        StatusCode::NOT_FOUND => (
            ErrorKind::Api,
            format!("OpenCode Go usage endpoint not found ({status}){suffix}"),
        ),
        _ => (
            ErrorKind::Api,
            format!("OpenCode Go API error ({status}){suffix}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    struct MockServer {
        base_url: String,
        request_lines: Arc<Mutex<Vec<String>>>,
    }

    impl MockServer {
        fn serve(status: &'static str, body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let request_lines = Arc::new(Mutex::new(Vec::new()));
            let thread_lines = request_lines.clone();

            std::thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
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
            });

            Self {
                base_url: format!("http://127.0.0.1:{port}"),
                request_lines,
            }
        }

        fn client(&self) -> Client {
            Client::new(self.base_url.clone(), 10, "oc_sk_test".to_string()).unwrap()
        }

        fn request_lines(&self) -> Vec<String> {
            self.request_lines.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn usage_maps_windows() {
        let server = MockServer::serve(
            "200 OK",
            r#"{"usage":{"rolling":{"status":"ok","percent":7,"resetsAt":"2026-10-23T10:53:28.000Z"},"weekly":{"status":"ok","percent":3,"resetsAt":"2026-10-26T00:00:00.000Z"},"monthly":{"status":"ok","percent":1,"resetsAt":"2026-11-23T05:35:12.000Z"}}}"#,
        );

        let report = server.client().usage().await.unwrap();
        let rolling = report.usage.rolling.expect("rolling window");
        assert_eq!(rolling.status, "ok");
        assert_eq!(rolling.percent, 7.0);
        assert_eq!(
            rolling.resets_at.as_deref(),
            Some("2026-10-23T10:53:28.000Z")
        );
        assert_eq!(report.usage.weekly.unwrap().percent, 3.0);
        assert_eq!(report.usage.monthly.unwrap().percent, 1.0);
        assert_eq!(server.request_lines()[0], "GET /zen/go/v1/usage HTTP/1.1");
    }

    #[tokio::test]
    async fn usage_tolerates_missing_windows() {
        let server = MockServer::serve("200 OK", r#"{"usage":{"rolling":{"percent":4}}}"#);

        let report = server.client().usage().await.unwrap();
        assert_eq!(report.usage.rolling.unwrap().percent, 4.0);
        assert!(report.usage.weekly.is_none());
        assert!(report.usage.monthly.is_none());
    }

    #[tokio::test]
    async fn maps_401_to_auth_error() {
        let server = MockServer::serve(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid key","code":"invalid_api_key"}}"#,
        );

        let err = server.client().usage().await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("rejected the API key"));
        assert!(message.contains("invalid_api_key"));
    }

    #[tokio::test]
    async fn maps_429_to_rate_limited() {
        let server = MockServer::serve(
            "429 Too Many Requests",
            r#"{"error":{"message":"slow down"}}"#,
        );

        let err = server.client().usage().await.unwrap_err();
        let api = err.downcast_ref::<ApiError>().expect("typed error");
        assert_eq!(api.kind(), ErrorKind::RateLimited);
    }

    #[test]
    fn error_detail_redacts_key_material() {
        // The scrubber runs on the response body; keep the token shape realistic.
        let scrubbed = crate::api::scrub_secrets(
            "key oc_sk_0123456789abcdef0123456789abcdef rejected",
            "oc_sk_test",
        );
        assert!(!scrubbed.contains("0123456789abcdef"));
    }
}
