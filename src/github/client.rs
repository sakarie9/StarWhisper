use crate::github::{Release, Repository};
use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    base_url: String,
}

impl GitHubClient {
    pub fn new(token: String) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("starwhisper/0.1.0"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: "https://api.github.com".to_string(),
        }
    }

    /// Parse the `Link` header and return the URL for `rel="next"`, if any.
    fn next_page_url(headers: &reqwest::header::HeaderMap) -> Option<String> {
        let link = headers.get("link")?.to_str().ok()?;
        for part in link.split(',') {
            let part = part.trim();
            // Format: <url>; rel="next"
            if part.contains(r#"rel="next""#)
                && let Some(url) = part.split('<').nth(1)
                && let Some(url) = url.split('>').next()
            {
                return Some(url.to_string());
            }
        }
        None
    }

    async fn get_paginated<T: DeserializeOwned>(&self, url: &str) -> Result<Vec<T>> {
        let mut all_items: Vec<T> = Vec::new();
        let mut next_url = Some(url.to_string());

        while let Some(current_url) = next_url {
            debug!("GitHub API GET (paginated) {}", current_url);
            let response = self.client.get(&current_url).send().await?;

            // Check rate limit headers
            if let Some(remaining) = response.headers().get("x-ratelimit-remaining")
                && let Ok(remaining_str) = remaining.to_str()
                && let Ok(remaining_num) = remaining_str.parse::<i32>()
            {
                trace!("GitHub API rate limit remaining: {}", remaining_num);
                if remaining_num < 100 {
                    warn!("GitHub API rate limit low: {} remaining", remaining_num);
                }
                if remaining_num < 10 {
                    let reset = response
                        .headers()
                        .get("x-ratelimit-reset")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok());
                    warn!(
                        "GitHub API rate limit critically low ({}), reset: {:?}",
                        remaining_num, reset
                    );
                }
            }
            let status = response.status();
            if !status.is_success() {
                let text = response.text().await?;
                warn!("GitHub API error {}: {}", status, text);
                return Err(anyhow::anyhow!("GitHub API error {}: {}", status, text));
            }

            next_url = Self::next_page_url(response.headers());
            if next_url.is_some() {
                debug!("Found next page link");
            }

            let items = response.json::<Vec<T>>().await?;
            all_items.extend(items);
        }

        Ok(all_items)
    }

    pub async fn get_starred_repos(&self, username: &str) -> Result<Vec<Repository>> {
        let url = format!("{}/users/{}/starred?per_page=100", self.base_url, username);
        info!("Fetching all starred repos for user '{}'", username);
        let repos: Vec<Repository> = self.get_paginated(&url).await?;
        info!("Fetched {} starred repos for '{}'", repos.len(), username);
        Ok(repos)
    }

    pub async fn get_latest_release(&self, owner: &str, repo: &str) -> Result<Release> {
        let url = format!("{}/repos/{}/{}/releases/latest", self.base_url, owner, repo);
        trace!("Fetching latest release for {}/{}", owner, repo);

        let response = self.client.get(&url).send().await?;

        if let Some(remaining) = response.headers().get("x-ratelimit-remaining")
            && let Ok(remaining_str) = remaining.to_str()
            && let Ok(remaining_num) = remaining_str.parse::<i32>()
        {
            trace!("GitHub API rate limit remaining: {}", remaining_num);
            if remaining_num < 100 {
                warn!("GitHub API rate limit low: {} remaining", remaining_num);
            }
        }
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            debug!("{}/{} has no releases (404)", owner, repo);
            return Err(anyhow::anyhow!("{}/{}: no releases found", owner, repo));
        }
        if !status.is_success() {
            let text = response.text().await?;
            warn!(
                "GitHub API error for {}/{}: {} {}",
                owner, repo, status, text
            );
            return Err(anyhow::anyhow!(
                "GitHub API error for {}/{}: {} {}",
                owner,
                repo,
                status,
                text
            ));
        }

        let release = response.json::<Release>().await?;
        trace!("Got release {} for {}/{}", release.tag_name, owner, repo);
        Ok(release)
    }
}
