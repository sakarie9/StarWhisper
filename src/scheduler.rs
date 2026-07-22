use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{debug, error, info, trace, warn};

use crate::config::Config;
use crate::db::ReleaseRecord;
use crate::db::repository::Database;
use crate::github::client::GitHubClient;
use crate::github::{Release, Repository};
use crate::llm::client::{LlmClient, ReleaseSummary};
use crate::telegram::notifier::TelegramNotifier;

struct NewReleaseInfo {
    repo_id: i64,
    repo_full_name: String,
    old_release_tag: Option<String>,
    release_tag: String,
    release_url: String,
    release_body: Option<String>,
}

/// Groups all dependencies needed by the cron job, so each invocation only
/// clones a single `Arc` instead of five separate values.
struct CheckContext {
    config: Arc<Config>,
    db: Arc<Database>,
    github: GitHubClient,
    telegram: TelegramNotifier,
    llm: LlmClient,
}

// ─── Scheduler ────────────────────────────────────────────────────────────────

pub async fn run_scheduler(
    config: Arc<Config>,
    db: Arc<Database>,
    github: GitHubClient,
    telegram: TelegramNotifier,
    llm: LlmClient,
) -> Result<()> {
    info!(
        "Initializing scheduler with cron: '{}'",
        config.schedule.cron
    );
    let scheduler = JobScheduler::new().await?;
    let cron = config.schedule.cron.clone();

    let ctx = Arc::new(CheckContext {
        config,
        db,
        github,
        telegram,
        llm,
    });

    let job = Job::new_async_tz(&cron, chrono::Local, move |_uuid, _l| {
        let ctx = ctx.clone();
        Box::pin(async move {
            info!("Scheduled check triggered");
            if let Err(e) =
                run_check(&ctx.config, &ctx.db, &ctx.github, &ctx.telegram, &ctx.llm).await
            {
                error!("Scheduled check failed: {:#}", e);
            }
        })
    })
    .map_err(|e| anyhow::anyhow!("Failed to create cron job: {}", e))?;

    scheduler.add(job).await?;
    info!("Scheduler job registered, starting scheduler");
    scheduler.start().await?;
    info!("Scheduler started, waiting for next trigger");

    std::future::pending::<()>().await;
    Ok(())
}

// ─── Public entry point ───────────────────────────────────────────────────────

pub async fn run_check(
    config: &Config,
    db: &Database,
    github: &GitHubClient,
    telegram: &TelegramNotifier,
    llm: &LlmClient,
) -> Result<()> {
    let is_first_run = !db.has_tracked_repos().await.unwrap_or(false);
    let skip_notification = is_first_run && !config.schedule.notify_on_first_run;

    if skip_notification {
        info!("First run detected, skipping notifications (notify_on_first_run=false)");
    }
    debug!(
        "Starting check: is_first_run={}, skip_notification={}",
        is_first_run, skip_notification
    );

    let new_releases = discover_new_releases(config, db, github).await?;
    let _ = db.set_check_completed_at().await;

    if new_releases.is_empty() {
        info!("Check complete: no new releases found");
        return Ok(());
    }

    info!("Check complete: {} new releases found", new_releases.len());

    if skip_notification {
        info!(
            "Skipping notification for {} new releases (first run)",
            new_releases.len()
        );
        mark_all_notified(db, &new_releases).await?;
        return Ok(());
    }

    send_notifications(config, db, telegram, llm, &new_releases).await
}

// ─── Release discovery ────────────────────────────────────────────────────────

/// Scan all starred repos across every configured user and collect new releases.
async fn discover_new_releases(
    config: &Config,
    db: &Database,
    github: &GitHubClient,
) -> Result<Vec<NewReleaseInfo>> {
    let mut new_releases = Vec::new();

    for username in &config.github.usernames {
        debug!("Fetching starred repos for user '{}'", username);
        let repos = github.get_starred_repos(username).await?;
        info!(
            "Found {} starred repos for user '{}'",
            repos.len(),
            username
        );

        prune_unstarred_repos(db, &repos).await?;

        let total = repos.len();
        for (i, repo) in repos.iter().enumerate() {
            if (i + 1) % 50 == 0 {
                info!(
                    "Progress: {}/{} repos checked for user '{}'",
                    i + 1,
                    total,
                    username
                );
            }

            if db.is_blacklisted(&repo.full_name).await.unwrap_or(false) {
                debug!("Skipping blacklisted repo: {}", repo.full_name);
                continue;
            }

            debug!("Checking repo: {}", repo.full_name);
            if let Some(release) = check_single_repo(db, github, repo).await? {
                info!(
                    "New release found: {} - {}",
                    release.repo_full_name, release.release_tag
                );
                new_releases.push(release);
            }
        }

        info!("Finished checking {} repos for user '{}'", total, username);
    }

    // Deduplicate by repo_id: monorepos (e.g. switch-tools) may have multiple
    // "latest" releases returned by GitHub in quick succession, causing the same
    // repo to appear more than once in new_releases.
    let before = new_releases.len();
    let mut seen: HashMap<i64, NewReleaseInfo> = HashMap::new();
    for release in new_releases {
        seen.insert(release.repo_id, release);
    }
    let new_releases: Vec<NewReleaseInfo> = seen.into_values().collect();
    let dupes = before.saturating_sub(new_releases.len());
    if dupes > 0 {
        warn!("Deduplicated {} duplicate release entries", dupes);
    }

    Ok(new_releases)
}

