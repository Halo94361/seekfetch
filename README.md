# seekfetch-rs

**English** | [简体中文](README.zh-CN.md)

Unofficial MCP server and CLI for the [DeepSeek Search/Fetch API](https://api-docs.deepseek.com/zh-cn/guides/search), packaged as a single Rust binary. The repository and Cargo package are named `seekfetch-rs`; the installed binary remains `seekfetch`.

The CLI, encrypted credential storage and the MCP server are fully implemented. `seekfetch doctor` verifies configuration, credentials and live API connectivity.

## Why

DeepSeek Search is currently driven by curl and prompt instructions. This wrapper keeps the API key out of prompts and command lines, turns the response into structured results, and exposes the same capability as an MCP tool with output truncation built in.

Unofficial tool.

## Tools

| Tool | Input | Output |
| --- | --- | --- |
| `deepseek_search` | `query`, `count`, `need_content`, `max_chars`, `min_rerank`, `min_authority`, `domain`, `no_cache`, `refresh` | `title`, `url`, `summary`, optional `content`, `rerank_score`, `authority` |
| `deepseek_fetch` | `url` or `urls` (batch, max 20), `max_chars`, `no_cache`, `refresh` | `title`, `url`, `content`, `truncated`; batch returns one entry per URL, failures inline |
| `deepseek_balance` | — | `is_available`, `balance_infos` (`currency`, `total_balance`, `granted_balance`, `topped_up_balance`) |

Known server-side extraction behavior (observed, not configurable): HTML tables are flattened into pipe-joined plain-text lines instead of Markdown tables, and content inside inactive tabs or otherwise hidden sections may be omitted entirely.

## Install

Linux x86_64, installs `seekfetch` into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Halo94361/seekfetch-rs/main/install.sh | sh
```

Set `PREFIX=/usr/local/bin` to choose another directory. Prebuilt Windows binaries and older releases are attached to [GitHub Releases](https://github.com/Halo94361/seekfetch-rs/releases):

- `seekfetch-vX.Y.Z-linux-x86_64` (Linux)
- `seekfetch-vX.Y.Z-windows-x86_64.exe` (Windows)

Build from source instead:

```bash
git clone https://github.com/Halo94361/seekfetch-rs && cargo install --path ./seekfetch-rs --locked
```

For a Windows GNU build from Linux, add `--target x86_64-pc-windows-gnu` to `cargo build --release`.

Shell completions are generated at runtime:

```bash
seekfetch completions bash > ~/.local/share/bash-completion/completions/seekfetch
# also: zsh, fish, powershell, elvish
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
seekfetch search "site:github.com rust" --json
seekfetch search "deepseek" --min-rerank 1.5 --min-authority 3 --domain deepseek.com
echo "rust mcp server" | seekfetch search            # query from stdin
seekfetch fetch "https://example.com" --max-chars 20000
seekfetch fetch url1 url2 url3                        # batch: pages joined by ---
seekfetch fetch --input urls.txt --output pages.md    # one URL per line, # comments ok
cat urls.txt | seekfetch fetch                        # batch from stdin
seekfetch balance
seekfetch usage                                       # call counts and estimated cost
seekfetch cache list [keyword]                        # inspect cached requests
seekfetch doctor
```

`seekfetch balance` reads the account balance from `GET /user/balance`:

```
available: yes
CNY
  total balance:     93.80
  granted balance:   0.00
  topped-up balance: 93.80
```

Compatible gateways that do not implement the balance endpoint answer 404.

### Batch fetch

`fetch` accepts any number of URLs (arguments, `--input` file, or piped stdin). Pages are fetched sequentially and joined with `---` separators; a failed URL does not abort the batch — its error goes to stderr, `--json` marks it as `{"url": ..., "error": ...}`, and the exit code is 9 when anything failed.

### Cache

Search and fetch responses are cached locally (default 24h TTL) to save billed API calls and speed up repeats. The cache key uses a normalized query (trimmed, lowercased, whitespace collapsed), so trivially different spellings still hit. Entries store the original request, so the cache is inspectable by keyword:

```bash
seekfetch cache stats                 # entries, size, expiry
seekfetch cache list                  # all cached requests, newest first
seekfetch cache list deepseek         # keyword filter (case-insensitive)
seekfetch cache show deepseek         # dump a matching entry's payload
seekfetch cache clear                 # delete all entries
seekfetch cache path                  # print the cache directory
seekfetch search "..." --no-cache     # bypass the cache for one call
seekfetch search "..." --refresh      # force a fresh call and update the cached entry
```

Default location: `~/.cache/seekfetch` (Linux), `%LOCALAPPDATA%\seekfetch` (Windows). Cache problems never fail a request — a broken entry is just a miss.

### Usage statistics

Every search/fetch/balance call (CLI and MCP) is appended to `~/.local/share/seekfetch/usage.jsonl` (Linux). `seekfetch usage` aggregates the log: call counts, failures, cache hits (free), billable calls and the estimated cost at ¥5 per 1000 successful calls. Balance queries and cache hits are not billed; failed calls are not billed.

`--since YYYY-MM-DD` / `--until YYYY-MM-DD` select a UTC date range (both ends inclusive); `--last 30d` selects a relative window (`d`, `w`, `m`, `y`; `m` = 30 days, `y` = 365 days). `--group-by day|week|month` reports per-bucket counts; weeks start on Monday. Buckets with no calls are omitted. With `--group-by`, `--json` returns an array of buckets instead of a single summary.

```bash
seekfetch usage --last 30d
seekfetch usage --since 2026-09-01 --until 2026-09-30 --group-by day
seekfetch usage --last 6m --group-by month --json
```

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | other error (config, credentials, I/O) |
| 2 | CLI usage error (clap) |
| 3 | network unreachable |
| 4 | timeout |
| 5 | authentication / rejected (401, 403) |
| 6 | insufficient balance (402) |
| 7 | rate limited (429) |
| 8 | other API error (400, 404, 5xx, decode) |
| 9 | batch fetch partially failed |

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

Replace `~/.env` with whichever dotenv file holds your key. `DEEPSEEK_API_KEY` in the environment overrides the stored credential. The key is never printed; `config show` reports only a masked value, and API error messages are scrubbed of key material before display. `config path` prints the configuration and credential file locations.

**Threat model:** the file is encrypted with a key derived from the machine id and user name, so copying it to another machine does not reveal the secret. It does not protect against a process running as the same user (or root) that deliberately decrypts it.

## Configuration

`~/.config/seekfetch/config.toml` (optional; shown with defaults):

```toml
base_url = "https://api.deepseek.com"
timeout_s = 35
default_count = 10
default_max_chars = 300
min_interval_ms = 200   # process-wide API throttle; 0 disables

[cache]
enabled = true
ttl_s = 86400
# dir = "/path/to/cache"   # default: platform cache dir

[usage]
enabled = true
```

`min_interval_ms` sets the minimum gap between API requests in the current process (default 200ms, i.e. 5 QPS). Cache hits are not throttled. Set it to 0 to disable the throttle.

`seekfetch config show` prints the effective configuration with the masked key and its source.

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
  main.rs         entry point, dispatch and exit-code mapping
  cli.rs          clap definitions
  commands.rs     CLI subcommand implementations
  config.rs       config.toml loading and platform paths
  credentials.rs  encrypted credential storage
  api.rs          DeepSeek Search/Fetch/Balance client, retry, typed errors
  cache.rs        local response cache with keyword inspection
  usage.rs        local usage log and cost estimate
  mcp.rs          stdio MCP server
  output.rs       output rendering, truncation and file writing
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
