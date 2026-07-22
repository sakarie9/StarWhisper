use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod client;

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub full_name: String,
    pub html_url: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub html_url: String,
    pub prerelease: bool,
}
