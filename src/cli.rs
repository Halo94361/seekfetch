use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Debug, Parser)]
#[command(name = "seekfetch", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run as an MCP server over stdio
    Mcp,

    /// Search the web through the DeepSeek Search API
    Search {
        /// Query text; DeepSeek operators such as site:, after: and quotes are supported.
        /// Reads from stdin when omitted and stdin is piped
        query: Option<String>,

        /// Maximum number of results (1-50)
        #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=50))]
        count: u8,

        /// Include each result's page content
        #[arg(long)]
        need_content: bool,

        /// Per-result character cap for summaries and content
        #[arg(long, default_value_t = 300)]
        max_chars: usize,

        /// Print the structured JSON payload instead of a human-readable list
        #[arg(long)]
        json: bool,

        /// Keep only results with a rerank score of at least this value
        #[arg(long, value_name = "SCORE")]
        min_rerank: Option<f64>,

        /// Keep only results with an authority of at least this value
        #[arg(long, value_name = "LEVEL")]
        min_authority: Option<i64>,

        /// Keep only results from this domain (repeatable; matches subdomains)
        #[arg(long, value_name = "DOMAIN")]
        domain: Vec<String>,

        /// Bypass the local cache for this request
        #[arg(long)]
        no_cache: bool,

        /// Force a fresh request and overwrite the cached entry
        #[arg(long, conflicts_with = "no_cache")]
        refresh: bool,

        /// Write the output to this file instead of stdout
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// Fetch one or more URLs through the DeepSeek Fetch API
    Fetch {
        /// URLs to fetch; reads lines from stdin when omitted and stdin is piped
        urls: Vec<String>,

        /// Read URLs from this file (one per line; blank lines and # comments ignored)
        #[arg(long, value_name = "PATH")]
        input: Option<PathBuf>,

        /// Character cap for each fetched content
        #[arg(long, default_value_t = 20_000)]
        max_chars: usize,

        /// Print the structured JSON payload instead of plain text
        #[arg(long)]
        json: bool,

        /// Bypass the local cache for this request
        #[arg(long)]
        no_cache: bool,

        /// Force a fresh request and overwrite the cached entry
        #[arg(long, conflicts_with = "no_cache")]
        refresh: bool,

        /// Write the output to this file instead of stdout
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// Show the DeepSeek account balance
    Balance {
        /// Print the structured JSON payload instead of a human-readable summary
        #[arg(long)]
        json: bool,

        /// Write the output to this file instead of stdout
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// Manage the local response cache
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Show local API usage statistics and the estimated cost
    Usage {
        /// Print the structured JSON payload instead of a human-readable summary
        #[arg(long)]
        json: bool,
        /// Only include events on or after this UTC date (YYYY-MM-DD)
        #[arg(long, value_name = "YYYY-MM-DD", conflicts_with = "last")]
        since: Option<String>,
        /// Only include events on or before this UTC date (inclusive)
        #[arg(long, value_name = "YYYY-MM-DD", conflicts_with = "last")]
        until: Option<String>,
        /// Only include the last N days/weeks/months/years (e.g. 30d, 4w, 6m, 1y)
        #[arg(long, value_name = "PERIOD", conflicts_with_all = ["since", "until"])]
        last: Option<String>,
        /// Group results by day, week or month
        #[arg(long, value_enum)]
        group_by: Option<crate::usage::GroupBy>,
    },

    /// Print shell completions to stdout
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Manage configuration and credentials
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Diagnose configuration, credentials and connectivity
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Show entry count, size on disk and expiry status
    Stats,

    /// Delete all cached entries
    Clear,

    /// Print the cache directory path
    Path,

    /// List cached requests, optionally filtered by a keyword
    List {
        /// Case-insensitive substring matched against the cached query or URL
        keyword: Option<String>,
    },

    /// Show the cached response payload of entries matching a keyword
    Show {
        /// Case-insensitive substring matched against the cached query or URL
        keyword: String,

        /// Character cap for each entry's payload preview
        #[arg(long, default_value_t = 2_000)]
        max_chars: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Store the API key in the encrypted credential file
    SetKey {
        /// Read the key from stdin instead of prompting
        #[arg(long)]
        stdin: bool,

        /// Read the key from this environment variable
        #[arg(long, value_name = "NAME")]
        from_env: Option<String>,
    },

    /// Import a key from a dotenv-style file
    Import {
        /// Path to the dotenv file
        #[arg(long, value_name = "PATH")]
        dotenv: PathBuf,

        /// Variable name to read from the file
        #[arg(long, default_value = "DEEPSEEK_API_KEY")]
        name: String,
    },

    /// Show paths, the effective configuration and a masked key
    Show,

    /// Print the configuration and credential file paths
    Path,

    /// Delete the stored credential
    Clear,
}
