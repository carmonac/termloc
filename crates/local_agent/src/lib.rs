//! Serves agent turns from a local Ollama instance, speaking the same `ResponseEvent` stream the
//! client consumes from a remote multi-agent server.
mod engine;
mod events;
mod history;
mod mcp;
pub mod ollama;
mod prompt;
mod tools;

pub use engine::{LocalAgentConfig, run_turn};
pub use ollama::{ChatBackend, LocalModel, OllamaClient};
