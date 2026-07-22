use anyhow;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tiktoken_rs::cl100k_base;
use tracing::{debug, error, info, trace, warn};

use crate::config::LlmConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseSummary {
    pub repo: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// System prompt template (constant part)
const SYSTEM_PROMPT: &str = "你是一个精简的 release 更新总结机器人。请用中文总结以下新发布的版本更新。\n\
输出要求：返回一个 JSON 数组，每个元素格式如下：\n\
{\"repo\": \"owner/name\", \"description\": \"一句话概括核心变更\"}\n\
- description 要极度精简，一句话概括核心变更\n\
- 如果 release notes 为空或无意义，description 设为 null\n\
- 如果是重大更新（大版本、breaking change、重要新功能），description 可以稍详细\n\
- **每个输入的 release 都必须出现在输出数组中，不能省略任何一个**\n\
- 只输出 JSON 数组，不要输出任何其他内容\n\n";

/// Max body chars to send to LLM
const MAX_BODY_CHARS: usize = 2000;

/// Reserve tokens for: system prompt + response
const RESERVED_TOKENS: usize = 4096;

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    config: LlmConfig,
    encoder: Arc<tiktoken_rs::CoreBPE>,
}

use std::sync::Arc;

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .unwrap();

        let encoder = cl100k_base().expect("Failed to load tiktoken encoder");

        info!(
            "LLM client created: provider={}, model={}, max_tokens={}",
            config.provider, config.model, config.max_tokens
        );
        Self {
            client,
            config,
            encoder: Arc::new(encoder),
        }
    }

    fn count_tokens(&self, text: &str) -> usize {
        self.encoder.encode_ordinary(text).len()
    }

    /// Estimate tokens for one release entry in the prompt.
    /// Returns a conservative estimate with 1.5x safety margin to avoid overfilling batches.
    fn estimate_release_tokens(&self, repo: &str, tag: &str, body: &Option<String>) -> usize {
        // Header: "## owner/repo (tag)\n\n"
        let header = format!("## {} ({})\n\n", repo, tag);
        let header_tokens = self.count_tokens(&header);

        let body_tokens = match body {
            Some(b) => {
                let trimmed: String = b.chars().take(MAX_BODY_CHARS).collect();
                self.count_tokens(&trimmed) + 1 // trailing \n
            }
            None => 0,
        };

        // 1.5x safety margin to account for tokenizer variance with CJK characters
        ((header_tokens + body_tokens) * 3) / 2
    }

    pub async fn summarize(
        &self,
        releases: &[(String, String, Option<String>)],
    ) -> anyhow::Result<Vec<ReleaseSummary>> {
        let total = releases.len();
        if total == 0 {
            return Ok(vec![]);
        }

        // Filter out releases without body — they don't need LLM summary
        let with_body: Vec<(String, String, Option<String>)> = releases
            .iter()
            .filter(|(_, _, body)| match body {
                Some(b) => !b.trim().is_empty(),
                None => false,
            })
            .cloned()
            .collect();

        let skipped = total - with_body.len();
        if skipped > 0 {
            info!("Skipping {} releases without body (no LLM needed)", skipped);
        }

        if with_body.is_empty() {
            info!("No releases with body, skipping LLM entirely");
            return Ok(releases
                .iter()
                .map(|(repo, _, _)| ReleaseSummary {
                    repo: repo.clone(),
                    description: None,
                })
                .collect());
        }

        // Batch by token count
        let max_prompt_tokens = self.config.max_tokens.saturating_sub(RESERVED_TOKENS);
        let system_tokens = self.count_tokens(SYSTEM_PROMPT);
        let available_tokens = max_prompt_tokens.saturating_sub(system_tokens);

        info!(
            "Generating summary for {} releases ({} with body, {} skipped), max {} tokens per batch",
            total,
            with_body.len(),
            skipped,
            available_tokens
        );

        let batches = self.batch_by_tokens(&with_body, available_tokens);
        info!("Split into {} batches", batches.len());

        let mut all_summaries: Vec<ReleaseSummary> = Vec::new();

        for (i, batch) in batches.iter().enumerate() {
            info!(
                "Processing batch {}/{} ({} releases)",
                i + 1,
                batches.len(),
                batch.len()
            );
            match self.summarize_batch(batch).await {
                Ok(summaries) => {
                    info!(
                        "Batch {}/{}: got {} summaries",
                        i + 1,
                        batches.len(),
                        summaries.len()
                    );
                    all_summaries.extend(summaries);
                }
                Err(e) => {
                    warn!(
                        "Batch {}/{} failed: {:#}, falling back to no descriptions",
                        i + 1,
                        batches.len(),
                        e
                    );
                    for (repo, _, _) in batch.iter() {
                        all_summaries.push(ReleaseSummary {
                            repo: repo.clone(),
                            description: None,
                        });
                    }
                }
            }
        }

        // Add back releases without body
        for (repo, _, _) in releases {
            if !all_summaries.iter().any(|s| s.repo == *repo) {
                all_summaries.push(ReleaseSummary {
                    repo: repo.clone(),
                    description: None,
                });
            }
        }

        info!("Total summaries: {}/{}", all_summaries.len(), total);
        Ok(all_summaries)
    }

    /// Split releases into batches that fit within the token budget.
    fn batch_by_tokens<'a>(
        &self,
        releases: &'a [(String, String, Option<String>)],
        available_tokens: usize,
    ) -> Vec<&'a [(String, String, Option<String>)]> {
        let mut batches = Vec::new();
        let mut start = 0;
        let mut batch_tokens = 0;

        for (i, (repo, tag, body)) in releases.iter().enumerate() {
            let est = self.estimate_release_tokens(repo, tag, body);
            if batch_tokens + est > available_tokens && i > start {
                // Current batch is full, cut before this release
                batches.push(&releases[start..i]);
                start = i;
                batch_tokens = 0;
            }
            batch_tokens += est;
        }
        if start < releases.len() {
            batches.push(&releases[start..]);
        }

        // Log batch stats
        for (i, batch) in batches.iter().enumerate() {
            let tokens: usize = batch
                .iter()
                .map(|(r, t, b)| self.estimate_release_tokens(r, t, b))
                .sum();
            debug!(
                "Batch {}: {} releases, ~{} tokens",
                i + 1,
                batch.len(),
                tokens + system_tokens_approx()
            );
        }

        batches
    }

    async fn summarize_batch(
        &self,
        releases: &[(String, String, Option<String>)],
    ) -> anyhow::Result<Vec<ReleaseSummary>> {
        let mut prompt = String::from(SYSTEM_PROMPT);
        for (repo, tag, body) in releases {
            prompt.push_str(&format!("## {} ({})\n", repo, tag));
            if let Some(b) = body {
                let trimmed: String = b.chars().take(MAX_BODY_CHARS).collect();
                debug!(
                    "Including release body for {} ({} chars, original {} chars)",
                    repo,
                    trimmed.len(),
                    b.len()
                );
                prompt.push_str(&trimmed);
                prompt.push('\n');
            }
            prompt.push('\n');
        }

        let token_count = self.count_tokens(&prompt);
        debug!(
            "LLM prompt: {} chars, ~{} tokens",
            prompt.len(),
            token_count
        );
        trace!("LLM prompt content:\n{}", prompt);

        let raw = match self.config.provider.as_str() {
            "openai" => {
                debug!("Calling OpenAI API (model: {})", self.config.model);
                self.call_openai(&prompt).await?
            }
            "claude" => {
                debug!("Calling Claude API (model: {})", self.config.model);
                self.call_claude(&prompt).await?
            }
            other => {
                error!("Unknown LLM provider: {}", other);
                return Err(anyhow::anyhow!("Unknown LLM provider: {}", other));
            }
        };

        debug!("LLM raw response: {} chars", raw.len());
        trace!("LLM raw response:\n{}", raw);

        // Try to extract JSON array from response (handle markdown code blocks)
        let json_str = extract_json(&raw);
        debug!("Extracted JSON: {} chars", json_str.len());

        match serde_json::from_str::<Vec<ReleaseSummary>>(json_str) {
            Ok(summaries) => {
                info!("Successfully parsed {} summaries from LLM", summaries.len());
                for s in &summaries {
                    debug!("  {} : {:?}", s.repo, s.description);
                }
                Ok(summaries)
            }
            Err(e) => {
                warn!("Failed to parse LLM JSON output: {}. Raw: {}", e, raw);
                // Fallback: return entries without descriptions
                Ok(releases
                    .iter()
                    .map(|(repo, _, _)| {
                        warn!("Fallback: using repo={} without description", repo);
                        ReleaseSummary {
                            repo: repo.clone(),
                            description: None,
                        }
                    })
                    .collect())
            }
        }
    }

    async fn call_openai(&self, prompt: &str) -> anyhow::Result<String> {
        let url = self
            .config
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let endpoint = format!("{}/chat/completions", url);
        debug!("OpenAI request to {}", endpoint);

        let body = json!({
            "model": self.config.model,
            "messages": [{"role": "user", "content": prompt}]
        });

        let response = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        debug!("OpenAI response status: {}", status);

        if !status.is_success() {
            let err_msg = response.text().await?;
            error!("OpenAI API error {}: {}", status, err_msg);
            return Err(anyhow::anyhow!("OpenAI API error {}: {}", status, err_msg));
        }

        let data: Value = response.json().await?;
        let summary = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("Failed to get summary")
            .to_string();

        debug!("OpenAI response received: {} chars", summary.len());
        Ok(summary)
    }

    async fn call_claude(&self, prompt: &str) -> anyhow::Result<String> {
        let url = self
            .config
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());

        let endpoint = format!("{}/messages", url);
        debug!("Claude request to {}", endpoint);

        let body = json!({
            "model": self.config.model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 4096
        });

        let response = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        debug!("Claude response status: {}", status);

        if !status.is_success() {
            let err_msg = response.text().await?;
            error!("Claude API error {}: {}", status, err_msg);
            return Err(anyhow::anyhow!("Claude API error {}: {}", status, err_msg));
        }

        let data: Value = response.json().await?;
        let summary = data["content"][0]["text"]
            .as_str()
            .unwrap_or("Failed to get summary")
            .to_string();

        debug!("Claude response received: {} chars", summary.len());
        Ok(summary)
    }
}

/// Extract JSON array from LLM response, handling markdown code blocks
fn extract_json(raw: &str) -> &str {
    let trimmed = raw.trim();
    // Handle ```json ... ``` code blocks
    if trimmed.starts_with("```") {
        debug!("Stripping markdown code block from LLM response");
        let start = trimmed.find('\n').map(|i| i + 1).unwrap_or(3);
        let end = trimmed.rfind("```").unwrap_or(trimmed.len());
        return trimmed[start..end].trim();
    }
    // Find the first [ and last ] for raw JSON
    if let Some(start) = trimmed.find('[')
        && let Some(end) = trimmed.rfind(']')
    {
        debug!("Extracting JSON array from position {} to {}", start, end);
        return &trimmed[start..=end];
    }
    debug!("No JSON array markers found, using full response");
    trimmed
}

fn system_tokens_approx() -> usize {
    // Rough estimate, actual counted in batch_by_tokens
    200
}
