use std::sync::Arc;

use tracing::{debug, error, info};

use starwhisper::commands;
use starwhisper::config;
use starwhisper::db;
use starwhisper::github;
use starwhisper::llm;
use starwhisper::scheduler;
use starwhisper::telegram;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init tracing with RUST_LOG env filter, default to info
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("StarWhisper starting up");

    debug!("Loading configuration from config.toml");
    let config = Arc::new(config::Config::load()?);
    info!(
        "Config loaded: {} GitHub users, cron '{}', notify_mode '{}', LLM provider '{}'",
        config.github.usernames.len(),
        config.schedule.cron,
        config.schedule.notify_mode,
        config.llm.provider,
    );

    debug!("Connecting to database at '{}'", config.database.path);
    let db = Arc::new(db::repository::Database::connect(&config.database.path).await?);
    info!("Database connected");

    debug!("Initializing GitHub client");
    let github = github::client::GitHubClient::new(config.github.token.clone());
    info!("GitHub client initialized");

    debug!("Initializing Telegram notifier");
    let telegram = telegram::notifier::TelegramNotifier::new(
        config.telegram.bot_token.clone(),
        config.telegram.chat_id.clone(),
    );
    info!(
        "Telegram notifier initialized (chat_id: {})",
        config.telegram.chat_id
    );

    debug!(
        "Initializing LLM client (provider: {}, model: {})",
        config.llm.provider, config.llm.model
    );
    let llm = llm::client::LlmClient::new(config.llm.clone());
    info!("LLM client initialized");

    // First startup with empty database: refresh immediately
    if !db.has_tracked_repos().await.unwrap_or(false) {
        info!("First startup detected (no tracked repos), running initial check");
        match scheduler::run_check(
            &config,
            &db,
            &github,
            &telegram,
            &llm,
            &scheduler::NoopProgress,
        )
        .await
        {
            Ok(()) => info!("Initial check completed successfully"),
            Err(e) => error!("Initial check failed: {:#}", e),
        }
    } else {
        debug!("Existing tracked repos found, skipping initial check");
    }

    // Spawn bot command handler
    let bot_config = config.clone();
    let bot_db = db.clone();
    let bot_github = github.clone();
    let bot_telegram = telegram.clone();
    let bot_llm = llm.clone();
    tokio::spawn(async move {
        info!("Bot command handler spawned");
        if let Err(e) =
            commands::run_bot(bot_config, bot_db, bot_github, bot_telegram, bot_llm).await
        {
            error!("Bot command handler failed: {:#}", e);
        }
    });

    // Run scheduler (blocks forever)
    info!("Starting scheduler...");
    scheduler::run_scheduler(config, db, github, telegram, llm).await?;

    Ok(())
}