/// Remove repos that the user has un-starred since the last check.
async fn prune_unstarred_repos(db: &Database, repos: &[Repository]) -> Result<()> {
    debug!("Checking for unstarred repos to remove");
    let starred_names: Vec<String> = repos.iter().map(|r| r.full_name.clone()).collect();
    let removed = db.remove_unstarred_repos(&starred_names).await?;
    if removed.is_empty() {
        debug!("No unstarred repos to remove");
    } else {
        info!("Removed {} unstarred repos: {:?}", removed.len(), removed);
    }
    Ok(())
}

/// Check a single repository for a new release.
///
/// - **First encounter**: saves the release as a baseline (marked notified immediately).
/// - **No change**: optionally back-fills a missing body, returns `None`.
/// - **New release**: saves it and returns `Some(NewReleaseInfo)`.
async fn check_single_repo(
    db: &Database,
    github: &GitHubClient,
    repo: &Repository,
) -> Result<Option<NewReleaseInfo>> {
    let tracked = db
        .get_or_create_repo(repo.id as i64, &repo.full_name, &repo.html_url)
        .await?;

    let (owner, repo_name) = match repo.full_name.split_once('/') {
        Some(parts) => parts,
        None => {
            warn!(
                "Invalid repo name format: '{}', expected 'owner/repo'",
                repo.full_name
            );
            return Ok(None);
        }
    };

    let latest_release = match github.get_latest_release(owner, repo_name).await {
        Ok(release) => release,
        Err(e) => {
            debug!("No release for {}: {}", repo.full_name, e);
            return Ok(None);
        }
    };

    // Record that we successfully queried this repo
    let _ = db.update_last_checked(tracked.id).await;

    let existing = db.get_latest_release(tracked.id).await?;

    // First time seeing this repo — save as baseline, no notification.
    let Some(stored) = existing else {
        debug!(
            "First release tracked for {}: {}, saving as baseline",
            repo.full_name, latest_release.tag_name
        );
        let release_id = db.save_release(tracked.id, &latest_release).await?;
        db.mark_release_notified(release_id).await?;
        return Ok(None);
    };

    // Tag unchanged — nothing to do (optionally refresh a missing body).
    if stored.tag_name == latest_release.tag_name {
        trace!(
            "No change for {}: still at {}",
            repo.full_name, stored.tag_name
        );
        refresh_body_if_missing(db, tracked.id, &stored, &latest_release).await;
        return Ok(None);
    }

    // New release detected.
    debug!(
        "New version for {}: {} -> {}",
        repo.full_name, stored.tag_name, latest_release.tag_name
    );
    let old_tag = stored.tag_name;
    db.save_release(tracked.id, &latest_release).await?;

    Ok(Some(NewReleaseInfo {
        repo_id: tracked.id,
        repo_full_name: repo.full_name.clone(),
        old_release_tag: Some(old_tag),
        release_tag: latest_release.tag_name,
        release_url: latest_release.html_url,
        release_body: latest_release.body,
    }))
}

/// Back-fill the release body if the stored record had an empty one.
async fn refresh_body_if_missing(
    db: &Database,
    repo_id: i64,
    stored: &ReleaseRecord,
    fresh: &Release,
) {
    if !stored.body.as_deref().unwrap_or("").is_empty() {
        return;
    }
    if let Ok(true) = db
        .update_release_body(repo_id, &stored.tag_name, &fresh.body)
        .await
    {
        info!(
            "Refreshed missing body for repo_id={} {}",
            repo_id, stored.tag_name
        );
    }
}

