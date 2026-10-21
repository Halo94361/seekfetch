//! Local response cache for search and fetch calls.
//!
//! Entries live under `<cache dir>/<kind>/<fnv1a-key>.json` and store the
//! original request next to the response payload, so `cache list`/`cache show`
//! can find entries by keyword even when the exact request is not reproduced.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{DeepSeekClient, FetchResult, SearchResult};
use crate::config::Config;

pub const KIND_SEARCH: &str = "search";
pub const KIND_FETCH: &str = "fetch";

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    created_at: u64,
    request: Value,
    payload: Value,
}

#[derive(Debug)]
pub struct Cached<T> {
    pub value: T,
    pub from_cache: bool,
    pub wrote_cache: bool,
}

/// How a call treats the local cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Read through the cache, writing misses.
    Normal,
    /// Bypass the cache entirely: no read, no write.
    NoCache,
    /// Skip the cache read, but overwrite the entry with the fresh response.
    Refresh,
}

impl CacheMode {
    pub fn from_flags(no_cache: bool, refresh: bool) -> Self {
        if no_cache {
            Self::NoCache
        } else if refresh {
            Self::Refresh
        } else {
            Self::Normal
        }
    }
}

impl Cache {
    pub fn new(dir: PathBuf, ttl_s: u64) -> Self {
        Self {
            dir,
            ttl: Duration::from_secs(ttl_s),
        }
    }

