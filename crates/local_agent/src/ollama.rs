use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt as _, Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// Reasoning tokens from thinking models. Never sent back to the model.
    #[serde(default, skip_serializing)]
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn tool_result(tool_name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_name: Some(tool_name.into()),
            ..Self::new("tool", content)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub function: ChatFunction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatFunction {
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub options: ChatOptions,
    pub stream: bool,
    /// How long Ollama keeps the model loaded, so a turn's steps don't each reload it.
    pub keep_alive: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChatOptions {
    pub num_ctx: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub message: ChatMessage,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A model installed in Ollama.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalModel {
    pub name: String,
    pub supports_tools: bool,
    pub supports_vision: bool,
    /// The model's context window in tokens, when Ollama reports it.
    pub context_length: Option<u32>,
    /// Size on disk in bytes.
    pub size: u64,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TaggedModel>,
}

#[derive(Deserialize)]
struct TaggedModel {
    name: String,
    #[serde(default)]
    size: u64,
}

#[derive(Default, Deserialize)]
struct ShowResponse {
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    model_info: serde_json::Map<String, Value>,
}

impl ShowResponse {
    fn into_model(self, name: String, size: u64) -> LocalModel {
        let context_length = self
            .model_info
            .iter()
            .find(|(key, _)| key.ends_with(".context_length"))
            .and_then(|(_, value)| value.as_u64())
            .and_then(|length| u32::try_from(length).ok());
        let supports = |capability: &str| self.capabilities.iter().any(|c| c == capability);
        LocalModel {
            supports_tools: supports("tools"),
            supports_vision: supports("vision"),
            context_length,
            size,
            name,
        }
    }
}

pub type ChatStream = BoxStream<'static, anyhow::Result<ChatChunk>>;

/// A source of streamed chat completions.
pub trait ChatBackend: Send + Sync + 'static {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'static, anyhow::Result<ChatStream>>;
}

pub struct OllamaClient {
    client: Arc<http_client::Client>,
    base_url: String,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Arc::new(http_client::Client::new_external()),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }
}

impl OllamaClient {
    /// Lists the installed models and what each one can do.
    pub async fn list_models(&self) -> anyhow::Result<Vec<LocalModel>> {
        let url = format!("{}/api/tags", self.base_url);
        let tags: TagsResponse = self
            .client
            .get(url.as_str())
            .send()
            .await
            .with_context(|| format!("could not reach Ollama at {url}"))?
            .json()
            .await
            .context("invalid Ollama model list")?;
        let mut models = Vec::with_capacity(tags.models.len());
        for model in tags.models {
            models.push(self.show_model(model).await?);
        }
        Ok(models)
    }

    async fn show_model(&self, model: TaggedModel) -> anyhow::Result<LocalModel> {
        let TaggedModel { name, size } = model;
        let url = format!("{}/api/show", self.base_url);
        let details: ShowResponse = self
            .client
            .post(url.as_str())
            .json(&serde_json::json!({ "model": name }))
            .send()
            .await
            .with_context(|| format!("could not reach Ollama at {url}"))?
            .json()
            .await
            .with_context(|| format!("invalid Ollama details for {name}"))?;
        Ok(details.into_model(name, size))
    }
}

impl ChatBackend for OllamaClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'static, anyhow::Result<ChatStream>> {
        let client = self.client.clone();
        let url = format!("{}/api/chat", self.base_url);
        async move {
            let response = client
                .post(url.as_str())
                .json(&request)
                .send()
                .await
                .with_context(|| format!("could not reach Ollama at {url}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("Ollama returned {status} for {url}"));
            }
            let bytes = response.bytes_stream().map(|chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(anyhow::Error::from)
            });
            Ok(parse_ndjson(bytes).boxed())
        }
        .boxed()
    }
}

/// Decodes newline-delimited [`ChatChunk`]s, whose lines may be split across network reads.
pub(crate) fn parse_ndjson(
    bytes: impl Stream<Item = anyhow::Result<Vec<u8>>> + Send + 'static,
) -> impl Stream<Item = anyhow::Result<ChatChunk>> + Send + 'static {
    async_stream::try_stream! {
        let mut bytes = Box::pin(bytes);
        let mut buffer = Vec::new();
        while let Some(read) = bytes.next().await {
            buffer.extend_from_slice(&read?);
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                if let Some(chunk) = parse_line(&line)? {
                    yield chunk;
                }
            }
        }
        if let Some(chunk) = parse_line(&buffer)? {
            yield chunk;
        }
    }
}

fn parse_line(line: &[u8]) -> anyhow::Result<Option<ChatChunk>> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let chunk: ChatChunk = serde_json::from_slice(line).context("invalid Ollama response line")?;
    match chunk.error {
        Some(error) => Err(anyhow!("Ollama error: {error}")),
        None => Ok(Some(chunk)),
    }
}

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
