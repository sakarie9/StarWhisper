CREATE TABLE IF NOT EXISTS tracked_repos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    github_id INTEGER NOT NULL UNIQUE,
    full_name TEXT NOT NULL,
    html_url TEXT NOT NULL,
    last_checked_at DATETIME
);

CREATE TABLE IF NOT EXISTS releases (
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
);

CREATE TABLE IF NOT EXISTS blacklist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    full_name TEXT NOT NULL UNIQUE,
    added_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
