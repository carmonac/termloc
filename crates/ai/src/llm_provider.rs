use serde::{Deserialize, Serialize};
use warp_core::ui::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LLMProvider {
    Ollama,
    /// Also absorbs provider names persisted by builds that supported other providers, so cached
    /// model lists keep deserializing.
    #[serde(other)]
    Unknown,
}

impl LLMProvider {
    pub fn icon(self) -> Option<Icon> {
        match self {
            Self::Ollama | Self::Unknown => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::Unknown => "this provider",
        }
    }
}

#[cfg(test)]
#[path = "llm_provider_tests.rs"]
mod tests;
