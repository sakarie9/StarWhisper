use starwhisper::db::repository::Database;
use starwhisper::llm::client::LlmClient;

/// Manual test: reads releases from the existing database and tests LLM batched summarization.
/// Run with: cargo test --test llm_batch_test -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_llm_batch_summarize_from_db() {
    // Init tracing for visible output
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
        .try_init();

    // Connect to the real database
    let db = Database::connect("data.db")
        .await
        .expect("Failed to connect to database");

    // Load config
    let config = starwhisper::config::Config::load().expect("Failed to load config.toml");

    // Get all tracked repos
    let repos = db
        .get_all_tracked_repos()
        .await
        .expect("Failed to get tracked repos");
    println!("Found {} tracked repos in database", repos.len());

    // Collect latest release for each repo (simulate what scheduler does)
    let mut releases: Vec<(String, String, Option<String>)> = Vec::new();
    for repo in &repos {
        if let Ok(Some(release)) = db.get_latest_release(repo.id).await {
            releases.push((
                repo.full_name.clone(),
                release.tag_name.clone(),
                release.body.clone(),
            ));
        }
    }
    println!("Collected {} releases total", releases.len());

    let with_body = releases
        .iter()
        .filter(|(_, _, b)| b.as_ref().is_some_and(|s| !s.trim().is_empty()))
        .count();
    let without_body = releases.len() - with_body;
    println!("  {} with body (will be sent to LLM)", with_body);
    println!("  {} without body (will be skipped)", without_body);

    // Create LLM client and run summarize
    let llm = LlmClient::new(config.llm);
    println!("\nStarting LLM batched summarization...");
    let start = std::time::Instant::now();

    let summaries = llm
        .summarize(&releases)
        .await
        .expect("LLM summarize failed");

    let elapsed = start.elapsed();
    println!("\nDone in {:.1}s", elapsed.as_secs_f64());
    println!(
        "Got {} summaries for {} releases",
        summaries.len(),
        releases.len()
    );

    // Verify all releases are present
    for (repo, _, _) in &releases {
        let found = summaries.iter().any(|s| s.repo == *repo);
        assert!(found, "Missing summary for {}", repo);
    }
    println!("✓ All releases present in summaries");

    // Show a few samples
    println!("\nSample summaries:");
    for s in summaries.iter().take(10) {
        let desc = s.description.as_deref().unwrap_or("(none)");
        println!("  {} : {}", s.repo, desc);
    }
}
