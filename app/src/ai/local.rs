//! Routes agent requests to a local Ollama model when `WARP_LOCAL_AGENT_MODEL` is set.
use std::sync::Arc;

use futures::Stream;
use local_agent::{LocalAgentConfig, OllamaClient};
use warp_multi_agent_api::{Request, ResponseEvent};

const MODEL_ENV_VAR: &str = "WARP_LOCAL_AGENT_MODEL";
const URL_ENV_VAR: &str = "WARP_LOCAL_AGENT_URL";
const DEFAULT_URL: &str = "http://localhost:11434";
const DEFAULT_NUM_CTX: u32 = 16_384;

pub(crate) fn is_enabled() -> bool {
    std::env::var_os(MODEL_ENV_VAR).is_some()
}

/// Starts a local turn, or returns `None` when no local model is configured.
pub(crate) fn run_turn(request: Request) -> Option<impl Stream<Item = ResponseEvent> + Send> {
    let model = std::env::var(MODEL_ENV_VAR).ok()?;
    let url = std::env::var(URL_ENV_VAR).unwrap_or_else(|_| DEFAULT_URL.to_owned());
    let config = LocalAgentConfig {
        model,
        num_ctx: DEFAULT_NUM_CTX,
    };
    Some(local_agent::run_turn(
        Arc::new(OllamaClient::new(url)),
        config,
        request,
    ))
}
