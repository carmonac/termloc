use super::*;

#[test]
fn url_must_be_http_with_a_host() {
    assert!(is_valid_url("http://localhost:11434"));
    assert!(is_valid_url("https://ollama.lan"));
    assert!(!is_valid_url("localhost:11434"));
    assert!(!is_valid_url("ftp://ollama.lan"));
    assert!(!is_valid_url(""));
}

#[test]
fn context_window_must_be_a_usable_number() {
    assert_eq!(parse_context_window(" 16384 "), Some(16_384));
    assert_eq!(parse_context_window("100"), None);
    assert_eq!(parse_context_window("lots"), None);
}
