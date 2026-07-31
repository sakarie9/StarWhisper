use frankenstein::AsyncTelegramApi;
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{EditMessageTextParams, SendMessageParams, SendRichMessageParams};
use frankenstein::rich_message::InputRichMessage;
use frankenstein::types::ChatId;
use tracing::{debug, error, info, warn};

use crate::llm::client::ReleaseSummary;

#[derive(Clone)]
pub struct TelegramNotifier {
    bot: Bot,
    chat_id: i64,
}

impl TelegramNotifier {
    pub fn new(bot_token: String, chat_id_str: String) -> Self {
        let bot = Bot::new(&bot_token);
        let chat_id: i64 = chat_id_str.parse().unwrap_or_else(|_| {
            warn!("Invalid chat_id '{}', defaulting to 0", chat_id_str);
            0
        });
        info!("Telegram notifier created for chat_id {}", chat_id);
        Self { bot, chat_id }
    }

    pub fn bot(&self) -> &Bot {
        &self.bot
    }

    pub async fn send_markdown(&self, markdown: &str) -> anyhow::Result<()> {
        debug!(
            "Sending markdown message ({} chars) to chat {}",
            markdown.len(),
            self.chat_id
        );
        let params = SendRichMessageParams::builder()
            .chat_id(ChatId::Integer(self.chat_id))
            .rich_message(InputRichMessage {
                html: None,
                markdown: Some(markdown.to_string()),
                is_rtl: None,
                skip_entity_detection: None,
            })
            .build();

        match self.bot.send_rich_message(&params).await {
            Ok(_) => {
                debug!("Message sent successfully");
                Ok(())
            }
            Err(e) => {
                error!("Failed to send Telegram message: {}", e);
                Err(e.into())
            }
        }
    }

    /// Send a plain text message (no formatting) and return its message id.
    ///
    /// Used for the `/check` progress message, which is updated in place via
    /// [`Self::edit_plain`] instead of sending multiple messages.
    pub async fn send_plain(&self, text: &str) -> anyhow::Result<i64> {
        debug!(
            "Sending plain message ({} chars) to chat {}",
            text.len(),
            self.chat_id
        );
        let params = SendMessageParams::builder()
            .chat_id(ChatId::Integer(self.chat_id))
            .text(text.to_string())
            .build();

        match self.bot.send_message(&params).await {
            Ok(resp) => {
                let message_id = resp.result.message_id as i64;
                debug!("Plain message sent (message_id: {})", message_id);
                Ok(message_id)
            }
            Err(e) => {
                error!("Failed to send Telegram plain message: {}", e);
                Err(e.into())
            }
        }
    }

    /// Edit a plain text message (no formatting), used for progress updates.
    pub async fn edit_plain(&self, message_id: i64, text: &str) -> anyhow::Result<()> {
        debug!("Editing message {} ({} chars)", message_id, text.len());
        let params = EditMessageTextParams::builder()
            .chat_id(ChatId::Integer(self.chat_id))
            .message_id(message_id as i32)
            .text(text.to_string())
            .build();

        match self.bot.edit_message_text(&params).await {
            Ok(_) => {
                debug!("Message {} edited successfully", message_id);
                Ok(())
            }
            Err(e) => {
                error!("Failed to edit Telegram message {}: {}", message_id, e);
                Err(e.into())
            }
        }
    }

    fn format_version(old_tag: &Option<String>, new_tag: &str) -> String {
        match old_tag {
            Some(old) if old != new_tag => {
                debug!("Version changed: {} -> {}", old, new_tag);
                format!("`{}` → `{}`", old, new_tag)
            }
            _ => format!("`{}`", new_tag),
        }
    }

    pub async fn send_release_notification(
        &self,
        repo_name: &str,
        old_tag: &Option<String>,
        release_tag: &str,
        url: &str,
        body: &Option<String>,
    ) -> anyhow::Result<()> {
        info!(
            "Sending immediate notification: {} {}",
            repo_name, release_tag
        );
        let version = Self::format_version(old_tag, release_tag);
        let mut md = format!("🆕 [{}]({})\n{}", repo_name, url, version);

        // Add collapsible changelog if body exists
        if let Some(body) = body {
            let trimmed = body.trim();
            if !trimmed.is_empty() {
                let display_body = if trimmed.len() > 2000 {
                    warn!(
                        "Release body for {} truncated ({} chars > 2000)",
                        repo_name,
                        trimmed.len()
                    );
                    format!("{}...", &trimmed[..2000])
                } else {
                    trimmed.to_string()
                };
                debug!(
                    "Including changelog for {} ({} chars)",
                    repo_name,
                    display_body.len()
                );
                md.push_str(&format!(
                    "\n\n<details><summary>更新日志</summary>{}</details>",
                    display_body.replace("\n", "<br/>")
                ));
            } else {
                debug!("No changelog body for {}", repo_name);
            }
        } else {
            debug!("No release body for {}", repo_name);
        }

        self.send_markdown(&md).await
    }

    pub async fn send_summary_notification(
        &self,
        date: &str,
        releases_info: &[(String, String, String, Option<String>)],
        summaries: &[ReleaseSummary],
    ) -> anyhow::Result<()> {
        info!(
            "Sending summary notification: {} releases, date {}",
            summaries.len(),
            date
        );
        let mut md = format!("📋 **Release 更新 · {}**\n\n---\n\n", date);

        for summary in summaries {
            let info = releases_info
                .iter()
                .find(|(r, _, _, _)| r == &summary.repo || summary.repo.contains(r.as_str()));

            let url = info.map(|(_, _, u, _)| u.as_str()).unwrap_or("");
            let new_tag = info.map(|(_, t, _, _)| t.as_str()).unwrap_or("");
            let old_tag = info.and_then(|(_, _, _, o)| o.as_ref());

            if url.is_empty() {
                warn!("No URL found for repo '{}', skipping link", summary.repo);
            }

            let version = Self::format_version(&old_tag.cloned(), new_tag);

            md.push_str("\n\n");
            if url.is_empty() {
                md.push_str(&format!("🔗 **{}** · {}", summary.repo, version));
            } else {
                md.push_str(&format!("🔗 [{}]({}) · {}", summary.repo, url, version));
            }

            if let Some(desc) = &summary.description {
                if !desc.is_empty() {
                    debug!("Summary for {}: {}", summary.repo, desc);
                    md.push_str(&format!("\n\n{}", desc));
                } else {
                    debug!("Empty description for {}", summary.repo);
                }
            } else {
                debug!("No description for {}", summary.repo);
            }
        }

        debug!("Summary message: {} chars total", md.len());
        self.send_markdown(&md).await
    }
}
