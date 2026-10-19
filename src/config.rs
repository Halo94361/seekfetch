use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

fn default_base_url() -> String {
    "https://api.deepseek.com".to_string()
}

fn default_timeout_s() -> u64 {
    60
}

fn default_count() -> u8 {
    10
}

fn default_max_chars() -> usize {
    300
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            timeout_s: default_timeout_s(),
            default_count: default_count(),
            default_max_chars: default_max_chars(),
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
