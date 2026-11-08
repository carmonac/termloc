//! This module contains model, controller, and view logic for Blocklist AI.
mod action_model;
pub mod agent_view;
pub mod block;
pub mod code_block;
mod context_model;
mod controller;
pub(crate) mod conversation_selection;
pub(crate) mod diff_storage;
pub(crate) mod diff_types;

mod passive_suggestions;
pub(crate) mod queued_query;
pub(super) use controller::RequestInput;
pub mod history_model;
pub mod inline_action;
mod input_mode_policy;
mod input_model;
mod permissions;
mod persistence;
pub mod prompt;
pub mod suggested_agent_mode_workflow_modal;
pub mod suggested_rule_modal;
mod suggestion_chip_view;
pub mod summarization_cancel_dialog;
pub mod usage;

pub(crate) mod codebase_index_speedbump_banner;
pub(crate) mod view_util;

// Consumed by `tui_export` for the `warp_tui` frontend.
#[cfg_attr(not(feature = "tui"), allow(unused_imports))]
pub use action_model::AIActionStatus;
pub(crate) use action_model::recording_controller::{RecordingController, video_recording_enabled};
// Consumed by `tui_export` for the `warp_tui` frontend.
#[cfg(feature = "tui")]
pub use action_model::{
    AskUserQuestionExecutor, NewConversationDecision, RequestFileEditsExecutor,
};
pub use action_model::{
    BlocklistAIActionEvent, BlocklistAIActionModel, ShellCommandExecutor, ShellCommandExecutorEvent,
};
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(crate) use action_model::{FileReadResult, apply_edits, read_local_file_context};
#[cfg(any(test, feature = "integration_tests"))]
pub(crate) use block::model::testing::FakeAIBlockModel;
pub(crate) use block::{AIBlock, AIBlockEvent, init, model};
pub use block::{keyboard_navigable_buttons, toggleable_items};
#[cfg(feature = "tui")]
pub use context_model::PendingAttachmentSummary;
#[cfg(not(feature = "tui"))]
pub(crate) use context_model::block_context_from_terminal_model;
#[cfg(feature = "tui")]
pub use context_model::block_context_from_terminal_model;
pub use context_model::{
    AttachmentType, BlocklistAIContextEvent, BlocklistAIContextModel, PendingAttachment,
    PendingFile,
};
pub use controller::BlocklistAIController;
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(in crate::ai) use controller::external_harness::ExternalHarnessPrompt;
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(crate) use controller::external_harness::{ExternalHarnessCancel, ExternalHarnessTurn};
pub use controller::input_context::{
    BLOCK_CONTEXT_ATTACHMENT_REGEX, DIFF_HUNK_ATTACHMENT_REGEX, DRIVE_OBJECT_ATTACHMENT_REGEX,
};
#[cfg(test)]
pub(crate) use controller::response_stream::ResponseStream;
pub(crate) use controller::response_stream::ResponseStreamId;
pub(crate) use controller::{
    BlocklistAIControllerEvent, CancellationRoute, ClientIdentifiers, SessionContext,
    SlashCommandRequest,
};
pub(crate) use conversation_selection::{
    ConversationSelection, ConversationSelectionEvent, ConversationSelectionHandle,
    PendingQueryState,
};
pub(crate) use history_model::{
    AIQueryHistory, AIQueryHistoryOutputStatus, BeginConversationRenameError,
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, ConversationStatusUpdate, FORK_PREFIX,
    PRE_REWIND_PREFIX,
};
// The policy types are re-exported for the TUI frontend via `tui_export`.
#[cfg_attr(not(feature = "tui"), allow(unused_imports))]
pub use input_mode_policy::{InputModePolicy, InputModePolicyHandle, PolicyConfigUpdate};
pub(crate) use input_model::BlocklistAIInputEvent;
pub use input_model::{
    BlocklistAIInputModel, InputConfig, InputType, InputTypeAutoDetectionSource,
};
pub(crate) use passive_suggestions::{
    LegacyPassiveSuggestionsEvent, LegacyPassiveSuggestionsModel, MaaPassiveSuggestionsEvent,
    MaaPassiveSuggestionsModel, PassiveSuggestionsModels,
};
pub use permissions::BlocklistAIPermissions;
#[cfg(test)]
pub(crate) use permissions::is_agent_mode_autonomy_allowed;
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(crate) use permissions::{
    CommandExecutionPermission, CommandExecutionPermissionDeniedReason, FileWritePermission,
    FileWritePermissionDeniedReason,
};
#[cfg_attr(target_family = "wasm", allow(unused))]
pub(crate) use persistence::PersistedAIInputType;
#[cfg_attr(target_family = "wasm", allow(unused))]
pub use persistence::maybe_build_ai_query_upsert_event;
pub(crate) use persistence::{PersistedAIInput, SerializedBlockListItem};
pub(crate) use queued_query::{
    AutofireAction, QueuedPromptDeliveryMode, QueuedQuery, QueuedQueryId, QueuedQueryOrigin,
    is_lrc_auto_queue_active, is_prompt_steering_enabled,
};
#[cfg_attr(not(feature = "tui"), allow(unused_imports))]
pub use queued_query::{QueuedQueryEvent, QueuedQueryModel};
pub use suggestion_chip_view::*;
pub(crate) use view_util::{
    ATTACH_AS_AGENT_MODE_CONTEXT_TEXT, CLAUDE_ORANGE, NEW_AGENT_PANE_LABEL, ai_brand_color,
    ai_indicator_height, get_ai_block_overflow_menu_element_position_id,
    get_attached_blocks_chip_element_position_id, render_ai_agent_mode_icon,
    render_ai_follow_up_icon,
};

pub use crate::ai::blocklist::block::{AIBlockResponseRating, TextLocation, secret_redaction};
