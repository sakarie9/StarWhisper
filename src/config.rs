use serde::Deserialize;
use tracing::debug;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub github: GitHubConfig,
    pub telegram: TelegramConfig,
    pub llm: LlmConfig,
    pub database: DatabaseConfig,
    pub schedule: ScheduleConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub usernames: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub api_base: Option<String>,
    /// Max context window size in tokens (default 64K)
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
}

fn default_max_tokens() -> usize {
    65536
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScheduleConfig {
    pub cron: String,
    pub notify_mode: String,
    #[serde(default)]
    pub notify_on_first_run: bool,
}

impl Config {
    /// Load configuration from a TOML file.
    ///
    /// The path is determined by (in priority order):
    /// 1. `STARWHISPER_CONFIG` environment variable
    /// 2. `config.toml` (default, in the current working directory)
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("STARWHISPER_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        debug!("Reading config from '{}'", path);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        let config: Config = toml::from_str(&content)?;
        debug!(
            "Config parsed: users={:?}, cron='{}', notify_mode='{}', db_path='{}'",
            config.github.usernames,
            config.schedule.cron,
            config.schedule.notify_mode,
            config.database.path
        );
        Ok(config)
    }
}
