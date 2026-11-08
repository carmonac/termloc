use anyhow::{Context as _, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
#[cfg(test)]
use mockall::automock;
use warp_core::channel::ChannelState;
use warp_multi_agent_api::ConversationData;
use warp_server_client::PresignedDownloadResponse;

use super::ServerApi;
use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::ServerAIConversationMetadata;
use crate::ai::generate_code_review_content::api::{
    GenerateCodeReviewContentRequest, GenerateCodeReviewContentResponse,
};
use crate::ai::request_usage_model::RequestLimitInfo;
use crate::ai::{AICreditAvailability, RequestUsageInfo};
use crate::ai_assistant::execution_context::WarpAiExecutionContext;
use crate::ai_assistant::requests::GenerateDialogueResult;
use crate::ai_assistant::utils::TranscriptPart;
use crate::ai_assistant::{AIGeneratedCommand, GenerateCommandsFromNaturalLanguageError};
use crate::drive::workflows::ai_assist::{GeneratedCommandMetadata, GeneratedCommandMetadataError};
use crate::terminal::model::block::SerializedBlock;

/// Request body for `POST /agent/conversations/{conversation_id}/fork`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ForkConversationRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// Response body for `POST /agent/conversations/{conversation_id}/fork`. The returned id is sent
/// on the subsequent `POST /agent/runs` request under `conversation_id` (resume semantics).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ForkConversationResponse {
    pub forked_conversation_id: String,
}

/// Request body for `POST /agent/conversations/{conversation_id}/rename`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenameConversationRequest {
    pub title: String,
}

/// Response body for `POST /agent/conversations/{conversation_id}/rename`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RenameConversationResponse {
    pub title: String,
}

/// Response from the artifact endpoint.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "artifact_type")]
pub enum ArtifactDownloadResponse {
    #[serde(rename = "SCREENSHOT")]
    Screenshot {
        #[serde(flatten)]
        common: ArtifactDownloadCommonFields,
        data: ScreenshotArtifactResponseData,
    },
    #[serde(rename = "FILE")]
    File {
        #[serde(flatten)]
        common: ArtifactDownloadCommonFields,
        data: FileArtifactResponseData,
    },
}

impl ArtifactDownloadResponse {
    fn common(&self) -> &ArtifactDownloadCommonFields {
        match self {
            ArtifactDownloadResponse::Screenshot { common, .. }
            | ArtifactDownloadResponse::File { common, .. } => common,
        }
    }

    pub fn artifact_uid(&self) -> &str {
        &self.common().artifact_uid
    }

    pub fn artifact_type(&self) -> &'static str {
        match self {
            ArtifactDownloadResponse::Screenshot { .. } => "SCREENSHOT",
            ArtifactDownloadResponse::File { .. } => "FILE",
        }
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.common().created_at
    }

    pub fn download_url(&self) -> &str {
        match self {
            ArtifactDownloadResponse::Screenshot { data, .. } => &data.download_url,
            ArtifactDownloadResponse::File { data, .. } => &data.download_url,
        }
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        match self {
            ArtifactDownloadResponse::Screenshot { data, .. } => data.expires_at,
            ArtifactDownloadResponse::File { data, .. } => data.expires_at,
        }
    }

    pub fn content_type(&self) -> &str {
        match self {
            ArtifactDownloadResponse::Screenshot { data, .. } => &data.content_type,
            ArtifactDownloadResponse::File { data, .. } => &data.content_type,
        }
    }

    pub fn filepath(&self) -> Option<&str> {
        match self {
            ArtifactDownloadResponse::Screenshot { .. } => None,
            ArtifactDownloadResponse::File { data, .. } => Some(&data.filepath),
        }
    }

    pub fn filename(&self) -> Option<&str> {
        match self {
            ArtifactDownloadResponse::Screenshot { .. } => None,
            ArtifactDownloadResponse::File { data, .. } => Some(&data.filename),
        }
    }

    pub fn description(&self) -> Option<&str> {
        match self {
            ArtifactDownloadResponse::Screenshot { data, .. } => data.description.as_deref(),
            ArtifactDownloadResponse::File { data, .. } => data.description.as_deref(),
        }
    }

    pub fn size_bytes(&self) -> Option<i64> {
        match self {
            ArtifactDownloadResponse::Screenshot { .. } => None,
            ArtifactDownloadResponse::File { data, .. } => data.size_bytes,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ArtifactDownloadCommonFields {
    pub artifact_uid: String,
    pub created_at: DateTime<Utc>,
}

/// Screenshot-specific data from the artifact endpoint.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ScreenshotArtifactResponseData {
    pub download_url: String,
    pub expires_at: DateTime<Utc>,
    pub content_type: String,
    pub description: Option<String>,
}

/// File-specific data from the artifact endpoint.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct FileArtifactResponseData {
    pub download_url: String,
    pub expires_at: DateTime<Utc>,
    pub content_type: String,
    pub filepath: String,
    pub filename: String,
    pub description: Option<String>,
    pub size_bytes: Option<i64>,
}

