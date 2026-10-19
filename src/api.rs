use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::credentials;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub rerank_score: Option<f64>,
    #[serde(default)]
    pub authority: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

pub struct DeepSeekClient {
    config: Config,
    api_key: String,
}

impl DeepSeekClient {
    pub fn from_config() -> Result<Self> {
        let config = Config::load()?;
        let api_key = credentials::load_key()?;
        Ok(Self { config, api_key })
    }

    pub async fn search(
        &self,
        _query: &str,
        _count: u8,
        _need_content: bool,
    ) -> Result<Vec<SearchResult>> {
        let _ = (&self.config, &self.api_key);
        // TODO: POST {base_url}/v1/tools/search with retry and error mapping.
        bail!("DeepSeek Search API is not implemented yet")
    }

    pub async fn fetch(&self, _url: &str) -> Result<FetchResult> {
        let _ = (&self.config, &self.api_key);
        // TODO: POST {base_url}/v1/tools/fetch with retry and error mapping.
        bail!("DeepSeek Fetch API is not implemented yet")
    }
}
