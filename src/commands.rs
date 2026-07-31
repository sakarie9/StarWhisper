use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use frankenstein::AsyncTelegramApi;
use frankenstein::methods::{GetUpdatesParams, SetMyCommandsParams};
use frankenstein::types::BotCommand;
use frankenstein::updates::UpdateContent;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::repository::Database;
use crate::github::client::GitHubClient;
use crate::llm::client::LlmClient;
use crate::scheduler;
use crate::scheduler::CheckProgress;
use crate::telegram::notifier::TelegramNotifier;

pub async fn run_bot(
    config: Arc<Config>,
    db: Arc<Database>,
    github: GitHubClient,
    telegram: TelegramNotifier,
    llm: LlmClient,
) -> Result<()> {
    let bot = telegram.bot().clone();
    let chat_id: i64 = config.telegram.chat_id.parse().unwrap_or(0);

    // Register bot commands
    info!("Registering bot commands...");
    let commands = vec![
        BotCommand::builder()
            .command("check")
            .description("手动检查新 release")
            .build(),
        BotCommand::builder()
            .command("list")
            .description("列出已跟踪的仓库")
            .build(),
        BotCommand::builder()
            .command("blacklist")
            .description("管理仓库黑名单")
            .build(),
        BotCommand::builder()
            .command("status")
            .description("查看 bot 状态")
            .build(),
        BotCommand::builder()
            .command("help")
            .description("显示帮助信息")
            .build(),
    ];
    let params = SetMyCommandsParams::builder().commands(commands).build();
    match bot.set_my_commands(&params).await {
        Ok(_) => info!("Bot commands registered successfully"),
        Err(e) => error!("Failed to set bot commands: {:#}", e),
    }

    let mut offset: Option<i64> = None;
    info!(
        "Bot command handler started, polling for updates (chat_id: {})",
        chat_id
    );

    loop {
        let params = GetUpdatesParams::builder()
            .offset(offset.unwrap_or(0))
            .timeout(10)
            .build();

        let updates = match bot.get_updates(&params).await {
            Ok(resp) => resp.result,
            Err(e) => {
                error!("Failed to get Telegram updates: {:#}", e);
                debug!("Retrying in 5 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        if !updates.is_empty() {
            debug!("Received {} update(s)", updates.len());
        }

        for update in updates {
            offset = Some(update.update_id as i64 + 1);

            if let UpdateContent::Message(msg) = update.content {
                if msg.chat.id != chat_id {
                    debug!("Ignoring message from unauthorized chat {}", msg.chat.id);
                    continue;
                }

                let text = match &msg.text {
                    Some(t) => t.clone(),
                    None => continue,
                };

                let cmd = text.split_whitespace().next().unwrap_or("");
                let args: Vec<&str> = text.split_whitespace().collect();

                info!("Received command: {} (args: {:?})", cmd, &args[1..]);

                let response = match cmd {
                    "/start" | "/help" => {
                        debug!("Executing help command");
                        Some(cmd_help())
                    }
                    "/check" => {
                        info!("Executing /check command");
                        cmd_check(&config, &db, &github, &telegram, &llm).await
                    }
                    "/list" => {
                        info!("Executing /list command");
                        Some(cmd_list(&db).await)
                    }
                    "/blacklist" => {
                        info!("Executing /blacklist command (args: {:?})", &args[1..]);
                        Some(cmd_blacklist(&db, &args).await)
                    }
                    "/status" => {
                        info!("Executing /status command");
                        Some(cmd_status(&config, &db).await)
                    }
                    _ => {
                        debug!("Unknown command: {}", cmd);
                        continue;
                    }
                };

                if let Some(response) = response {
                    debug!("Sending response for {} ({} chars)", cmd, response.len());
                    if let Err(e) = telegram.send_markdown(&response).await {
                        error!("Failed to send command response for {}: {}", cmd, e);
                    }
                }
            }
        }
    }
}

fn cmd_help() -> String {
    "🤖 **StarWhisper Bot**\n\n\
     **可用命令：**\n\n\
     /check — 手动检查新 release\n\n\
     /list — 列出已跟踪的仓库及最新版本\n\n\
     /blacklist — 管理仓库黑名单\n\n\
     `  `/blacklist add owner/repo — 添加到黑名单\n\n\
     `  `/blacklist remove owner/repo — 从黑名单移除\n\n\
     `  `/blacklist list — 查看黑名单\n\n\
     /status — 查看 bot 运行状态\n\n\
     /help — 显示此帮助信息"
        .to_string()
}

/// Progress reporter that edits a single plain-text Telegram message in place.
struct CheckProgressMessage<'a> {
    telegram: &'a TelegramNotifier,
    message_id: i64,
}

impl CheckProgress for CheckProgressMessage<'_> {
    fn report<'a>(&'a self, text: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Err(e) = self.telegram.edit_plain(self.message_id, text).await {
                error!("Failed to update check progress message: {}", e);
            }
        })
    }
}

/// Run a manual check, reporting progress by editing a single plain-text message.
///
/// Returns `None` on success — the final status is edited into the progress
/// message itself. Returns `Some(text)` only when a separate error message
/// needs to be sent by the caller (the progress message is left untouched).
async fn cmd_check(
    config: &Config,
    db: &Database,
    github: &GitHubClient,
    telegram: &TelegramNotifier,
    llm: &LlmClient,
) -> Option<String> {
    debug!("Sending check progress message");
    let message_id = match telegram.send_plain("⏳ 正在检查更新...").await {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to send check progress message: {}", e);
            return Some(format!("❌ 无法发送进度消息：{}", e));
        }
    };

    let progress = CheckProgressMessage {
        telegram,
        message_id,
    };

    debug!("Running manual check");
    match scheduler::run_check(config, db, github, telegram, llm, &progress).await {
        Ok(()) => {
            info!("Manual check completed successfully");
            None
        }
        Err(e) => {
            error!("Manual check failed: {:#}", e);
            Some(format!("❌ 检查失败：{}", e))
        }
    }
}

