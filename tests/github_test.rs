use starwhisper::github::Release;
use starwhisper::github::Repository;

#[test]
fn test_repository_deserialization() {
    let json = r#"{
        "id": 12345,
        "full_name": "octocat/Hello-World",
        "html_url": "https://github.com/octocat/Hello-World",
        "description": "My first repository on GitHub!"
    }"#;

    let repo: Repository = serde_json::from_str(json).unwrap();
    assert_eq!(repo.id, 12345);
    assert_eq!(repo.full_name, "octocat/Hello-World");
    assert_eq!(repo.html_url, "https://github.com/octocat/Hello-World");
    assert_eq!(
        repo.description,
        Some("My first repository on GitHub!".to_string())
    );
}

#[test]
fn test_release_deserialization() {
    let json = r#"{
        "id": 100,
        "tag_name": "v1.0.0",
        "name": "Release 1.0",
        "body": "Initial release with basic features",
        "published_at": "2024-01-15T10:30:00Z",
        "html_url": "https://github.com/octocat/Hello-World/releases/tag/v1.0.0",
        "prerelease": false
    }"#;

    let release: Release = serde_json::from_str(json).unwrap();
    assert_eq!(release.id, 100);
    assert_eq!(release.tag_name, "v1.0.0");
    assert_eq!(release.name, Some("Release 1.0".to_string()));
    assert_eq!(
        release.body,
        Some("Initial release with basic features".to_string())
    );
    assert!(release.published_at.is_some());
    assert!(!release.prerelease);
}

#[test]
fn test_release_deserialization_minimal() {
    let json = r#"{
        "id": 200,
        "tag_name": "v2.0.0-beta",
        "html_url": "https://github.com/octocat/Hello-World/releases/tag/v2.0.0-beta",
        "prerelease": true
    }"#;

    let release: Release = serde_json::from_str(json).unwrap();
    assert_eq!(release.id, 200);
    assert_eq!(release.tag_name, "v2.0.0-beta");
    assert!(release.name.is_none());
    assert!(release.body.is_none());
    assert!(release.published_at.is_none());
    assert!(release.prerelease);
}

#[test]
fn test_release_deserialization_null_fields() {
    let json = r#"{
        "id": 300,
        "tag_name": "v3.0.0",
        "name": null,
        "body": null,
        "published_at": "2024-03-20T15:45:00Z",
        "html_url": "https://github.com/octocat/Hello-World/releases/tag/v3.0.0",
        "prerelease": false
    }"#;

    let release: Release = serde_json::from_str(json).unwrap();
    assert_eq!(release.id, 300);
    assert!(release.name.is_none());
    assert!(release.body.is_none());
    assert!(release.published_at.is_some());
}
