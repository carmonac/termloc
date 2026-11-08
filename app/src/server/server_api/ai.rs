use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use local_agent::oneshot::{ChangeContext, ChangeText};
#[cfg(test)]
use mockall::automock;
use warp_multi_agent_api::ConversationData;

use super::ServerApi;
use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::ServerAIConversationMetadata;
use crate::ai::generate_code_review_content::api::{
    GenerateCodeReviewContentRequest, GenerateCodeReviewContentResponse, OutputType,
};
use crate::ai_assistant::execution_context::WarpAiExecutionContext;
use crate::ai_assistant::requests::GenerateDialogueResult;
use crate::ai_assistant::utils::TranscriptPart;
use crate::ai_assistant::{AIGeneratedCommand, GenerateCommandsFromNaturalLanguageError};
use crate::drive::workflows::ai_assist::{GeneratedCommandMetadata, GeneratedCommandMetadataError};
use crate::terminal::model::block::SerializedBlock;

/// Response body for `POST /agent/conversations/{conversation_id}/fork`. The returned id is sent
/// on the subsequent `POST /agent/runs` request under `conversation_id` (resume semantics).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ForkConversationResponse {
    pub forked_conversation_id: String,
}

/// Response body for `POST /agent/conversations/{conversation_id}/rename`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RenameConversationResponse {
    pub title: String,
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
    Ok(GenerateDialogueResult {
        answer,
        truncated: false,
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
        _conversation_id: String,
        _title: Option<String>,
    ) -> anyhow::Result<ForkConversationResponse, anyhow::Error> {
        Err(cloud_conversations_unavailable())
    }

    async fn rename_conversation(
        &self,
        _conversation_id: String,
        _title: String,
    ) -> anyhow::Result<RenameConversationResponse, anyhow::Error> {
        Err(cloud_conversations_unavailable())
    }

    async fn get_block_snapshot(
        &self,
        _server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<SerializedBlock, anyhow::Error> {
        Err(cloud_conversations_unavailable())
    }

    async fn download_stored_screenshot(
        &self,
        _conversation_id: &str,
        _screenshot_uid: &str,
    ) -> anyhow::Result<Bytes, anyhow::Error> {
        Err(cloud_conversations_unavailable())
    }

    async fn generate_code_review_content(
        &self,
        request: GenerateCodeReviewContentRequest,
    ) -> Result<GenerateCodeReviewContentResponse, anyhow::Error> {
        let kind = match request.output_type {
            OutputType::CommitMessage => ChangeText::CommitMessage,
            OutputType::PrTitle => ChangeText::PrTitle,
            OutputType::PrDescription => ChangeText::PrDescription,
        };
        let changes = ChangeContext {
            diff: request.diff,
            branch_name: request.branch_name,
            commit_messages: request.commit_messages,
        };
        let client = crate::ai::local::one_shot().await?;
        let content = client.change_text(kind, &changes).await?;
        Ok(GenerateCodeReviewContentResponse { content })
    }
}

fn cloud_conversations_unavailable() -> anyhow::Error {
    anyhow!("Cloud conversations aren't available")
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
