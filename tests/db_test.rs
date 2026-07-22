use chrono::Utc;
use sqlx::sqlite::SqlitePoolOptions;

use starwhisper::db::repository::Database;
use starwhisper::github::Release;

async fn setup_db() -> Database {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();

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
    .await
    .unwrap();

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
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS blacklist (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            full_name TEXT NOT NULL UNIQUE,
            added_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    Database::from_pool(pool)
}

#[tokio::test]
async fn test_get_or_create_repo() {
    let db = setup_db().await;

    // First create
    let repo = db
        .get_or_create_repo(12345, "owner/repo", "https://github.com/owner/repo")
        .await
        .unwrap();
    assert_eq!(repo.github_id, 12345);
    assert_eq!(repo.full_name, "owner/repo");

    // Second call should return same repo
    let repo2 = db
        .get_or_create_repo(12345, "owner/repo", "https://github.com/owner/repo")
        .await
        .unwrap();
    assert_eq!(repo2.id, repo.id);
}

#[tokio::test]
async fn test_save_and_get_release() {
    let db = setup_db().await;

    let repo = db
        .get_or_create_repo(12345, "owner/repo", "https://github.com/owner/repo")
        .await
        .unwrap();

    let release = Release {
        id: 100,
        tag_name: "v1.0.0".to_string(),
        name: Some("Release 1.0".to_string()),
        body: Some("Initial release".to_string()),
        published_at: Some(Utc::now()),
        html_url: "https://github.com/owner/repo/releases/tag/v1.0.0".to_string(),
        prerelease: false,
    };

    let release_id = db.save_release(repo.id, &release).await.unwrap();
    assert!(release_id > 0);

    let latest = db.get_latest_release(repo.id).await.unwrap();
    assert!(latest.is_some());
    let latest = latest.unwrap();
    assert_eq!(latest.tag_name, "v1.0.0");
    assert_eq!(latest.previous_tag_name, None);
    assert_eq!(latest.name, Some("Release 1.0".to_string()));
}

#[tokio::test]
async fn test_latest_release_returns_most_recent() {
    let db = setup_db().await;

    let repo = db
        .get_or_create_repo(12345, "owner/repo", "https://github.com/owner/repo")
        .await
        .unwrap();

    // Save v1.0.0
    let release1 = Release {
        id: 100,
        tag_name: "v1.0.0".to_string(),
        name: Some("Release 1.0".to_string()),
        body: None,
        published_at: Some(Utc::now() - chrono::Duration::days(1)),
        html_url: "https://github.com/owner/repo/releases/tag/v1.0.0".to_string(),
        prerelease: false,
    };
    db.save_release(repo.id, &release1).await.unwrap();

    // Save v2.0.0 (replaces v1.0.0)
    let release2 = Release {
        id: 200,
        tag_name: "v2.0.0".to_string(),
        name: Some("Release 2.0".to_string()),
        body: None,
        published_at: Some(Utc::now()),
        html_url: "https://github.com/owner/repo/releases/tag/v2.0.0".to_string(),
        prerelease: false,
    };
    db.save_release(repo.id, &release2).await.unwrap();

    let latest = db.get_latest_release(repo.id).await.unwrap().unwrap();
    assert_eq!(latest.tag_name, "v2.0.0");
    assert_eq!(latest.previous_tag_name, Some("v1.0.0".to_string()));

    // Only one row per repo
    let releases = db.get_releases_for_repo(repo.id).await.unwrap();
    assert_eq!(releases.len(), 1);
}

#[tokio::test]
async fn test_mark_release_notified() {
    let db = setup_db().await;

    let repo = db
        .get_or_create_repo(12345, "owner/repo", "https://github.com/owner/repo")
        .await
        .unwrap();

    let release = Release {
        id: 100,
        tag_name: "v1.0.0".to_string(),
        name: Some("Release 1.0".to_string()),
        body: None,
        published_at: Some(Utc::now()),
        html_url: "https://github.com/owner/repo/releases/tag/v1.0.0".to_string(),
        prerelease: false,
    };

    let release_id = db.save_release(repo.id, &release).await.unwrap();

    // Initially not notified
    let latest = db.get_latest_release(repo.id).await.unwrap().unwrap();
    assert!(latest.notified_at.is_none());

    // Mark as notified
    db.mark_release_notified(release_id).await.unwrap();

    let latest = db.get_latest_release(repo.id).await.unwrap().unwrap();
    assert!(latest.notified_at.is_some());
}

#[tokio::test]
async fn test_blacklist_add_and_check() {
    let db = setup_db().await;

    // Initially not blacklisted
    assert!(!db.is_blacklisted("owner/repo").await.unwrap());

    // Add to blacklist
    let added = db.add_to_blacklist("owner/repo").await.unwrap();
    assert!(added);

    // Now should be blacklisted
    assert!(db.is_blacklisted("owner/repo").await.unwrap());

    // Adding again should return false (already exists)
    let added = db.add_to_blacklist("owner/repo").await.unwrap();
    assert!(!added);
}

#[tokio::test]
async fn test_blacklist_remove() {
    let db = setup_db().await;

    // Add to blacklist
    db.add_to_blacklist("owner/repo").await.unwrap();
    assert!(db.is_blacklisted("owner/repo").await.unwrap());

    // Remove from blacklist
    let removed = db.remove_from_blacklist("owner/repo").await.unwrap();
    assert!(removed);

    // Should not be blacklisted anymore
    assert!(!db.is_blacklisted("owner/repo").await.unwrap());

    // Removing again should return false
    let removed = db.remove_from_blacklist("owner/repo").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_blacklist_list() {
    let db = setup_db().await;

    // Initially empty
    let list = db.get_blacklist().await.unwrap();
    assert!(list.is_empty());

    // Add some repos
    db.add_to_blacklist("owner/repo2").await.unwrap();
    db.add_to_blacklist("owner/repo1").await.unwrap();
    db.add_to_blacklist("another/repo").await.unwrap();

    // Should be sorted
    let list = db.get_blacklist().await.unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0], "another/repo");
    assert_eq!(list[1], "owner/repo1");
    assert_eq!(list[2], "owner/repo2");
}