pub(crate) fn build_fork_conversation_url(conversation_id: &str) -> String {
    format!(
        "agent/conversations/{}/fork",
        urlencoding::encode(conversation_id)
    )
}

pub(crate) fn build_rename_conversation_url(conversation_id: &str) -> String {
    format!(
        "agent/conversations/{}/rename",
        urlencoding::encode(conversation_id)
    )
}

async fn local_commands_from_natural_language(
    prompt: &str,
) -> Result<Vec<AIGeneratedCommand>, GenerateCommandsFromNaturalLanguageError> {
    let client = crate::ai::local::one_shot()
        .await
        .map_err(|_| GenerateCommandsFromNaturalLanguageError::AiProviderError)?;
    let commands = client.generate_commands(prompt).await.map_err(|error| {
        log::warn!("Local command generation failed: {error:#}");
        GenerateCommandsFromNaturalLanguageError::AiProviderError
    })?;
    Ok(commands.into_iter().map(Into::into).collect())
}

async fn local_dialogue_answer(
    transcript: &[TranscriptPart],
    prompt: &str,
) -> anyhow::Result<GenerateDialogueResult> {
    let history: Vec<(String, String)> = transcript
        .iter()
        .map(|part| {
            (
                part.user.raw.clone(),
                part.assistant.formatted_message.raw.clone(),
            )
        })
        .collect();
    let client = crate::ai::local::one_shot().await?;
    let answer = client.answer(&history, prompt).await?;
    Ok(GenerateDialogueResult::Success {
        answer,
        truncated: false,
        request_limit_info: RequestLimitInfo::unlimited(),
        transcript_summarized: false,
    })
}

