# seekfetch

Unofficial MCP server and CLI for the [DeepSeek Search/Fetch API](https://api-docs.deepseek.com/zh-cn/guides/search), packaged as a single Rust binary.

The CLI, encrypted credential storage and the MCP server are fully implemented. `seekfetch doctor` verifies configuration, credentials and live API connectivity.

## Why

DeepSeek Search is currently driven by curl and prompt instructions. This wrapper keeps the API key out of prompts and command lines, turns the response into structured results, and exposes the same capability as an MCP tool with output truncation built in.

This project is not affiliated with or endorsed by DeepSeek.

## Tools

| Tool | Input | Output |
| --- | --- | --- |
| `deepseek_search` | `query`, `count`, `need_content`, `max_chars` | `title`, `url`, `summary`, optional `content`, `rerank_score`, `authority` |
| `deepseek_fetch` | `url`, `max_chars` | `title`, `url`, `content`, `truncated` |

## Install

Prebuilt binaries are attached to [GitHub Releases](https://github.com/Halo94361/seekfetch/releases):

- `seekfetch-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `seekfetch-vX.Y.Z-x86_64-pc-windows-gnu.zip`

Or build from source:

```bash
cargo build --release
# Linux: target/release/seekfetch
# Windows: target/x86_64-pc-windows-gnu/release/seekfetch.exe
```

## MCP configuration

Use the absolute path to the built or downloaded binary:

```json
{
  "mcpServers": {
    "deepseek": {
      "command": "/path/to/seekfetch",
      "args": ["mcp"],
      "deferred": true
    }
  }
}
```

## CLI

```bash
seekfetch search "rust mcp server" --count 5
seekfetch fetch "https://example.com" --max-chars 20000
seekfetch search "site:github.com rust" --json
seekfetch doctor
```

## Credentials

The API key is stored in an encrypted file under the platform config directory:

- Linux: `~/.config/seekfetch/credentials`
- Windows: `%APPDATA%\seekfetch\credentials`

Three ways to configure it:

```bash
# Interactive: hidden input when stdin is a terminal
seekfetch config set-key

# Script / agent: read from stdin
grep -E '^DEEPSEEK_API_KEY=' ~/.env | cut -d= -f2- | seekfetch config set-key --stdin

# Recommended for agents: import from a dotenv file, key never enters argv
seekfetch config import --dotenv ~/.env --name DEEPSEEK_API_KEY
```

Replace `~/.env` with whichever dotenv file holds your key. `DEEPSEEK_API_KEY` in the environment overrides the stored credential. The key is never printed; `config show` only reports a masked value.

**Threat model:** the file is encrypted with a key derived from the machine id and user name, so copying it to another machine does not reveal the secret. It does not protect against a process running as the same user (or root) that deliberately decrypts it.

## Configuration

`~/.config/seekfetch/config.toml` (optional):

```toml
base_url = "https://api.deepseek.com"
timeout_s = 60
default_count = 10
default_max_chars = 300
```

## Development

```bash
cargo fmt --check
cargo test
cargo build --release
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

CI builds both Linux and Windows GNU artifacts on GitHub Actions.

## Releasing

Create a tag and push it to both remotes:

```bash
git tag v0.1.0
git push origin v0.1.0
git push github v0.1.0
```

The `Release` workflow builds the Linux and Windows GNU binaries and attaches them to the GitHub release. Gitea mirrors the source and tag; release assets live on GitHub.

## Layout

```
src/
  main.rs         CLI dispatch
  cli.rs          clap definitions
  config.rs       config.toml loading and paths
  credentials.rs  encrypted credential storage
  api.rs          DeepSeek Search/Fetch client
  mcp.rs          stdio MCP server
  output.rs       human-readable output and truncation
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
