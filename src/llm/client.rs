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
- 只输出 JSON 数组，不要输出思考过程、解释、markdown 代码块或其他任何内容\n\
- 数组只能出现一次，不要在思考结束后重复输出\n\n";

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

        match parse_summaries(&raw) {
            Ok(summaries) => {
                info!("Successfully parsed {} summaries from LLM", summaries.len());
                for s in &summaries {
                    debug!("  {} : {:?}", s.repo, s.description);
                }
                Ok(summaries)
            }
            Err(e) => {
                warn!("Failed to parse LLM JSON output: {:#}. Raw: {}", e, raw);
                warn!(
                    "Falling back to {} releases without descriptions",
                    releases.len()
                );
                // Fallback: return entries without descriptions
                Ok(releases
                    .iter()
                    .map(|(repo, _, _)| ReleaseSummary {
                        repo: repo.clone(),
                        description: None,
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

/// Closing tags that reasoning models may emit around their scratchpad. The
/// escaped `<\think` spelling is included because some models (and some
/// templates) print the backslash literally.
const THINK_CLOSE_TAGS: [&str; 2] = ["</think", "<\\think"];

/// Parse the LLM response into release summaries.
///
/// Reasoning models do not always respect "only output JSON": a typical broken
/// response looks like a fenced array, then `</think>`, then the same array
/// again. Slicing from the first `[` to the last `]` would swallow the prose in
/// between and fail with "trailing characters". Instead we look at the text
/// after the last reasoning terminator first, then at every *balanced* JSON
/// array, and finally salvage complete objects from a truncated array.
fn parse_summaries(raw: &str) -> anyhow::Result<Vec<ReleaseSummary>> {
    for candidate in json_candidates(raw) {
        if let Ok(summaries) = serde_json::from_str::<Vec<ReleaseSummary>>(&candidate)
            && !summaries.is_empty()
        {
            debug!(
                "Parsed {} summaries from {} char JSON candidate",
                summaries.len(),
                candidate.len()
            );
            return Ok(summaries);
        }
    }

    // Truncated or otherwise malformed array: keep every complete object we can
    // parse instead of throwing the whole batch away.
    let salvaged = salvage_summaries(raw);
    if !salvaged.is_empty() {
        warn!(
            "Salvaged {} summaries from malformed LLM JSON output",
            salvaged.len()
        );
        return Ok(salvaged);
    }

    Err(anyhow::anyhow!("no parseable JSON summary array found"))
}

/// Candidate JSON array slices, most promising first.
fn json_candidates(raw: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();

    // The real answer normally follows the reasoning terminator.
    if let Some(idx) = last_think_close(raw) {
        let tail = raw[idx..].trim();
        if !tail.is_empty() {
            candidates.push(tail.to_string());
        }
    }

    // Balanced arrays in order of appearance. This also strips markdown code
    // fences and any prose the model wrapped around the JSON.
    candidates.extend(balanced_arrays(raw));

    // Last resort: the whole trimmed response (e.g. a bare JSON object).
    let trimmed = raw.trim();
    if !trimmed.is_empty() && !candidates.iter().any(|c| c == trimmed) {
        candidates.push(trimmed.to_string());
    }

    candidates
}

/// Return the byte index just past the last reasoning closing tag, if any.
fn last_think_close(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    THINK_CLOSE_TAGS
        .iter()
        .filter_map(|tag| lower.rfind(tag).map(|idx| (idx, *tag)))
        .max_by_key(|(idx, _)| *idx)
        .and_then(|(idx, tag)| {
            lower[idx + tag.len()..]
                .find('>')
                .map(|gt| idx + tag.len() + gt + 1)
        })
}

/// Every balanced top-level JSON array in `text`, in order of appearance.
/// Unbalanced (truncated) arrays and brackets inside strings are handled.
fn balanced_arrays(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut arrays = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'[' {
            match matching_bracket(text, i, b'[', b']') {
                Some(end) => {
                    arrays.push(text[i..=end].to_string());
                    i = end + 1;
                    continue;
                }
                None => break, // truncated array, nothing balanced follows
            }
        }
        i += 1;
    }

    arrays
}

/// Recover summaries from a truncated/malformed array by parsing each complete
/// top-level `{...}` object independently.
fn salvage_summaries(text: &str) -> Vec<ReleaseSummary> {
    let bytes = text.as_bytes();
    let mut summaries = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'{' {
            match matching_bracket(text, i, b'{', b'}') {
                Some(end) => {
                    if let Ok(summary) = serde_json::from_str::<ReleaseSummary>(&text[i..=end]) {
                        summaries.push(summary);
                    }
                    i = end + 1;
                    continue;
                }
                None => break,
            }
        }
        i += 1;
    }

    summaries
}

/// Given the byte index of an opening bracket, return the byte index of its
/// match, ignoring brackets that appear inside JSON strings.
fn matching_bracket(text: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &b) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }

        if b == b'"' {
            in_string = true;
        } else if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(start + offset);
            }
        }
    }

    None
}

