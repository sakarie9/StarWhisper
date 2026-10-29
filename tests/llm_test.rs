use starwhisper::config::LlmConfig;
use starwhisper::llm::client::LlmClient;

#[test]
fn test_llm_client_creation() {
    let config = LlmConfig {
        provider: "openai".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-4o".to_string(),
        api_base: None,
        max_tokens: 65536,
        language: "中文".to_string(),
    };

    let _client = LlmClient::new(config);
}

#[test]
fn test_llm_client_with_custom_base() {
    let config = LlmConfig {
        provider: "claude".to_string(),
        api_key: "sk-ant-test".to_string(),
        model: "claude-sonnet-4-20250514".to_string(),
        api_base: Some("https://custom.api.com/v1".to_string()),
        max_tokens: 65536,
        language: "English".to_string(),
    };

    let _client = LlmClient::new(config);
}
