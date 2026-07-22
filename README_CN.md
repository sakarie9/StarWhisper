# StarWhisper

[English](./README.md) | 中文

**StarWhisper** 是一个 Telegram 机器人，用于监控 GitHub 上已标星（Star）的仓库，并在检测到新 release 时发送通知。支持两种通知模式：即时单条推送和 AI 每日摘要。

---

## 功能特性

- ⭐ **Star 跟踪** — 自动跟踪已配置 GitHub 用户标星的所有仓库
- 🆕 **Release 检测** — 通过 GitHub API 定期检查新 release
- 🤖 **AI 摘要** — 可选地使用 OpenAI 兼容 API 或 Claude API 生成 release 摘要（每日汇总模式）
- 📦 **两种通知模式**：
  - `immediate` — 每条新 release 单独发送一条 Telegram 消息
  - `summary` — 将所有新 release 汇总为一条 AI 生成的每日摘要
- 🚫 **黑名单** — 排除特定仓库，不接收其通知
- 📊 **状态查询** — 通过 Telegram 查看机器人运行状态、跟踪仓库数和最近检查时间
- 🗄️ **SQLite 存储** — 轻量本地数据库，无需外部依赖
- ⏰ **Cron 调度** — 可配置的检查间隔

---

## 目录

- [快速开始](#快速开始)
- [配置说明](#配置说明)
- [使用方法](#使用方法)
- [机器人命令](#机器人命令)
- [编译构建](#编译构建)
- [Docker](#docker)
- [许可证](#许可证)

---

## 快速开始

```bash
# 克隆仓库
git clone https://github.com/yourusername/starwhisper.git
cd starwhisper

# 复制并编辑配置文件
cp config.toml.example config.toml
# 填入你的 GitHub Token、Telegram Bot Token 等

# 运行
cargo run
```

> **注意：** `config.toml` 已在 `.gitignore` 中，不会被提交到仓库。请使用 `config.toml.example` 作为配置参考。

---

## 配置说明

所有配置位于 `config.toml` 文件中。完整参考请见 `config.toml.example`。

```toml
[github]
token = "ghp_xxxx"                    # GitHub 个人访问令牌
usernames = ["octocat", "torvalds"]   # 要监控的 GitHub 用户名

[telegram]
bot_token = "123456:ABC..."           # Telegram Bot Token（从 @BotFather 获取）
chat_id = "-1001234567890"            # 目标聊天/群组 ID

[llm]
provider = "openai"                   # "openai" | "claude"
api_key = "sk-xxx"                    # API 密钥
model = "gpt-4o"                      # 模型名称
api_base = "https://api.openai.com/v1"  # 可选，API 代理地址
max_tokens = 65536                    # 可选，上下文窗口大小

[database]
path = "data.db"                      # SQLite 数据库路径

[schedule]
cron = "0 0 8 * * *"                 # Cron 表达式（秒 分 时 日 月 周）
notify_mode = "summary"              # "immediate" | "summary"
notify_on_first_run = false          # 首次冷启动是否发送通知
```

### 通知模式对比

| 模式 | 说明 |
| --- | --- |
| `immediate` | 每条新 release 单独发送一条 Telegram 消息，包含完整更新日志 |
| `summary` | 将所有新 release 汇总为一条 AI 生成的每日摘要推送 |

---

## 使用方法

### 首次运行

首次启动且数据库为空时，StarWhisper 会自动执行一次初始检查以填充 release 历史记录，**不会**发送通知。之后的定期检查将仅提醒你 **新的** release。

### 定时检查

机器人根据 `config.toml` 中的 cron 表达式定时检查新 release。默认配置为每天上午 8:00 执行。

### 机器人命令

| 命令 | 说明 |
| --- | --- |
| `/check` | 手动触发一次 release 检查 |
| `/list` | 列出所有已跟踪仓库及其最新版本 |
| `/blacklist add owner/repo` | 将仓库加入黑名单 |
| `/blacklist remove owner/repo` | 将仓库移出黑名单 |
| `/blacklist list` | 查看黑名单中的所有仓库 |
| `/status` | 显示机器人运行状态和统计信息 |
| `/help` | 显示帮助信息 |

---

## 编译构建

### 环境要求

- Rust 1.85 或更高版本（edition 2024）
- SQLite（无需系统安装包——使用 `sqlx` 内置 SQLite）

### 构建与测试

```bash
# 构建（调试模式）
cargo build

# 构建（发布模式）
cargo build --release

# 运行测试
cargo test

# 运行（详细日志）
RUST_LOG=debug cargo run

# 运行（跟踪日志）
RUST_LOG=trace cargo run
```

---

## Docker

预构建的 Docker 镜像已发布至 GitHub Container Registry：

```bash
# 拉取镜像
docker pull ghcr.io/sakarie9/starwhisper:latest

# 运行容器
# 镜像默认读取 STARWHISPER_CONFIG 环境变量（默认 /etc/starwhisper/config.toml）
# 数据库路径由配置文件中的 database.path 决定，相对路径相对于 WORKDIR /data
docker run -d \
  --name starwhisper \
  -v /path/to/config.toml:/etc/starwhisper/config.toml:ro \
  -v starwhisper-data:/data \
  ghcr.io/sakarie9/starwhisper:latest
```

> 容器的工作目录是 `/data` —— 请挂载一个 volume 到该目录以持久化数据库文件。
> 建议在配置中设置 `path = "data.db"`，这样数据库会存储在容器内的 `/data/data.db`。
>
> 如需自定义配置文件路径，设置 `STARWHISPER_CONFIG` 环境变量即可。

### Docker Compose

```yaml
services:
  starwhisper:
    image: ghcr.io/sakarie9/starwhisper:latest
    container_name: starwhisper
    restart: unless-stopped
    environment:
      - STARWHISPER_CONFIG=/etc/starwhisper/config.toml
      - RUST_LOG=info
    volumes:
      - /path/to/config.toml:/etc/starwhisper/config.toml:ro
      - starwhisper-data:/data

volumes:
  starwhisper-data:
```

---

## 许可证

本项目采用 MIT 许可证。详见 [LICENSE](LICENSE) 文件。
