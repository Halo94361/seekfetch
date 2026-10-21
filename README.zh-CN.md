# seekfetch-rs

[English](README.md) | **简体中文**

面向 [DeepSeek Search/Fetch API](https://api-docs.deepseek.com/zh-cn/guides/search) 的非官方 MCP 服务器与 CLI，以单个 Rust 二进制分发。仓库和 Cargo 包名为 `seekfetch-rs`，安装后的二进制仍为 `seekfetch`。

CLI、加密凭据存储和 MCP 服务器均已完整实现。`seekfetch doctor` 会检查配置、凭据和 API 连通性。

## 为什么

目前使用 DeepSeek Search 往往依赖 curl 和提示词指令。本项目将 API key 从提示词和命令行中隔离出来，把响应转换为结构化结果，并以 MCP 工具的形式暴露同一能力，同时内置输出截断。

非官方工具。

## 工具

| 工具 | 输入 | 输出 |
| --- | --- | --- |
| `deepseek_search` | `query`, `count`, `need_content`, `max_chars`, `min_rerank`, `min_authority`, `domain`, `no_cache`, `refresh` | `title`, `url`, `summary`，可选 `content`，`rerank_score`, `authority` |
| `deepseek_fetch` | `url` 或 `urls`（批量，最多 20 个），`max_chars`, `no_cache`, `refresh` | `title`, `url`, `content`, `truncated`；批量模式每个 URL 返回一条，失败信息内联 |
| `deepseek_balance` | — | `is_available`, `balance_infos`（`currency`, `total_balance`, `granted_balance`, `topped_up_balance`） |

已知的服务端提取行为（实测，不可配置）：HTML 表格会被压平为以竖线连接的纯文本行，而不是 Markdown 表格；非活动标签页或隐藏区域中的内容可能被完全省略。

## 安装

预编译二进制附在 [GitHub Releases](https://github.com/Halo94361/seekfetch-rs/releases)：

- `seekfetch-vX.Y.Z-linux-x86_64`（Linux）
- `seekfetch-vX.Y.Z-windows-x86_64.exe`（Windows）

或从源码构建：

```bash
cargo build --release
# Linux: target/release/seekfetch
# Windows: target/x86_64-pc-windows-gnu/release/seekfetch.exe
```

Shell 补全在运行时生成：

```bash
seekfetch completions bash > ~/.local/share/bash-completion/completions/seekfetch
# 也支持：zsh、fish、powershell、elvish
```

## MCP 配置

使用构建产物或下载二进制的绝对路径：

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
echo "rust mcp server" | seekfetch search            # 从 stdin 读取查询
seekfetch fetch "https://example.com" --max-chars 20000
seekfetch fetch url1 url2 url3                        # 批量：页面之间用 --- 分隔
seekfetch fetch --input urls.txt --output pages.md    # 每行一个 URL，# 开头为注释
cat urls.txt | seekfetch fetch                        # 从 stdin 批量读取
seekfetch balance
seekfetch usage                                       # 调用次数与预估费用
seekfetch cache list [keyword]                        # 查看缓存请求
seekfetch doctor
```

`seekfetch balance` 通过 `GET /user/balance` 读取账户余额：

```
available: yes
CNY
  total balance:     93.80
  granted balance:   0.00
  topped-up balance: 93.80
```

不实现余额接口的兼容网关会返回 404。

### 批量抓取

`fetch` 接受任意数量的 URL（命令行参数、`--input` 文件或管道 stdin）。页面按顺序抓取，并以 `---` 分隔拼接；单个 URL 失败不会中止整批——错误写入 stderr，`--json` 模式下标记为 `{"url": ..., "error": ...}`，只要有失败，退出码就是 9。

### 缓存

搜索和抓取响应会缓存在本地（默认 TTL 24 小时），以节省计费调用并加速重复请求。缓存键使用归一化后的查询（去首尾空白、转小写、合并连续空白），因此细微的拼写差异仍能命中。条目保存原始请求，可按关键词检查：

```bash
seekfetch cache stats                 # 条目数、大小、过期时间
seekfetch cache list                  # 所有缓存请求，最新在前
seekfetch cache list deepseek         # 关键词过滤（不区分大小写）
seekfetch cache show deepseek         # 输出匹配条目的完整内容
seekfetch cache clear                 # 删除所有条目
seekfetch cache path                  # 打印缓存目录
seekfetch search "..." --no-cache     # 单次调用绕过缓存
seekfetch search "..." --refresh      # 强制重新请求并更新缓存条目
```

默认位置：`~/.cache/seekfetch`（Linux）、`%LOCALAPPDATA%\seekfetch`（Windows）。缓存问题不会导致请求失败——损坏的条目只视为未命中。

### 用量统计

每次 search/fetch/balance 调用（CLI 和 MCP）都会追加到 `~/.local/share/seekfetch/usage.jsonl`（Linux）。`seekfetch usage` 聚合日志：调用次数、失败次数、缓存命中（免费）、计费调用，以及按每 1000 次成功调用 ¥5 估算的费用。余额查询和缓存命中不计费；失败调用不计费。

`--since YYYY-MM-DD` / `--until YYYY-MM-DD` 按 UTC 日期筛选（含首尾）；`--last 30d` 选择相对窗口（支持 `d`、`w`、`m`、`y`，其中 `m` = 30 天，`y` = 365 天）。`--group-by day|week|month` 按天/周/月分桶，周从周一开始；没有调用的桶不会输出。使用 `--group-by` 时，`--json` 返回桶数组而不是单个汇总。

```bash
seekfetch usage --last 30d
seekfetch usage --since 2026-09-01 --until 2026-09-30 --group-by day
seekfetch usage --last 6m --group-by month --json
```

### 退出码

| 退出码 | 含义 |
| --- | --- |
| 0 | 成功 |
| 1 | 其他错误（配置、凭据、I/O） |
| 2 | CLI 用法错误（clap） |
| 3 | 网络不可达 |
| 4 | 超时 |
| 5 | 认证失败或被拒绝（401、403） |
| 6 | 余额不足（402） |
| 7 | 触发限流（429） |
| 8 | 其他 API 错误（400、404、5xx、解码失败） |
| 9 | 批量抓取部分失败 |

## 凭据

API key 保存在平台配置目录下的加密文件中：

- Linux：`~/.config/seekfetch/credentials`
- Windows：`%APPDATA%\seekfetch\credentials`

三种配置方式：

```bash
# 交互式：stdin 是终端时隐藏输入
seekfetch config set-key

# 脚本 / agent：从 stdin 读取
grep -E '^DEEPSEEK_API_KEY=' ~/.env | cut -d= -f2- | seekfetch config set-key --stdin

# 推荐的 agent 方式：从 dotenv 文件导入，密钥不进入 argv
seekfetch config import --dotenv ~/.env --name DEEPSEEK_API_KEY
```

将 `~/.env` 替换为存放密钥的 dotenv 文件。环境变量 `DEEPSEEK_API_KEY` 会覆盖已存储的凭据。密钥永远不会被打印；`config show` 只显示掩码值，API 错误信息在展示前也会清除密钥内容。`config path` 打印配置和凭据文件位置。

**威胁模型：** 文件使用由机器 ID 和用户名派生出的密钥加密，复制到另一台机器无法直接解密。它不能防御以同一用户（或 root）身份运行并刻意解密的进程。

## 配置

`~/.config/seekfetch/config.toml`（可选；以下为默认值）：

```toml
base_url = "https://api.deepseek.com"
timeout_s = 35
default_count = 10
default_max_chars = 300
min_interval_ms = 200   # 进程内 API 节流；0 表示关闭

[cache]
enabled = true
ttl_s = 86400
# dir = "/path/to/cache"   # 默认：平台缓存目录

[usage]
enabled = true
```

`min_interval_ms` 设置当前进程内 API 请求的最小间隔（默认 200ms，即 5 QPS）。缓存命中不触发限流；设为 0 可关闭节流。

`seekfetch config show` 打印生效配置、掩码后的密钥及其来源。

## 开发

```bash
cargo fmt --check
cargo test
cargo build --release
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

CI 在 GitHub Actions 上构建 Linux 和 Windows GNU 产物。

## 发布

创建 tag 并推送到两个 remote：

```bash
git tag v0.1.0
git push origin v0.1.0
git push github v0.1.0
```

`Release` 工作流会构建 Linux 和 Windows GNU 二进制并附加到 GitHub release。Gitea 镜像源码和 tag；release 资产放在 GitHub。

## 目录结构

```
src/
  main.rs         入口、命令分发和退出码映射
  cli.rs          clap 定义
  commands.rs     CLI 子命令实现
  config.rs       config.toml 加载和平台路径
  credentials.rs  加密凭据存储
  api.rs          DeepSeek Search/Fetch/Balance 客户端、重试、类型化错误
  cache.rs        带关键词检查的本地响应缓存
  usage.rs        本地用量日志和费用估算
  mcp.rs          stdio MCP 服务器
  output.rs       输出渲染、截断和文件写入
```

## 许可证

以下两种许可证任选其一：[MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)。
