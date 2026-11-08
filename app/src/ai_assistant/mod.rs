//! AI Assistant has since been renamed to "Warp AI" in the product.
use std::collections::HashSet;
use std::sync::Arc;

use itertools::Itertools;
use lazy_static::lazy_static;
use pathfinder_color::ColorU;
use serde::{Deserialize, Serialize};
use warp_core::command::ExitCode;

use crate::terminal::model::terminal_model::BlockIndex;
use crate::workflows::workflow::{Argument, Workflow};

pub mod execution_context;
pub mod panel;
pub mod requests;
pub mod transcript;
pub mod utils;

#[cfg(test)]
mod test_util;

/// We want to make sure the user doesn't send a prompt too large.s
/// Since a token is ~ 4 chars, the limit we impose here is 250 tokens.
/// This is also roughly the limit at which the editor starts degrading.
pub const PROMPT_CHARACTER_LIMIT: usize = 1000;

pub const AI_ASSISTANT_FEATURE_NAME: &str = "AI";
pub const ASK_AI_ASSISTANT_TEXT: &str = "Ask AI";

pub const AI_ASSISTANT_SVG_PATH: &str = "bundled/svg/ai-assistant.svg";

lazy_static! {
    pub static ref AI_ASSISTANT_LOGO_COLOR: ColorU = ColorU::new(243, 185, 17, 255);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskAIType {
    /// Covers all possible origins of text selection, including the block list terminal,
    /// the alt-screen terminal, and the input area. Not all instances will require
    /// `populate_input_box`, which determines whether we should automatically render
    /// something like "Explain the following" within the user's input box.
    FromTextSelection {
        text: Arc<String>,
        populate_input_box: bool,
    },
    /// Data about a block to inform Agent Mode.
    FromBlock {
        input: Arc<String>,
        output: Arc<String>,
        exit_code: ExitCode,
        block_index: BlockIndex,
    },
    /// Which blocks to attach to a block list AI query.
    FromBlocks {
        block_indices: HashSet<BlockIndex>,
    },
    FromAICommandSearch {
        query: Arc<String>,
    },
}

pub struct AIGeneratedCommand {
    command: String,
    description: String,
    parameters: Vec<AIGeneratedCommandParameter>,
}

pub struct AIGeneratedCommandParameter {
    id: String,
    description: String,
}

impl From<AIGeneratedCommand> for Workflow {
    fn from(ai_command: AIGeneratedCommand) -> Self {
        // Note that we use the AI generated description as the _title_ of the workflow.
        Workflow::new(ai_command.description, ai_command.command).with_arguments(
            ai_command
                .parameters
                .into_iter()
                .map(|p| Argument {
                    name: p.id,
                    description: Some(p.description),
                    default_value: None,
                    arg_type: Default::default(),
                })
                .collect_vec(),
        )
    }
}

#[cfg(not(target_family = "wasm"))]
impl From<local_agent::oneshot::GeneratedCommand> for AIGeneratedCommand {
    fn from(value: local_agent::oneshot::GeneratedCommand) -> Self {
        AIGeneratedCommand {
            command: value.command,
            description: value.description,
            parameters: Vec::new(),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum GenerateCommandsFromNaturalLanguageError {
    BadPrompt,
    AiProviderError,
    Other,
}
