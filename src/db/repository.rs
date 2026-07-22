use crate::db::{ReleaseRecord, TrackedRepo};
use crate::github::Release;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use tracing::{debug, info, trace};

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn connect(database_path: &str) -> Result<Self> {
        // sqlx 0.8 SQLite connection string format
        let connection_string = if database_path == ":memory:" {
            "sqlite::memory:".to_string()
        } else {
            // Use sqlite: prefix with ?mode=rwc to auto-create the file
            format!("sqlite:{}?mode=rwc", database_path)
        };

        info!("Connecting to database: {}", connection_string);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        info!("Database connected successfully");

        // Create tables manually
        debug!("Creating tables if not exist...");
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tracked_repos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                github_id INTEGER NOT NULL UNIQUE,
                full_name TEXT NOT NULL,
                html_url TEXT NOT NULL,
                last_checked_at DATETIME
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS releases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id INTEGER NOT NULL REFERENCES tracked_repos(id) UNIQUE,
                release_id INTEGER NOT NULL,
                tag_name TEXT NOT NULL,
                previous_tag_name TEXT,
                name TEXT,
                body TEXT,
                published_at DATETIME NOT NULL,
                notified_at DATETIME,
                html_url TEXT NOT NULL,
                prerelease BOOLEAN DEFAULT FALSE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS blacklist (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                full_name TEXT NOT NULL UNIQUE,
                added_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        debug!("Database tables initialized");
        Ok(Self { pool })
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get_or_create_repo(
        &self,
        github_id: i64,
        full_name: &str,
        html_url: &str,
    ) -> Result<TrackedRepo> {
        trace!(
            "get_or_create_repo: github_id={}, full_name={}",
            github_id, full_name
        );
        let repo = sqlx::query_as::<_, TrackedRepo>(
            "SELECT id, github_id, full_name, html_url, last_checked_at FROM tracked_repos WHERE github_id = ?",
        )
        .bind(github_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(repo) = repo {
            trace!("Found existing repo: id={}, {}", repo.id, repo.full_name);
            return Ok(repo);
        }

        debug!("Creating new tracked repo: {}", full_name);
        let row = sqlx::query(
            "INSERT INTO tracked_repos (github_id, full_name, html_url) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(github_id)
        .bind(full_name)
        .bind(html_url)
        .fetch_one(&self.pool)
        .await?;

        let id: i64 = row.get(0);
        debug!("Created repo with id={}", id);

        let repo = sqlx::query_as::<_, TrackedRepo>(
            "SELECT id, github_id, full_name, html_url, last_checked_at FROM tracked_repos WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(repo)
    }

    pub async fn get_latest_release(&self, repo_id: i64) -> Result<Option<ReleaseRecord>> {
        trace!("get_latest_release: repo_id={}", repo_id);
        let release = sqlx::query_as::<_, ReleaseRecord>(
            "SELECT id, repo_id, release_id, tag_name, previous_tag_name, name, body, published_at, notified_at, html_url, prerelease FROM releases WHERE repo_id = ?",
        )
        .bind(repo_id)
        .fetch_optional(&self.pool)
        .await?;

        trace!(
            "get_latest_release result: {:?}",
            release.as_ref().map(|r| &r.tag_name)
        );
        Ok(release)
    }

    pub async fn save_release(&self, repo_id: i64, release: &Release) -> Result<i64> {
        debug!(
            "Saving release: repo_id={}, tag={}",
            repo_id, release.tag_name
        );
        // Read the current tag before upserting
        let previous_tag: Option<String> =
            sqlx::query_scalar("SELECT tag_name FROM releases WHERE repo_id = ?")
                .bind(repo_id)
                .fetch_optional(&self.pool)
                .await?;

        let row = sqlx::query(
            "INSERT INTO releases (repo_id, release_id, tag_name, previous_tag_name, name, body, published_at, html_url, prerelease) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(repo_id) DO UPDATE SET release_id = excluded.release_id, tag_name = excluded.tag_name, previous_tag_name = excluded.previous_tag_name, name = excluded.name, body = excluded.body, published_at = excluded.published_at, html_url = excluded.html_url, prerelease = excluded.prerelease, notified_at = NULL RETURNING id",
        )
        .bind(repo_id)
        .bind(release.id as i64)
        .bind(&release.tag_name)
        .bind(&previous_tag)
        .bind(&release.name)
        .bind(&release.body)
        .bind(release.published_at.unwrap_or_else(Utc::now))
        .bind(&release.html_url)
        .bind(release.prerelease)
        .fetch_one(&self.pool)
        .await?;

        let id: i64 = row.get(0);
        debug!("Saved release with id={}", id);
        Ok(id)
    }

    pub async fn update_release_body(
        &self,
        repo_id: i64,
        tag_name: &str,
        body: &Option<String>,
    ) -> Result<bool> {
        if let Some(body) = body
            && !body.is_empty()
        {
            trace!(
                "Updating release body for repo_id={}, tag={}",
                repo_id, tag_name
            );
            let result = sqlx::query(
                    "UPDATE releases SET body = ? WHERE repo_id = ? AND tag_name = ? AND (body IS NULL OR body = '')",
                )
                .bind(body)
                .bind(repo_id)
                .bind(tag_name)
                .execute(&self.pool)
                .await?;

            let updated = result.rows_affected() > 0;
            if updated {
                debug!(
                    "Updated missing release body for repo_id={}, tag={}",
                    repo_id, tag_name
                );
            }
            return Ok(updated);
        }
        Ok(false)
    }

    pub async fn mark_release_notified(&self, release_id: i64) -> Result<()> {
        trace!("mark_release_notified: release_id={}", release_id);
        sqlx::query("UPDATE releases SET notified_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(release_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_last_checked(&self, repo_id: i64) -> Result<()> {
        trace!("update_last_checked: repo_id={}", repo_id);
        sqlx::query("UPDATE tracked_repos SET last_checked_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_check_completed_at(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO metadata (key, value) VALUES ('last_check_completed_at', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(&now)
        .execute(&self.pool)
        .await?;
        debug!("Recorded check completion at {}", now);
        Ok(())
    }

    pub async fn get_check_completed_at(&self) -> Result<Option<DateTime<Utc>>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM metadata WHERE key = 'last_check_completed_at'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(v,)| {
            DateTime::parse_from_rfc3339(&v)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }))
    }

    pub async fn get_unnotified_releases(&self) -> Result<Vec<ReleaseRecord>> {
        debug!("Fetching unnotified releases");
        let releases = sqlx::query_as::<_, ReleaseRecord>(
            "SELECT id, repo_id, release_id, tag_name, previous_tag_name, name, body, published_at, notified_at, html_url, prerelease FROM releases WHERE notified_at IS NULL ORDER BY published_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        debug!("Found {} unnotified releases", releases.len());
        Ok(releases)
    }

    pub async fn get_releases_for_repo(&self, repo_id: i64) -> Result<Vec<ReleaseRecord>> {
        trace!("get_releases_for_repo: repo_id={}", repo_id);
        let releases = sqlx::query_as::<_, ReleaseRecord>(
            "SELECT id, repo_id, release_id, tag_name, previous_tag_name, name, body, published_at, notified_at, html_url, prerelease FROM releases WHERE repo_id = ? ORDER BY published_at DESC LIMIT 2",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;

        trace!("Found {} releases for repo_id={}", releases.len(), repo_id);
        Ok(releases)
    }

    pub async fn get_all_tracked_repos(&self) -> Result<Vec<TrackedRepo>> {
        debug!("Fetching all tracked repos");
        let repos = sqlx::query_as::<_, TrackedRepo>(
            "SELECT id, github_id, full_name, html_url, last_checked_at FROM tracked_repos ORDER BY full_name",
        )
        .fetch_all(&self.pool)
        .await?;

        debug!("Found {} tracked repos", repos.len());
        Ok(repos)
    }

    pub async fn get_all_releases_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM releases")
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.get(0);
        trace!("Total releases count: {}", count);
        Ok(count)
    }

    pub async fn has_tracked_repos(&self) -> Result<bool> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM tracked_repos")
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.get(0);
        trace!("has_tracked_repos: count={}, result={}", count, count > 0);
        Ok(count > 0)
    }

    // Blacklist methods
    pub async fn add_to_blacklist(&self, full_name: &str) -> Result<bool> {
        debug!("Adding to blacklist: {}", full_name);
        let result = sqlx::query("INSERT OR IGNORE INTO blacklist (full_name) VALUES (?)")
            .bind(full_name)
            .execute(&self.pool)
            .await?;

        let added = result.rows_affected() > 0;
        if added {
            info!("Added to blacklist: {}", full_name);
        } else {
            debug!("Already in blacklist: {}", full_name);
        }
        Ok(added)
    }

    pub async fn remove_from_blacklist(&self, full_name: &str) -> Result<bool> {
        debug!("Removing from blacklist: {}", full_name);
        let result = sqlx::query("DELETE FROM blacklist WHERE full_name = ?")
            .bind(full_name)
            .execute(&self.pool)
            .await?;

        let removed = result.rows_affected() > 0;
        if removed {
            info!("Removed from blacklist: {}", full_name);
        } else {
            debug!("Not in blacklist: {}", full_name);
        }
        Ok(removed)
    }

    pub async fn is_blacklisted(&self, full_name: &str) -> Result<bool> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM blacklist WHERE full_name = ?")
            .bind(full_name)
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.get(0);
        trace!("is_blacklisted({}): {}", full_name, count > 0);
        Ok(count > 0)
    }

    pub async fn get_blacklist(&self) -> Result<Vec<String>> {
        debug!("Fetching blacklist");
        let rows = sqlx::query("SELECT full_name FROM blacklist ORDER BY full_name")
            .fetch_all(&self.pool)
            .await?;

        let blacklist: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        debug!("Blacklist contains {} entries", blacklist.len());
        Ok(blacklist)
    }

    pub async fn remove_unstarred_repos(&self, starred_names: &[String]) -> Result<Vec<String>> {
        debug!(
            "Checking for unstarred repos ({} starred)",
            starred_names.len()
        );
        let all_tracked = self.get_all_tracked_repos().await?;
        let mut removed = Vec::new();

        for repo in all_tracked {
            if !starred_names.contains(&repo.full_name) {
                debug!("Removing unstarred repo: {}", repo.full_name);
                // Delete releases first (foreign key)
                sqlx::query("DELETE FROM releases WHERE repo_id = ?")
                    .bind(repo.id)
                    .execute(&self.pool)
                    .await?;
                // Delete repo
                sqlx::query("DELETE FROM tracked_repos WHERE id = ?")
                    .bind(repo.id)
                    .execute(&self.pool)
                    .await?;
                removed.push(repo.full_name);
            }
        }

        if !removed.is_empty() {
            info!("Removed {} unstarred repos", removed.len());
        } else {
            debug!("No unstarred repos to remove");
        }
        Ok(removed)
    }
}