    /// Caching is opt-out: None when `[cache] enabled = false` or the cache
    /// directory cannot be located.
    pub fn from_config(config: &Config) -> Option<Self> {
        if !config.cache.enabled {
            return None;
        }
        config
            .cache
            .dir()
            .ok()
            .map(|dir| Self::new(dir, config.cache.ttl_s))
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    fn entry_path(&self, kind: &str, key: &str) -> PathBuf {
        self.dir.join(kind).join(format!("{key}.json"))
    }

    /// Missing, corrupt and expired entries all count as a miss; cache I/O
    /// problems must never fail a request. Expired files are removed lazily.
    fn get(&self, kind: &str, key: &str) -> Option<Value> {
        let path = self.entry_path(kind, key);
        let text = std::fs::read_to_string(&path).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        if self.is_expired(&entry) {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(entry.payload)
    }

    fn put(&self, kind: &str, key: &str, request: Value, payload: &Value) -> bool {
        let path = self.entry_path(kind, key);
        let Some(parent) = path.parent() else {
            return false;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
        let entry = Entry {
            created_at: now_secs(),
            request,
            payload: payload.clone(),
        };
        match serde_json::to_string(&entry) {
            Ok(text) => std::fs::write(&path, text).is_ok(),
            Err(_) => false,
        }
    }

    fn is_expired(&self, entry: &Entry) -> bool {
        now_secs().saturating_sub(entry.created_at) > self.ttl.as_secs()
    }

    /// All entries, newest first, optionally filtered by a case-insensitive
    /// keyword matched against the stored query or URL.
    fn entries(&self, keyword: Option<&str>) -> Vec<(String, String, u64, Entry)> {
        let keyword = keyword.map(str::to_lowercase);
        let mut out = Vec::new();
        for kind in [KIND_SEARCH, KIND_FETCH] {
            let Ok(read) = std::fs::read_dir(self.dir.join(kind)) else {
                continue;
            };
            for item in read.flatten() {
                let path = item.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(entry) = serde_json::from_str::<Entry>(&text) else {
                    continue;
                };
                if let Some(keyword) = &keyword {
                    if !request_summary(kind, &entry.request)
                        .to_lowercase()
                        .contains(keyword)
                    {
                        continue;
                    }
                }
                let key = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string();
                out.push((kind.to_string(), key, text.len() as u64, entry));
            }
        }
        out.sort_by(|a, b| b.3.created_at.cmp(&a.3.created_at));
        out
    }

    /// Delete every cached entry; returns how many were removed.
    pub fn clear(&self) -> Result<usize> {
        let mut removed = 0;
        for kind in [KIND_SEARCH, KIND_FETCH] {
            let dir = self.dir.join(kind);
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for item in read.flatten() {
                let path = item.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                    && std::fs::remove_file(&path).is_ok()
                {
                    removed += 1;
                }
            }
            // Remove the now-empty kind directory; ignore leftovers.
            let _ = std::fs::remove_dir(&dir);
        }
        Ok(removed)
    }

    pub fn render_stats(&self) -> String {
        let entries = self.entries(None);
        let expired = entries
            .iter()
            .filter(|(_, _, _, entry)| self.is_expired(entry))
            .count();
        let bytes: u64 = entries.iter().map(|(_, _, bytes, _)| bytes).sum();
        format!(
            "cache dir: {}\nttl: {}s\nentries: {} ({} fresh, {} expired)\nsize: {}\n",
            self.dir.display(),
            self.ttl.as_secs(),
            entries.len(),
            entries.len() - expired,
            expired,
            format_size(bytes),
        )
    }

    pub fn render_list(&self, keyword: Option<&str>) -> String {
        let entries = self.entries(keyword);
        if entries.is_empty() {
            return match keyword {
                Some(keyword) => format!("no cache entries matching \"{keyword}\"\n"),
                None => "cache is empty\n".to_string(),
            };
        }
        let mut out = String::new();
        for (kind, key, bytes, entry) in &entries {
            let age = format_age(now_secs().saturating_sub(entry.created_at));
            let expired = if self.is_expired(entry) {
                " (expired)"
            } else {
                ""
            };
            out.push_str(&format!(
                "[{kind}] {} — {age} ago{expired}, {}, key {key}\n",
                request_summary(kind, &entry.request),
                format_size(*bytes),
            ));
        }
        out
    }

    pub fn render_show(&self, keyword: &str, max_chars: usize) -> String {
        let entries = self.entries(Some(keyword));
        if entries.is_empty() {
            return format!("no cache entries matching \"{keyword}\"\n");
        }
        let mut out = String::new();
        for (index, (kind, key, _, entry)) in entries.iter().enumerate() {
            if index > 0 {
                out.push_str("\n---\n\n");
            }
            let age = format_age(now_secs().saturating_sub(entry.created_at));
            out.push_str(&format!(
                "[{kind}] {} — {age} ago, key {key}\n\n",
                request_summary(kind, &entry.request),
            ));
            let pretty = serde_json::to_string_pretty(&entry.payload).unwrap_or_default();
            out.push_str(&truncate(&pretty, max_chars));
            out.push('\n');
        }
        out
    }
}

/// Search with cache read-through. The cache key uses the normalized query
/// (trimmed, lowercased, whitespace-collapsed) so trivially different
/// spellings of the same query still hit.
pub async fn search(
    client: &DeepSeekClient,
    cache: Option<&Cache>,
    query: &str,
    count: u8,
    need_content: bool,
    mode: CacheMode,
) -> Result<Cached<Vec<SearchResult>>> {
    let cache = cache.filter(|_| mode != CacheMode::NoCache);
    let normalized = normalize_query(query);
    let request = serde_json::json!({
        "query": normalized,
        "count": count,
        "need_content": need_content,
    });
    let key = fnv1a64(&format!("search\0{normalized}\0{count}\0{need_content}"));

    if mode != CacheMode::Refresh {
        if let Some(cache) = cache {
            if let Some(payload) = cache.get(KIND_SEARCH, &key) {
                if let Ok(value) = serde_json::from_value(payload) {
                    return Ok(Cached {
                        value,
                        from_cache: true,
                        wrote_cache: false,
                    });
                }
            }
        }
    }

    let value = client.search(query, count, need_content).await?;
    let mut wrote_cache = false;
    if let Some(cache) = cache {
        if let Ok(payload) = serde_json::to_value(&value) {
            wrote_cache = cache.put(KIND_SEARCH, &key, request, &payload);
        }
    }
    Ok(Cached {
        value,
        from_cache: false,
        wrote_cache,
    })
}

/// Fetch with cache read-through. The cache key is the trimmed URL.
pub async fn fetch(
    client: &DeepSeekClient,
    cache: Option<&Cache>,
    url: &str,
    mode: CacheMode,
) -> Result<Cached<FetchResult>> {
    let cache = cache.filter(|_| mode != CacheMode::NoCache);
    let url = url.trim();
    let request = serde_json::json!({ "url": url });
    let key = fnv1a64(&format!("fetch\0{url}"));

    if mode != CacheMode::Refresh {
        if let Some(cache) = cache {
            if let Some(payload) = cache.get(KIND_FETCH, &key) {
                if let Ok(value) = serde_json::from_value(payload) {
                    return Ok(Cached {
                        value,
                        from_cache: true,
                        wrote_cache: false,
                    });
                }
            }
        }
    }

    let value = client.fetch(url).await?;
    let mut wrote_cache = false;
    if let Some(cache) = cache {
        if let Ok(payload) = serde_json::to_value(&value) {
            wrote_cache = cache.put(KIND_FETCH, &key, request, &payload);
        }
    }
    Ok(Cached {
        value,
        from_cache: false,
        wrote_cache,
    })
}

fn request_summary(kind: &str, request: &Value) -> String {
    if kind == KIND_SEARCH {
        let query = request.get("query").and_then(Value::as_str).unwrap_or("");
        let count = request.get("count").and_then(Value::as_u64).unwrap_or(0);
        let need_content = request
            .get("need_content")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut flags = Vec::new();
        if count > 0 {
            flags.push(format!("count={count}"));
        }
        if need_content {
            flags.push("need_content".to_string());
        }
        if flags.is_empty() {
            format!("\"{query}\"")
        } else {
            format!("\"{query}\" ({})", flags.join(", "))
        }
    } else {
        request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
}

fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn fnv1a64(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h{}m", secs / 3_600, (secs % 3_600) / 60)
    } else {
        format!("{}d{}h", secs / 86_400, (secs % 86_400) / 3_600)
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_048_576 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache(ttl_s: u64) -> (Cache, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "seekfetch-cache-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        (Cache::new(dir.clone(), ttl_s), dir)
    }

    #[test]
    fn put_get_roundtrip_and_keyword_lookup() {
        let (cache, dir) = temp_cache(3_600);
        cache.put(
            KIND_SEARCH,
            "k1",
            serde_json::json!({"query": "alpha beta", "count": 10, "need_content": false}),
            &serde_json::json!([{"title": "A"}]),
        );
        cache.put(
            KIND_FETCH,
            "k2",
            serde_json::json!({"url": "https://gamma.example/page"}),
            &serde_json::json!({"title": "G"}),
        );

        let hit = cache.get(KIND_SEARCH, "k1").unwrap();
        assert_eq!(hit[0]["title"], "A");
        assert!(cache.get(KIND_SEARCH, "missing").is_none());

        assert_eq!(cache.entries(None).len(), 2);
        assert_eq!(cache.entries(Some("ALPHA")).len(), 1);
        assert_eq!(cache.entries(Some("gamma.EXAMPLE")).len(), 1);
        assert!(cache.entries(Some("nope")).is_empty());

        assert!(cache.render_list(None).contains("alpha beta"));
        assert!(cache.render_show("alpha", 500).contains("\"title\": \"A\""));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expired_entries_are_a_lazy_miss() {
        let (cache, dir) = temp_cache(50);
        let path = cache.entry_path(KIND_FETCH, "old");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let entry = Entry {
            created_at: now_secs().saturating_sub(100),
            request: serde_json::json!({"url": "https://x.example"}),
            payload: serde_json::json!({"title": "X"}),
        };
        std::fs::write(&path, serde_json::to_string(&entry).unwrap()).unwrap();

        assert!(cache.get(KIND_FETCH, "old").is_none());
        assert!(!path.exists(), "expired entry should be removed on read");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_everything() {
        let (cache, dir) = temp_cache(3_600);
        cache.put(
            KIND_FETCH,
            "k",
            serde_json::json!({"url": "https://x.example"}),
            &serde_json::json!({}),
        );
        assert_eq!(cache.clear().unwrap(), 1);
        assert!(cache.entries(None).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_normalization_collapses_case_and_whitespace() {
        assert_eq!(
            normalize_query("  DeepSeek\t  Search \n Tools "),
            "deepseek search tools"
        );
        assert_eq!(fnv1a64("a"), fnv1a64("a"));
        assert_ne!(fnv1a64("a"), fnv1a64("b"));
    }

    #[test]
    fn cache_mode_from_flags() {
        assert_eq!(CacheMode::from_flags(false, false), CacheMode::Normal);
        assert_eq!(CacheMode::from_flags(true, false), CacheMode::NoCache);
        assert_eq!(CacheMode::from_flags(false, true), CacheMode::Refresh);
        assert_eq!(CacheMode::from_flags(true, true), CacheMode::NoCache);
    }
}
