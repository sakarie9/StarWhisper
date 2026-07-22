use chrono::{DateTime, Utc};
use sqlx::FromRow;

pub mod repository;

#[derive(Debug, Clone, FromRow)]
pub struct TrackedRepo {
    pub id: i64,
    pub github_id: i64,
    pub full_name: String,
    pub html_url: String,
    pub last_checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ReleaseRecord {
    pub id: i64,
    pub repo_id: i64,
    pub release_id: i64,
    pub tag_name: String,
    pub previous_tag_name: Option<String>,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: DateTime<Utc>,
    pub notified_at: Option<DateTime<Utc>>,
    pub html_url: String,
    pub prerelease: bool,
}
