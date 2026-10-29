#[cfg(test)]
mod tests {
    use starwhisper::config::Config;

    #[test]
    fn test_config_from_toml() {
        let toml_str = r#"
[github]
token = "ghp_test"
usernames = ["user1", "user2"]

[telegram]
bot_token = "123:ABC"
chat_id = "-100123"

[llm]
provider = "openai"
api_key = "sk-test"
model = "gpt-4o"

[database]
path = ":memory:"

[schedule]
cron = "0 8 * * *"
notify_mode = "summary"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.github.token, "ghp_test");
        assert_eq!(config.github.usernames, vec!["user1", "user2"]);
        assert_eq!(config.telegram.bot_token, "123:ABC");
        assert_eq!(config.telegram.chat_id, "-100123");
        assert_eq!(config.llm.provider, "openai");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.llm.language, "English"); // default language
        assert_eq!(config.database.path, ":memory:");
        assert_eq!(config.schedule.cron, "0 8 * * *");
        assert_eq!(config.schedule.notify_mode, "summary");
        assert!(!config.schedule.notify_on_first_run); // default false
    }

    #[test]
    fn test_config_default_api_base() {
        let toml_str = r#"
[github]
token = "ghp_test"
usernames = ["user1"]

[telegram]
bot_token = "123:ABC"
chat_id = "-100123"

[llm]
provider = "openai"
api_key = "sk-test"
model = "gpt-4o"

[database]
path = ":memory:"

[schedule]
cron = "0 8 * * *"
notify_mode = "immediate"
notify_on_first_run = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.llm.api_base.is_none());
        assert_eq!(config.schedule.notify_mode, "immediate");
        assert!(config.schedule.notify_on_first_run);
    }
}
