use starwhisper::telegram::notifier::TelegramNotifier;

#[test]
fn test_telegram_notifier_creation() {
    // This should not panic even with invalid chat_id
    let _notifier = TelegramNotifier::new("test_token".to_string(), "invalid_id".to_string());
    // Just verify it creates without panicking
}

#[test]
fn test_telegram_notifier_valid_chat_id() {
    let _notifier = TelegramNotifier::new("test_token".to_string(), "-1001234567890".to_string());
}
