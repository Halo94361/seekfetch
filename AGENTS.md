# seekfetch

Unofficial MCP server and CLI for DeepSeek Search/Fetch, written in Rust.

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

`.github/workflows/release.yml` builds Linux and Windows GNU binaries and creates the GitHub release.

## Layout

- `src/cli.rs` — clap command surface
- `src/config.rs` — `config.toml` loading and platform paths
- `src/credentials.rs` — encrypted credential file (machine-bound key)
- `src/api.rs` — DeepSeek Search/Fetch client, retry and error mapping
- `src/mcp.rs` — stdio MCP server exposing `deepseek_search` / `deepseek_fetch`
- `src/output.rs` — human-readable output and character truncation

## Rules

- Never print an API key, pass it through argv, or commit it.
- Credentials live in the platform config directory, never in this repository.
- `DEEPSEEK_API_KEY` overrides the stored credential.
- Keep MCP tool output truncated by default; a search result should not dump raw API JSON into the model context.
- `origin` is the local Gitea remote; `github` is the public GitHub remote. Push to both.