async fn local_metadata_for_command(
    command: &str,
) -> Result<GeneratedCommandMetadata, GeneratedCommandMetadataError> {
    let client = crate::ai::local::one_shot()
        .await
        .map_err(|_| GeneratedCommandMetadataError::AiProviderError)?;
    client
        .command_metadata(command)
        .await
        .map(Into::into)
        .map_err(|error| {
            log::warn!("Local command metadata failed: {error:#}");
            GeneratedCommandMetadataError::BadCommand
        })
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AIClient: 'static + Send + Sync {
    async fn generate_commands_from_natural_language(
        &self,
        prompt: String,
        ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> Result<Vec<AIGeneratedCommand>, GenerateCommandsFromNaturalLanguageError>;

    async fn generate_dialogue_answer(
        &self,
        transcript: Vec<TranscriptPart>,
        prompt: String,
        ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> anyhow::Result<GenerateDialogueResult>;

    async fn generate_metadata_for_command(
        &self,
        command: String,
    ) -> Result<GeneratedCommandMetadata, GeneratedCommandMetadataError>;

    async fn get_request_limit_info(&self) -> Result<RequestUsageInfo, anyhow::Error>;

    /// Returns whether the user can start an interactive AI request.
    async fn get_ai_credit_availability(&self) -> Result<AICreditAvailability, anyhow::Error>;

    async fn provide_negative_feedback_response_for_ai_conversation(
        &self,
        conversation_id: String,
        request_ids: Vec<String>,
    ) -> anyhow::Result<i32, anyhow::Error>;

    /// Materialize a server-side fork of a conversation.
    async fn fork_conversation(
        &self,
        conversation_id: String,
        title: Option<String>,
    ) -> anyhow::Result<ForkConversationResponse, anyhow::Error>;

    /// Rename a server-side conversation and return the normalized title.
    async fn rename_conversation(
        &self,
        conversation_id: String,
        title: String,
    ) -> anyhow::Result<RenameConversationResponse, anyhow::Error>;

    async fn get_ai_conversation(
        &self,
        server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<(ConversationData, ServerAIConversationMetadata), anyhow::Error>;

    async fn list_ai_conversation_metadata(
        &self,
        conversation_ids: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<ServerAIConversationMetadata>>;

    async fn get_block_snapshot(
        &self,
        server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<SerializedBlock, anyhow::Error>;

    async fn delete_ai_conversation(
        &self,
        server_conversation_token: String,
    ) -> anyhow::Result<(), anyhow::Error>;

    async fn get_artifact_download(
        &self,
        artifact_uid: &str,
    ) -> anyhow::Result<ArtifactDownloadResponse, anyhow::Error>;

    /// Downloads the bytes of a computer-use screenshot stored in Warp-managed
    /// object storage. Requires view access to the conversation.
    async fn download_stored_screenshot(
        &self,
        conversation_id: &str,
        screenshot_uid: &str,
    ) -> anyhow::Result<Bytes, anyhow::Error>;

    /// Generates AI copy for code-review flows: commit messages at dialog-open
    /// time and PR titles / bodies at confirm time. `output_type` in the
    /// request picks which of the three the server returns.
    async fn generate_code_review_content(
        &self,
        request: GenerateCodeReviewContentRequest,
    ) -> Result<GenerateCodeReviewContentResponse, anyhow::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AIClient for ServerApi {
    async fn generate_commands_from_natural_language(
        &self,
        prompt: String,
        _ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> Result<Vec<AIGeneratedCommand>, GenerateCommandsFromNaturalLanguageError> {
        local_commands_from_natural_language(&prompt).await
    }

    async fn generate_dialogue_answer(
        &self,
        transcript: Vec<TranscriptPart>,
        prompt: String,
        _ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> anyhow::Result<GenerateDialogueResult> {
        local_dialogue_answer(&transcript, &prompt).await
    }

    async fn generate_metadata_for_command(
        &self,
        command: String,
    ) -> Result<GeneratedCommandMetadata, GeneratedCommandMetadataError> {
        local_metadata_for_command(&command).await
    }

    async fn get_request_limit_info(&self) -> Result<RequestUsageInfo, anyhow::Error> {
        Ok(RequestUsageInfo {
            request_limit_info: RequestLimitInfo::unlimited(),
            bonus_grants: vec![],
        })
    }

    async fn get_ai_credit_availability(&self) -> Result<AICreditAvailability, anyhow::Error> {
        Ok(AICreditAvailability::available_with_source(None))
    }

    async fn provide_negative_feedback_response_for_ai_conversation(
        &self,
        _conversation_id: String,
        _request_ids: Vec<String>,
    ) -> anyhow::Result<i32, anyhow::Error> {
        Ok(0)
    }

    async fn get_ai_conversation(
        &self,
        _server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<(ConversationData, ServerAIConversationMetadata), anyhow::Error> {
        Err(anyhow!(
            "Cloud conversations are not available in local-only builds"
        ))
    }

    async fn list_ai_conversation_metadata(
        &self,
        _conversation_ids: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<ServerAIConversationMetadata>> {
        Ok(vec![])
    }

    async fn delete_ai_conversation(
        &self,
        _server_conversation_token: String,
    ) -> anyhow::Result<(), anyhow::Error> {
        Ok(())
    }

    async fn fork_conversation(
        &self,
        conversation_id: String,
        title: Option<String>,
    ) -> anyhow::Result<ForkConversationResponse, anyhow::Error> {
        let request = ForkConversationRequest { title };
        let response: ForkConversationResponse = self
            .post_public_api(&build_fork_conversation_url(&conversation_id), &request)
            .await?;
        Ok(response)
    }

    async fn rename_conversation(
        &self,
        conversation_id: String,
        title: String,
    ) -> anyhow::Result<RenameConversationResponse, anyhow::Error> {
        let request = RenameConversationRequest { title };
        let response: RenameConversationResponse = self
            .post_public_api(&build_rename_conversation_url(&conversation_id), &request)
            .await?;
        Ok(response)
    }

    async fn get_block_snapshot(
        &self,
        server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<SerializedBlock, anyhow::Error> {
        let conversation_id = server_conversation_token.as_str();
        // Make sure to use `SerializedBlock::from_json` to correctly handle the serialized
        // command and output grid contents.
        let response = self
            .get_public_api_response(&format!(
                "agent/conversations/{conversation_id}/block-snapshot?redirect=false"
            ))
            .await?;
        let download = response
            .json::<PresignedDownloadResponse>()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Failed to decode block snapshot download URL")?;
        let json_bytes = self.base_client.download_presigned(&download.url).await?;
        SerializedBlock::from_json(&json_bytes)
    }

    async fn get_artifact_download(
        &self,
        artifact_uid: &str,
    ) -> anyhow::Result<ArtifactDownloadResponse, anyhow::Error> {
        let response: ArtifactDownloadResponse = self
            .get_public_api(&format!("agent/artifacts/{artifact_uid}"))
            .await?;
        Ok(response)
    }

    async fn download_stored_screenshot(
        &self,
        conversation_id: &str,
        screenshot_uid: &str,
    ) -> anyhow::Result<Bytes, anyhow::Error> {
        // The endpoint redirects to a short-lived signed URL, which the HTTP
        // client follows transparently. Strip that URL from body-read errors so
        // it cannot leak into logs or Sentry breadcrumbs.
        let response = self
            .get_public_api_response(&format!(
                "agent/conversations/{}/screenshots/{}/download",
                urlencoding::encode(conversation_id),
                urlencoding::encode(screenshot_uid)
            ))
            .await?;
        response
            .bytes()
            .await
            .map_err(|error| anyhow::Error::new(error.without_url()))
    }

    async fn generate_code_review_content(
        &self,
        request: GenerateCodeReviewContentRequest,
    ) -> Result<GenerateCodeReviewContentResponse, anyhow::Error> {
        let auth_token = self.get_or_refresh_access_token().await?;
        let request_builder = self.base_client.http_client().post(format!(
            "{}/ai/generate_code_review_content",
            ChannelState::server_root_url()
        ));
        let response = if let Some(token) = auth_token.as_bearer_token() {
            request_builder.bearer_auth(token)
        } else {
            request_builder
        }
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
        Ok(response)
    }
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