async fn cmd_list(db: &Database) -> String {
    let repos = match db.get_all_tracked_repos().await {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to get tracked repos: {}", e);
            return format!("❌ 获取仓库列表失败：{}", e);
        }
    };

    debug!("Listing {} tracked repos", repos.len());

    if repos.is_empty() {
        return "📭 暂无跟踪的仓库。".to_string();
    }

    let mut md = format!("📦 **已跟踪 {} 个仓库**\n\n---", repos.len());

    for repo in &repos {
        let latest = db.get_latest_release(repo.id).await.ok().flatten();
        match latest {
            Some(rel) => {
                md.push_str(&format!(
                    "\n\n🔗 [{}]({})\n最新：`{}`",
                    repo.full_name, repo.html_url, rel.tag_name
                ));
            }
            None => {
                md.push_str(&format!(
                    "\n\n🔗 [{}]({})\n_无 release_",
                    repo.full_name, repo.html_url,
                ));
            }
        }
    }

    md
}

async fn cmd_status(config: &Config, db: &Database) -> String {
    debug!("Gathering bot status");
    let repos = db.get_all_tracked_repos().await.unwrap_or_default();
    let releases = db.get_all_releases_count().await.unwrap_or(0);
    let unnotified = db.get_unnotified_releases().await.unwrap_or_default();
    let blacklist = db.get_blacklist().await.unwrap_or_default();

    let last_checked = db.get_check_completed_at().await.ok().flatten();

    debug!(
        "Status: {} repos, {} releases, {} unnotified, {} blacklisted, last_checked={:?}",
        repos.len(),
        releases,
        unnotified.len(),
        blacklist.len(),
        last_checked
    );

    let last_checked_str = match last_checked {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "从未".to_string(),
    };

    format!(
        "📊 **Bot 状态**\n\n---\n\n\
         📦 跟踪仓库数：**{}**\n\n\
         📝 已记录 release：**{}**\n\n\
         🔔 待通知：**{}**\n\n\
         🚫 黑名单：**{}** 个仓库\n\n\
         ⏰ 定时任务：`{}`\n\n\
         🔔 通知模式：`{}`\n\n\
         🚀 首次启动通知：`{}`\n\n\
         🕐 最近检查：`{}`",
        repos.len(),
        releases,
        unnotified.len(),
        blacklist.len(),
        config.schedule.cron,
        config.schedule.notify_mode,
        if config.schedule.notify_on_first_run {
            "是"
        } else {
            "否"
        },
        last_checked_str
    )
}

async fn cmd_blacklist(db: &Database, args: &[&str]) -> String {
    if args.len() < 2 {
        debug!("Blacklist command without subcommand");
        return "❓ 用法：/blacklist add|remove|list owner/repo".to_string();
    }

    match args[1] {
        "add" => {
            if args.len() < 3 {
                return "❓ 用法：/blacklist add owner/repo".to_string();
            }
            let repo_name = args[2];
            info!("Adding {} to blacklist", repo_name);
            match db.add_to_blacklist(repo_name).await {
                Ok(true) => {
                    info!("Added {} to blacklist", repo_name);
                    format!("✅ 已添加到黑名单：`{}`", repo_name)
                }
                Ok(false) => {
                    debug!("{} already in blacklist", repo_name);
                    format!("⚠️ 已在黑名单中：`{}`", repo_name)
                }
                Err(e) => {
                    error!("Failed to add {} to blacklist: {}", repo_name, e);
                    format!("❌ 添加失败：{}", e)
                }
            }
        }
        "remove" => {
            if args.len() < 3 {
                return "❓ 用法：/blacklist remove owner/repo".to_string();
            }
            let repo_name = args[2];
            info!("Removing {} from blacklist", repo_name);
            match db.remove_from_blacklist(repo_name).await {
                Ok(true) => {
                    info!("Removed {} from blacklist", repo_name);
                    format!("✅ 已从黑名单移除：`{}`", repo_name)
                }
                Ok(false) => {
                    debug!("{} not in blacklist", repo_name);
                    format!("⚠️ 不在黑名单中：`{}`", repo_name)
                }
                Err(e) => {
                    error!("Failed to remove {} from blacklist: {}", repo_name, e);
                    format!("❌ 移除失败：{}", e)
                }
            }
        }
        "list" => {
            debug!("Listing blacklist");
            let blacklist = match db.get_blacklist().await {
                Ok(b) => b,
                Err(e) => {
                    error!("Failed to get blacklist: {}", e);
                    return format!("❌ 获取黑名单失败：{}", e);
                }
            };

            if blacklist.is_empty() {
                return "📭 黑名单为空。".to_string();
            }

            debug!("Blacklist contains {} entries", blacklist.len());
            let mut md = format!("🚫 **黑名单 ({} 个仓库)**\n\n---", blacklist.len());
            for repo in &blacklist {
                md.push_str(&format!("\n• `{}`", repo));
            }
            md
        }
        other => {
            warn!("Unknown blacklist subcommand: {}", other);
            "❓ 未知子命令。可用：add, remove, list".to_string()
        }
    }
}
