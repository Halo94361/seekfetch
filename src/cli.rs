use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
        /// Query text; DeepSeek operators such as site:, after: and quotes are supported
        query: String,

        /// Maximum number of results (1-50)
        #[arg(short, long, default_value_t = 10)]
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
    },

    /// Fetch a URL through the DeepSeek Fetch API
    Fetch {
        url: String,

        /// Character cap for the fetched content
        #[arg(long, default_value_t = 20_000)]
        max_chars: usize,

        /// Print the structured JSON payload instead of plain text
        #[arg(long)]
        json: bool,
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

    /// Show the credential path and a masked key
    Show,

    /// Delete the stored credential
    Clear,
}
