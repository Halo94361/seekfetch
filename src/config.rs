use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

fn default_base_url() -> String {
    "https://api.deepseek.com".to_string()
}

fn default_timeout_s() -> u64 {
    // Slightly above the API's own ~30s gateway timeout, so the API's error
    // response (e.g. 504) usually arrives before the client gives up.
    35
}

fn default_count() -> u8 {
    10
}

fn default_max_chars() -> usize {
    300
}

fn default_cache_enabled() -> bool {
    true
}

// 24h, so work spanning a day boundary still hits the cache.
fn default_cache_ttl_s() -> u64 {
    86_400
}

fn default_usage_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_cache_enabled")]
    pub enabled: bool,
    /// Override the cache directory; defaults to the platform cache dir.
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default = "default_cache_ttl_s")]
    pub ttl_s: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_cache_enabled(),
            dir: None,
            ttl_s: default_cache_ttl_s(),
        }
    }
}

impl CacheConfig {
    pub fn dir(&self) -> Result<PathBuf> {
        match &self.dir {
            Some(dir) => Ok(dir.clone()),
            None => Ok(dirs::cache_dir()
                .context("cannot locate the platform cache directory")?
                .join("seekfetch")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageConfig {
    #[serde(default = "default_usage_enabled")]
    pub enabled: bool,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            enabled: default_usage_enabled(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    #[serde(default = "default_count")]
    pub default_count: u8,
    #[serde(default = "default_max_chars")]
    pub default_max_chars: usize,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub usage: UsageConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            timeout_s: default_timeout_s(),
            default_count: default_count(),
            default_max_chars: default_max_chars(),
            cache: CacheConfig::default(),
            usage: UsageConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("cannot locate the platform config directory")?
            .join("seekfetch");
        Ok(dir.join("config.toml"))
    }
}