fn system_tokens_approx() -> usize {
    // Rough estimate, actual counted in batch_by_tokens
    200
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos(summaries: &[ReleaseSummary]) -> Vec<&str> {
        summaries.iter().map(|s| s.repo.as_str()).collect()
    }

    #[test]
    fn parses_plain_json_array() {
        let raw = r#"[{"repo": "a/b", "description": "修复 bug"}]"#;
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(repos(&summaries), vec!["a/b"]);
        assert_eq!(summaries[0].description.as_deref(), Some("修复 bug"));
    }

    #[test]
    fn parses_fenced_json_array() {
        let raw = "```json\n[{\"repo\": \"a/b\", \"description\": null}]\n```";
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(repos(&summaries), vec!["a/b"]);
        assert!(summaries[0].description.is_none());
    }

    #[test]
    fn parses_duplicated_array_after_think_tag() {
        // Reproduces the reported failure: the model emitted the array inside a
        // code block, then `</think>`, then repeated the array. Slicing from the
        // first `[` to the last `]` used to fail with "trailing characters".
        let raw = "\n\n[\n  {\"repo\": \"AnYiEE/touhou\", \"description\": \"发布 v2.10.3 版本\"},\n  {\"repo\": \"wushuo894/ani-rss\", \"description\": \"修复安全问题\"}\n]\n```\nEnd of thought process.\n</think>\n\n[\n  {\"repo\": \"AnYiEE/touhou\", \"description\": \"发布 v2.10.3 版本\"},\n  {\"repo\": \"wushuo894/ani-rss\", \"description\": \"修复安全问题\"}\n]\n";
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(
            repos(&summaries),
            vec!["AnYiEE/touhou", "wushuo894/ani-rss"]
        );
        assert_eq!(summaries[1].description.as_deref(), Some("修复安全问题"));
    }

    #[test]
    fn parses_answer_following_escaped_think_tag() {
        let raw = "<\\think>\nreasoning about [stuff]\n<\\think>\n[{\"repo\": \"a/b\", \"description\": \"x\"}]";
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(repos(&summaries), vec!["a/b"]);
    }

    #[test]
    fn ignores_prose_brackets_before_json() {
        let raw = "Here are the tags [v1, v2] and the result:\n[{\"repo\": \"a/b\", \"description\": \"x\"}]";
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(repos(&summaries), vec!["a/b"]);
    }

    #[test]
    fn salvages_objects_from_truncated_array() {
        let raw = "[{\"repo\": \"a/b\", \"description\": \"first\"}, {\"repo\": \"c/d\", \"descrip";
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(repos(&summaries), vec!["a/b"]);
    }

    #[test]
    fn keeps_brackets_inside_strings() {
        let raw = r#"[{"repo": "a/b", "description": "支持 [x] 与 {y} 语法"}]"#;
        let summaries = parse_summaries(raw).unwrap();
        assert_eq!(
            summaries[0].description.as_deref(),
            Some("支持 [x] 与 {y} 语法")
        );
    }

    #[test]
    fn errors_on_unparseable_output() {
        assert!(parse_summaries("I could not summarize these releases.").is_err());
        assert!(parse_summaries("").is_err());
    }
}
