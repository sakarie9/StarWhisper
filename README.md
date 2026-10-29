# StarWhisper

English | [中文](./README_CN.md)

**StarWhisper** is a Telegram bot that monitors starred repositories on GitHub and sends notifications when new releases are detected. It supports two notification modes — immediate per-release alerts and AI-powered daily digests.

---

## Features

- ⭐ **Star-based Tracking** — Automatically tracks all repositories starred by configured GitHub users
- 🆕 **Release Detection** — Periodically checks for new releases via the GitHub API
- 🤖 **AI Summaries** — Optionally summarizes releases using OpenAI-compatible or Claude API (daily digest mode)
- 📦 **Two Notification Modes**:
  - `immediate` — One Telegram message per new release
  - `summary` — A single daily digest with AI-generated release descriptions
- 🚫 **Blacklist** — Exclude specific repositories from notifications
- 📊 **Status Command** — View bot health, tracked repos, and last check time via Telegram
- 🗄️ **SQLite Storage** — Lightweight local database, no external dependencies
- ⏰ **Cron Scheduling** — Configurable check interval

---

## Table of Contents

- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Usage](#usage)
- [Bot Commands](#bot-commands)
- [Building from Source](#building-from-source)
- [Docker](#docker)

---

## Quick Start

```bash
# Clone the repository
git clone https://github.com/yourusername/starwhisper.git
cd starwhisper

# Copy and edit the configuration
cp config.toml.example config.toml
# Fill in your GitHub token, Telegram bot token, etc.

# Run
cargo run
```

> **Note:** `config.toml` is in `.gitignore` and will not be committed. Use `config.toml.example` as a reference.

---

## Configuration

All configuration lives in `config.toml`. See `config.toml.example` for a complete reference.

```toml
[github]
token = "ghp_xxxx"                    # GitHub personal access token
usernames = ["octocat", "torvalds"]   # GitHub users to watch

[telegram]
bot_token = "123456:ABC..."           # Telegram bot token (from @BotFather)
chat_id = "-1001234567890"            # Target chat/group ID

[llm]
provider = "openai"                   # "openai" | "claude"
api_key = "sk-xxx"                    # API key
model = "gpt-4o"                      # Model name
api_base = "https://api.openai.com/v1"  # Optional API base URL
max_tokens = 65536                    # Optional, context window size
language = "English"                  # Optional, summary language (default "English", e.g. "中文")

[database]
path = "data.db"                      # SQLite database path

[schedule]
cron = "0 0 8 * * *"                 # Cron expression (sec min hour day month weekday)
notify_mode = "summary"              # "immediate" | "summary"
notify_on_first_run = false          # Notify on first cold start
```

### Notification Modes

| Mode | Description |
| --- | --- |
| `immediate` | Sends a separate Telegram message for each new release with full changelog |
| `summary` | Aggregates all new releases into a single daily digest with AI-generated summaries |

---

## Usage

### First Run

On first startup with an empty database, StarWhisper automatically performs an initial check to populate the release history without sending notifications. Subsequent scheduled checks will only alert you about **new** releases.

### Scheduled Checks

The bot checks for new releases according to the cron expression in `config.toml`. By default it runs daily at 8:00 AM.

### Bot Commands

| Command | Description |
| --- | --- |
| `/check` | Manually trigger a release check (shows live progress) |
| `/list` | List all tracked repositories with latest versions |
| `/blacklist add owner/repo` | Add a repository to the blacklist |
| `/blacklist remove owner/repo` | Remove a repository from the blacklist |
| `/blacklist list` | Show all blacklisted repositories |
| `/status` | Display bot status and statistics |
| `/help` | Show help message |

---

## Building from Source

### Prerequisites

- Rust 1.85 or later (edition 2024)
- SQLite (no system package required — uses `sqlx` with bundled SQLite)

### Build & Test

```bash
# Build (debug)
cargo build

# Build (release)
cargo build --release

# Run tests
cargo test

# Run with verbose logging
RUST_LOG=debug cargo run

# Run with trace logging
RUST_LOG=trace cargo run
```

---

## Docker

Pre-built Docker images are available on GitHub Container Registry:

```bash
# Pull the image
docker pull ghcr.io/sakarie9/starwhisper:latest

# Run with a config file
# The image reads STARWHISPER_CONFIG (default: /etc/starwhisper/config.toml)
# and uses the database.path from the config (relative paths are relative to WORKDIR /data)
docker run -d \
  --name starwhisper \
  -e TZ=Asia/Shanghai \
  -v /path/to/config.toml:/etc/starwhisper/config.toml:ro \
  -v starwhisper-data:/data \
  ghcr.io/sakarie9/starwhisper:latest
```

> The container working directory is `/data` — mount a volume there so the database
> file persists. Set `path = "data.db"` in your config (recommended), and it will
> be stored at `/data/data.db` inside the container.
>
> To use a custom config path, set the `STARWHISPER_CONFIG` environment variable.

### Docker Compose

```yaml
services:
  starwhisper:
    image: ghcr.io/sakarie9/starwhisper:latest
    container_name: starwhisper
    restart: unless-stopped
    environment:
      - RUST_LOG=info
      - TZ=Asia/Shanghai
    volumes:
      - /path/to/config.toml:/etc/starwhisper/config.toml:ro
      - starwhisper-data:/data

volumes:
  starwhisper-data:
```
