use super::LLMProvider;

#[test]
fn ollama_round_trips_through_serde() {
    let json = serde_json::to_string(&LLMProvider::Ollama).unwrap();
    assert_eq!(
        serde_json::from_str::<LLMProvider>(&json).unwrap(),
        LLMProvider::Ollama
    );
}

#[test]
fn removed_providers_deserialize_as_unknown() {
    for name in ["OpenAI", "Anthropic", "Google", "Xai"] {
        assert_eq!(
            serde_json::from_str::<LLMProvider>(&format!("\"{name}\"")).unwrap(),
            LLMProvider::Unknown
        );
    }
}
