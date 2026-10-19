mod api;
mod clean;
mod cli;
mod config;
mod credentials;
mod mcp;
mod output;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command, ConfigCommand};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Mcp => mcp::run().await,
        Command::Search {
            query,
            count,
            need_content,
            max_chars,
            json,
        } => {
            let client = api::DeepSeekClient::from_config()?;
            let results = client.search(&query, count, need_content).await?;
            output::print_search(&results, max_chars, json)
        }
        Command::Fetch {
            url,
            max_chars,
            json,
        } => {
            let client = api::DeepSeekClient::from_config()?;
            let page = client.fetch(&url).await?;
            output::print_fetch(&page, max_chars, json)
        }
        Command::Config { command } => match command {
            ConfigCommand::SetKey { stdin, from_env } => {
                credentials::set_key(stdin, from_env.as_deref())
            }
            ConfigCommand::Import { dotenv, name } => credentials::import_dotenv(&dotenv, &name),
            ConfigCommand::Show => credentials::show(),
            ConfigCommand::Clear => credentials::clear(),
        },
        Command::Doctor => doctor().await,
    }
}

async fn doctor() -> Result<()> {
    let config = config::Config::load()?;
    println!("config: {}", config::Config::path()?.display());
    println!("credentials: {}", credentials::path()?.display());
    println!("base_url: {}", config.base_url);

    match credentials::load_key() {
        Ok(_) => println!("api key: configured"),
        Err(err) => {
            println!("api key: {err}");
            return Ok(());
        }
    }

    let client = api::DeepSeekClient::from_config()?;
    match client.search("deepseek", 1, false).await {
        Ok(_) => println!("connectivity: ok"),
        Err(err) => println!("connectivity: {err}"),
    }
    Ok(())
}
