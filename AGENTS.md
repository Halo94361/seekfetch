# seekfetch-rs

Unofficial MCP server and CLI for DeepSeek Search/Fetch, written in Rust. The repository and Cargo package are named `seekfetch-rs`; the binary remains `seekfetch`.

## Build and test

```bash
cargo fmt --check
cargo test
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu
```

## Release

```bash
git tag v0.1.0
git push origin v0.1.0
git push github v0.1.0
```

`.github/workflows/release.yml` builds Linux and Windows GNU binaries and uploads them as plain files to the GitHub release (no archives).

## Layout

- `src/main.rs` — entry point, dispatch and exit-code mapping (kept thin)
- `src/cli.rs` — clap command surface
- `src/commands.rs` — CLI subcommand implementations (search/fetch batch/balance/cache/usage/config/doctor)
- `src/config.rs` — `config.toml` loading and platform paths
- `src/credentials.rs` — encrypted credential file (machine-bound key)
- `src/api.rs` — DeepSeek Search/Fetch/Balance client, retry, typed errors and error mapping
- `src/cache.rs` — local response cache (24h TTL default), keyword-inspectable entries
- `src/usage.rs` — local usage log and cost estimate
- `src/mcp.rs` — stdio MCP server exposing `deepseek_search` / `deepseek_fetch` / `deepseek_balance`
- `src/output.rs` — output rendering, truncation and `--output` file writing

Keep files split by functional module; do not let any single file grow unwieldy.

## Rules

- Never print an API key, pass it through argv, or commit it.
- Credentials live in the platform config directory, never in this repository.
- `DEEPSEEK_API_KEY` overrides the stored credential.
- Keep MCP tool output truncated by default; a search result should not dump raw API JSON into the model context.
- Never put credentials in source, config, tests, comments, docs, examples, logs, or commit messages. Runtime secrets come from environment variables or the encrypted credential file only.
- Never put personal information in code, docs, examples, or commit metadata: real names, personal email addresses, phone numbers, home addresses, local usernames, or absolute paths such as `/home/<user>`.
- Use placeholders in examples: `YOUR_API_KEY`, `/path/to/seekfetch`, `<user>`.
- Use a GitHub noreply address for commit authorship. Before pushing, scan the working tree and full history for secrets and personal paths.
- `origin` is the local Gitea remote; `github` is the public GitHub remote. Push to both.