// ─── Notifications ────────────────────────────────────────────────────────────

/// Dispatch notifications based on the configured `notify_mode`.
///
/// Each handler is responsible for marking releases as notified after a
/// successful send so that failed batches can be retried on the next check.
async fn send_notifications(
    config: &Config,
    db: &Database,
    telegram: &TelegramNotifier,
    llm: &LlmClient,
    releases: &[NewReleaseInfo],
) -> Result<()> {
    match config.schedule.notify_mode.as_str() {
        "immediate" => notify_immediate(db, telegram, releases).await,
        "summary" => notify_summary(db, telegram, llm, releases).await,
        other => {
            warn!("Unknown notify_mode '{}', skipping notifications", other);
            Ok(())
        }
    }
}

/// Send one Telegram message per release; mark each as notified after success.
async fn notify_immediate(
    db: &Database,
    telegram: &TelegramNotifier,
    releases: &[NewReleaseInfo],
) -> Result<()> {
    info!(
        "Sending immediate notifications for {} releases",
        releases.len()
    );

    for release in releases {
        debug!(
            "Notifying {} - {}",
            release.repo_full_name, release.release_tag
        );
        telegram
            .send_release_notification(
                &release.repo_full_name,
                &release.old_release_tag,
                &release.release_tag,
                &release.release_url,
                &release.release_body,
            )
            .await?;

        mark_repo_notified(db, release).await?;
    }

    info!("All immediate notifications sent");
    Ok(())
}

/// Build an LLM summary and send a single Telegram digest; mark all as notified.
async fn notify_summary(
    db: &Database,
    telegram: &TelegramNotifier,
    llm: &LlmClient,
    releases: &[NewReleaseInfo],
) -> Result<()> {
    info!(
        "Sending summary notification for {} releases",
        releases.len()
    );

    let summaries = request_llm_summaries(llm, releases).await;

    let date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let releases_info: Vec<(String, String, String, Option<String>)> = releases
        .iter()
        .map(|r| {
            (
                r.repo_full_name.clone(),
                r.release_tag.clone(),
                r.release_url.clone(),
                r.old_release_tag.clone(),
            )
        })
        .collect();

    telegram
        .send_summary_notification(&date, &releases_info, &summaries)
        .await?;
    info!("Summary notification sent successfully");

    for release in releases {
        mark_repo_notified(db, release).await?;
    }
    Ok(())
}

/// Request LLM summaries, back-filling any releases the LLM missed.
async fn request_llm_summaries(
    llm: &LlmClient,
    releases: &[NewReleaseInfo],
) -> Vec<ReleaseSummary> {
    let releases_for_llm: Vec<(String, String, Option<String>)> = releases
        .iter()
        .map(|r| {
            (
                r.repo_full_name.clone(),
                r.release_tag.clone(),
                r.release_body.clone(),
            )
        })
        .collect();

    info!("Requesting LLM summary for {} releases...", releases.len());
    let mut summaries = match llm.summarize(&releases_for_llm).await {
        Ok(s) => {
            info!("LLM returned {} summaries", s.len());
            s
        }
        Err(e) => {
            error!("Failed to get summary from LLM: {:#}", e);
            vec![]
        }
    };

    let original_count = summaries.len();
    for release in releases {
        let already_present = summaries.iter().any(|s| s.repo == release.repo_full_name);
        if !already_present {
            warn!(
                "LLM missed release {} {}, adding without description",
                release.repo_full_name, release.release_tag
            );
            summaries.push(ReleaseSummary {
                repo: release.repo_full_name.clone(),
                description: None,
            });
        }
    }
    if summaries.len() != original_count {
        debug!(
            "Added {} missing releases to summaries",
            summaries.len() - original_count
        );
    }

    summaries
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Mark every release in the batch as notified (used for skip / summary paths).
async fn mark_all_notified(db: &Database, releases: &[NewReleaseInfo]) -> Result<()> {
    for release in releases {
        mark_repo_notified(db, release).await?;
    }
    Ok(())
}

/// Look up the latest stored release for a repo and mark it as notified.
async fn mark_repo_notified(db: &Database, release: &NewReleaseInfo) -> Result<()> {
    if let Some(latest) = db.get_latest_release(release.repo_id).await? {
        db.mark_release_notified(latest.id).await?;
        debug!(
            "Marked release {} ({} {}) as notified",
            latest.id, release.repo_full_name, release.release_tag
        );
    }
    Ok(())
}
