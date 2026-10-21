mod api;
mod cache;
mod clean;
mod cli;
mod commands;
mod config;
mod credentials;
mod mcp;
mod output;
mod usage;

use clap::Parser;

use cli::{Cli, Command, ConfigCommand};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match dispatch(cli.command).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            exit_code(&err)
        }
    };
    std::process::exit(code);
}

async fn dispatch(command: Command) -> anyhow::Result<i32> {
    match command {
        Command::Mcp => {
            mcp::run().await?;
            Ok(0)
        }
        Command::Search {
            query,
            count,
            need_content,
            max_chars,
            json,
            min_rerank,
            min_authority,
            domain,
            no_cache,
            refresh,
            output,
        } => {
            commands::search(
                query,
                count,
                need_content,
                max_chars,
                json,
                min_rerank,
                min_authority,
                domain,
                no_cache,
                refresh,
                output,
            )
            .await
        }
        Command::Fetch {
            urls,
            input,
            max_chars,
            json,
            no_cache,
            refresh,
            output,
        } => commands::fetch(urls, input, max_chars, json, no_cache, refresh, output).await,
        Command::Balance { json, output } => commands::balance(json, output).await,
        Command::Cache { command } => commands::cache_command(&command),
        Command::Usage {
            json,
            since,
            until,
            last,
            group_by,
        } => commands::usage_command(
            json,
            since.as_deref(),
            until.as_deref(),
            last.as_deref(),
            group_by,
        ),
        Command::Completions { shell } => commands::completions(shell),
        Command::Config { command } => match command {
            ConfigCommand::SetKey { stdin, from_env } => {
                credentials::set_key(stdin, from_env.as_deref())?;
                Ok(0)
            }
            ConfigCommand::Import { dotenv, name } => {
                credentials::import_dotenv(&dotenv, &name)?;
                Ok(0)
            }
            ConfigCommand::Show => commands::show_config(),
            ConfigCommand::Path => commands::config_paths(),
            ConfigCommand::Clear => {
                credentials::clear()?;
                Ok(0)
            }
        },
        Command::Doctor => commands::doctor().await,
    }
}

/// Exit code scheme: 0 ok, 1 other (config, credentials, I/O), 2 clap usage
/// error (clap's own), 3-8 classified API failures, 9 partial batch failure.
fn exit_code(err: &anyhow::Error) -> i32 {
    match err.downcast_ref::<api::ApiError>().map(api::ApiError::kind) {
        Some(api::ErrorKind::Network) => 3,
        Some(api::ErrorKind::Timeout) => 4,
        Some(api::ErrorKind::Auth) => 5,
        Some(api::ErrorKind::Payment) => 6,
        Some(api::ErrorKind::RateLimited) => 7,
        Some(api::ErrorKind::Api) => 8,
        None => 1,
    }
}