#[tokio::test]
async fn test_remove_unstarred_repos() {
    let db = setup_db().await;

    // Create some repos
    db.get_or_create_repo(1, "owner/repo1", "https://github.com/owner/repo1")
        .await
        .unwrap();
    db.get_or_create_repo(2, "owner/repo2", "https://github.com/owner/repo2")
        .await
        .unwrap();
    db.get_or_create_repo(3, "owner/repo3", "https://github.com/owner/repo3")
        .await
        .unwrap();

    // Add a release to repo2
    let release = Release {
        id: 100,
        tag_name: "v1.0.0".to_string(),
        name: Some("Release 1.0".to_string()),
        body: None,
        published_at: Some(Utc::now()),
        html_url: "https://github.com/owner/repo2/releases/tag/v1.0.0".to_string(),
        prerelease: false,
    };
    db.save_release(2, &release).await.unwrap();

    // Current starred: repo1 and repo3 (repo2 was unstarred)
    let starred = vec!["owner/repo1".to_string(), "owner/repo3".to_string()];
    let removed = db.remove_unstarred_repos(&starred).await.unwrap();

    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0], "owner/repo2");

    // Verify repo2 is gone
    let all_repos = db.get_all_tracked_repos().await.unwrap();
    assert_eq!(all_repos.len(), 2);
    assert!(all_repos.iter().any(|r| r.full_name == "owner/repo1"));
    assert!(all_repos.iter().any(|r| r.full_name == "owner/repo3"));

    // Verify releases for repo2 are also gone
    let releases = db.get_latest_release(2).await.unwrap();
    assert!(releases.is_none());
}
