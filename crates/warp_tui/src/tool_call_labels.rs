//! Per-tool, per-state labels for tool-call rows in the TUI transcript,
//! modeled on the GUI's inline action text.

use std::path::Path;

use i18n::t;
use warp::tui_export::{
    AIActionStatus, AIAgentAction, AIAgentActionResultType, AIAgentActionType,
    AskUserQuestionResult, FileGlobV2Result, GrepResult, RequestCommandOutputResult,
    SearchCodebaseFailureReason, SearchCodebaseResult, StopRecordingResult,
    SuggestNewConversationResult, mcp_server_name_for_id,
};
use warp_core::command::ExitCode;
use warpui_core::AppContext;
use warpui_core::elements::tui::{Modifier, TuiStyle};

use self::ToolCallDisplayState as State;
use crate::tui_builder::TuiUiBuilder;

/// Ground-truth state of the terminal block backing a shell-command tool
/// call, resolved by the caller. When a block exists, its state supersedes
/// the stored action status/result for execution states (mirroring the GUI's
/// `RequestedCommandView`, which derives icon and expandability from the
/// block whenever one exists). Notably, an agent-monitored command's stored
/// result stays a `LongRunningCommandSnapshot` forever, so without the block
/// its row could never leave the "still running" state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBlockState {
    Running,
    Finished { exit_code: ExitCode },
}

/// A shell-command tool call's terminal block as resolved by the caller: its
/// execution state plus the command it actually ran. The block's command
/// supersedes the streamed one, which the user may have edited before
/// accepting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedCommandBlock {
    /// The block's command, when it has one; `None` while the block's
    /// command grid is still empty.
    pub(crate) command: Option<String>,
    pub(crate) state: CommandBlockState,
}

/// Longest rendered length for compact interpolated values such as queries and
/// paths. Shell commands are preserved in full and wrap in their collapsible
/// header instead.
const MAX_INLINE_LEN: usize = 80;

/// Coarse presentation state for a tool call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolCallDisplayState {
    /// The tool call's arguments are still streaming and may be incomplete.
    Constructing,
    /// The tool call is waiting to begin execution.
    Pending,
    /// The tool call is blocked on user confirmation.
    Blocked,
    /// The tool call is executing asynchronously.
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ToolCallDisplayState {
    /// The compact leading glyph for this state.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Constructing | Self::Pending => "○",
            Self::Blocked | Self::Cancelled => "■",
            Self::Running => "●",
            Self::Succeeded => "✓",
            Self::Failed => "×",
        }
    }

    /// The semantic theme style for this state's glyph.
    pub(crate) fn glyph_style(self, builder: &TuiUiBuilder) -> TuiStyle {
        match self {
            Self::Constructing | Self::Pending => builder.dim_text_style(),
            Self::Blocked | Self::Running => builder.attention_glyph_style(),
            Self::Succeeded => builder.success_glyph_style(),
            Self::Failed => builder.error_text_style(),
            Self::Cancelled => builder.muted_text_style(),
        }
    }

    /// The semantic text style paired with this state.
    pub(crate) fn label_style(self, builder: &TuiUiBuilder) -> TuiStyle {
        match self {
            Self::Constructing | Self::Pending => builder.dim_text_style(),
            Self::Blocked | Self::Running | Self::Succeeded | Self::Failed | Self::Cancelled => {
                builder.primary_text_style()
            }
        }
    }
}

/// Styles the first word of a tool-call label as the action and the rest as details.
pub(crate) fn styled_tool_call_label_spans(
    label: &str,
    builder: &TuiUiBuilder,
) -> Vec<(String, TuiStyle)> {
    let action_style = builder.primary_text_style().add_modifier(Modifier::BOLD);
    let details_style = builder.neutral_7_text_style();
    match label.find(char::is_whitespace) {
        Some(first_word_end) => vec![
            (label[..first_word_end].to_owned(), action_style),
            (label[first_word_end..].to_owned(), details_style),
        ],
        None => vec![(label.to_owned(), action_style)],
    }
}

/// Collapses an optional action status into the coarse display state.
/// `output_streaming` is whether the exchange output is still streaming;
/// a status-less action in a streaming output is still being constructed
/// (mirroring the GUI's `status.is_none() && is_streaming()` gating).
/// A resolved `block_state` supersedes the status for execution states
/// (see [`CommandBlockState`]).
pub(crate) fn tool_call_display_state(
    status: Option<&AIActionStatus>,
    output_streaming: bool,
    block_state: Option<CommandBlockState>,
) -> ToolCallDisplayState {
    // A block existing means the command actually started executing, so its
    // state is authoritative over the action status/result.
    match block_state {
        Some(CommandBlockState::Running) => return State::Running,
        Some(CommandBlockState::Finished { exit_code }) => {
            return if exit_code.is_sigint() {
                State::Cancelled
            } else if exit_code.was_successful() {
                State::Succeeded
            } else {
                State::Failed
            };
        }
        None => {}
    }
    match status {
        None if output_streaming => State::Constructing,
        None | Some(AIActionStatus::Preprocessing | AIActionStatus::Queued) => State::Pending,
        Some(AIActionStatus::Blocked) => State::Blocked,
        Some(AIActionStatus::RunningAsync) => State::Running,
        Some(finished @ AIActionStatus::Finished(_)) => {
            if finished.is_cancelled() {
                State::Cancelled
            } else if finished.is_failed() {
                State::Failed
            } else {
                State::Succeeded
            }
        }
    }
}

/// Returns the transcript label for a tool call in its current state.
///
/// Equivalent to [`tool_call_label_with_server`] with no MCP server name; use
/// that variant when rendering an MCP tool call whose originating server is
/// known so the label surfaces both the tool name and the server.
pub(crate) fn tool_call_label(
    action: &AIAgentAction,
    status: Option<&AIActionStatus>,
    output_streaming: bool,
    block: Option<&ResolvedCommandBlock>,
) -> String {
    tool_call_label_with_server(action, status, output_streaming, block, None)
}

/// Like [`tool_call_label`], but interpolates the MCP tool's originating server
/// name (when known) into the per-state label so MCP tool calls surface both
/// their tool name and server identity across the transcript lifecycle.
pub(crate) fn tool_call_label_with_server(
    action: &AIAgentAction,
    status: Option<&AIActionStatus>,
    output_streaming: bool,
    block: Option<&ResolvedCommandBlock>,
    server_name: Option<&str>,
) -> String {
    let state = tool_call_display_state(status, output_streaming, block.map(|block| block.state));
    let result = status
        .and_then(AIActionStatus::finished_result)
        .map(|result| &result.result);
    let label = label_for_action(&action.action, state, result, block, server_name);
    match state {
        State::Blocked => t!("{label} (awaiting approval)", label = label),
        State::Constructing
        | State::Pending
        | State::Running
        | State::Succeeded
        | State::Failed
        | State::Cancelled => label,
    }
}

/// Resolves the user-facing name of the originating MCP server for an MCP
/// tool-call action, for use in transcript labels. Returns `None` for non-
/// MCP-tool actions, legacy/flat calls with no server id, or unknown servers.
pub(crate) fn mcp_server_name_for_action(
    action: &AIAgentActionType,
    app: &AppContext,
) -> Option<String> {
    match action {
        AIAgentActionType::CallMCPTool { server_id, .. } => server_id
            .as_ref()
            .and_then(|id| mcp_server_name_for_id(id, app)),
        _ => None,
    }
}

/// Builds the per-tool label body; the awaiting-approval suffix is applied by
/// [`tool_call_label`]. `result` is the finished result, when there is one.
///
/// `Constructing` arms never interpolate argument fields (they may be empty
/// or partial while streaming); their copy is indexed on the GUI's loading
/// messages (`common.rs` `LOAD_OUTPUT_MESSAGE_*` and the requested-command
/// view's "Generating command...").
fn label_for_action(
    action: &AIAgentActionType,
    state: State,
    result: Option<&AIAgentActionResultType>,
    block: Option<&ResolvedCommandBlock>,
    server_name: Option<&str>,
) -> String {
    let block_state = block.map(|block| block.state);
    match action {
        AIAgentActionType::RequestCommandOutput { command, .. } => {
            // The streamed command can be edited before acceptance, so
            // prefer the executed command from the finished result or the
            // resolved block over the original suggestion.
            let executed = result
                .and_then(AIAgentActionResultType::command_str)
                .or_else(|| block.and_then(|block| block.command.as_deref()));
            // Shell-command headers wrap in `TuiShellCommandView`, so retain
            // the complete command instead of capping it at MAX_INLINE_LEN.
            let cmd = executed.unwrap_or(command).trim_end();
            match state {
                State::Constructing => t!("Generating command…").to_owned(),
                State::Pending | State::Blocked => t!("Run `{cmd}`", cmd = cmd),
                State::Running => t!("Running `{cmd}`", cmd = cmd),
                State::Succeeded => match block_state {
                    Some(CommandBlockState::Finished { .. }) => t!("Ran `{cmd}`", cmd = cmd),
                    // No local block: fall back to the stored result. A
                    // snapshot result means the command was still running at
                    // the last point we could observe it.
                    Some(CommandBlockState::Running) | None => match result {
                        Some(AIAgentActionResultType::RequestCommandOutput(
                            RequestCommandOutputResult::LongRunningCommandSnapshot { .. },
                        )) => t!("`{cmd}` is still running", cmd = cmd),
                        _ => t!("Ran `{cmd}`", cmd = cmd),
                    },
                },
                State::Failed => match block_state {
                    Some(CommandBlockState::Finished { exit_code }) => {
                        t!(
                            "`{cmd}` exited with code {code}",
                            cmd = cmd,
                            code = exit_code.value()
                        )
                    }
                    Some(CommandBlockState::Running) | None => match result {
                        Some(AIAgentActionResultType::RequestCommandOutput(
                            RequestCommandOutputResult::Completed { exit_code, .. },
                        )) => t!(
                            "`{cmd}` exited with code {code}",
                            cmd = cmd,
                            code = exit_code.value()
                        ),
                        Some(AIAgentActionResultType::RequestCommandOutput(
                            RequestCommandOutputResult::Denylisted { .. },
                        )) => t!("`{cmd}` denied (denylisted)", cmd = cmd),
                        Some(AIAgentActionResultType::RequestCommandOutput(
                            RequestCommandOutputResult::TerminalBusy { .. },
                        )) => t!("`{cmd}` not started (terminal busy)", cmd = cmd),
                        _ => t!("`{cmd}` failed", cmd = cmd),
                    },
                },
                State::Cancelled => t!("Cancelled `{cmd}`", cmd = cmd),
            }
        }
        AIAgentActionType::WriteToLongRunningShellCommand { .. } => match state {
            State::Constructing => t!("Writing command input…").to_owned(),
            State::Pending | State::Blocked => t!("Write input to running command").to_owned(),
            State::Running => t!("Writing input to running command…").to_owned(),
            State::Succeeded => t!("Wrote input to running command").to_owned(),
            State::Failed => t!("Failed to write to running command").to_owned(),
            State::Cancelled => t!("Write to running command cancelled").to_owned(),
        },
        AIAgentActionType::ReadFiles(request) => {
            let files = files_summary(request.locations.iter().map(|location| &location.name));
            match state {
                State::Constructing => t!("Reading files…").to_owned(),
                State::Pending | State::Blocked | State::Succeeded => {
                    t!("Read {files}", files = files)
                }
                State::Running => t!("Reading {files}", files = files),
                State::Failed => t!("Failed to read {files}", files = files),
                State::Cancelled => t!("Cancelled reading {files}", files = files),
            }
        }
        AIAgentActionType::SearchCodebase(request) => {
            let query = single_line(&request.query);
            let scope = request
                .codebase_path
                .as_deref()
                .map(|path| t!(" in {path}", path = base_name(path)))
                .unwrap_or_default();
            match state {
                State::Constructing => t!("Searching codebase…").to_owned(),
                State::Pending | State::Blocked => {
                    t!(
                        "Search for \"{query}\"{scope}",
                        query = query,
                        scope = scope
                    )
                }
                State::Running => t!(
                    "Searching for \"{query}\"{scope}",
                    query = query,
                    scope = scope
                ),
                State::Succeeded => match result {
                    Some(AIAgentActionResultType::SearchCodebase(
                        SearchCodebaseResult::Success { files },
                    )) if files.is_empty() => {
                        t!(
                            "Searched for \"{query}\"{scope}, no results",
                            query = query,
                            scope = scope
                        )
                    }
                    Some(AIAgentActionResultType::SearchCodebase(
                        SearchCodebaseResult::Success { files },
                    )) => t!(
                        "Searched for \"{query}\"{scope}, {results}",
                        query = query,
                        scope = scope,
                        results = count_label(files.len(), "1 result", "{count} results")
                    ),
                    _ => t!(
                        "Searched for \"{query}\"{scope}",
                        query = query,
                        scope = scope
                    ),
                },
                State::Failed => match result {
                    Some(AIAgentActionResultType::SearchCodebase(
                        SearchCodebaseResult::Failed {
                            reason: SearchCodebaseFailureReason::CodebaseNotIndexed,
                            ..
                        },
                    )) => t!(
                        "Search for \"{query}\"{scope} failed because the codebase isn't indexed",
                        query = query,
                        scope = scope
                    ),
                    _ => t!(
                        "Search for \"{query}\"{scope} failed",
                        query = query,
                        scope = scope
                    ),
                },
                State::Cancelled => t!(
                    "Search for \"{query}\"{scope} cancelled",
                    query = query,
                    scope = scope
                ),
            }
        }
        // Rendered by its own stateful child view (`TuiFileEditsView`); the
        // label path should never be reached for it.
        AIAgentActionType::RequestFileEdits { .. } => {
            log::warn!("tool_call_label called for RequestFileEdits, which has custom rendering");
            String::new()
        }
        AIAgentActionType::Grep { queries, path } => {
            let queries = single_line(&queries.join(", "));
            let path = display_path(path);
            match state {
                State::Constructing => t!("Grepping…").to_owned(),
                State::Pending | State::Blocked => {
                    t!(
                        "Grep for {queries} in {path}",
                        queries = queries,
                        path = path
                    )
                }
                State::Running => t!(
                    "Grepping for {queries} in {path}",
                    queries = queries,
                    path = path
                ),
                State::Succeeded => match result {
                    Some(AIAgentActionResultType::Grep(GrepResult::Success { matched_files })) => {
                        t!(
                            "Grepped for {queries} in {path}, {matches}",
                            queries = queries,
                            path = path,
                            matches = count_label(
                                matched_files.len(),
                                "1 matching file",
                                "{count} matching files"
                            )
                        )
                    }
                    _ => t!(
                        "Grepped for {queries} in {path}",
                        queries = queries,
                        path = path
                    ),
                },
                State::Failed => t!("Grep for {queries} failed", queries = queries),
                State::Cancelled => t!("Grep for {queries} cancelled", queries = queries),
            }
        }
        AIAgentActionType::FileGlob { patterns, path } => {
            file_glob_label(patterns, path.as_deref(), state, None)
        }
        AIAgentActionType::FileGlobV2 {
            patterns,
            search_dir,
        } => {
            let matched_count = match result {
                Some(AIAgentActionResultType::FileGlobV2(FileGlobV2Result::Success {
                    matched_files,
                    ..
                })) => Some(matched_files.len()),
                _ => None,
            };
            file_glob_label(patterns, search_dir.as_deref(), state, matched_count)
        }
        AIAgentActionType::ReadMCPResource { name, uri, .. } => {
            let resource = single_line(uri.as_deref().unwrap_or(name));
            match state {
                // The resource name arrives with the tool-call header (not
                // the streamed args), so include it when present, like the
                // GUI's "Reading \"{name}\" MCP resource..." loading text.
                State::Constructing if name.is_empty() => t!("Reading MCP resource…").to_owned(),
                State::Constructing => t!("Reading \"{name}\" MCP resource…", name = name),
                State::Pending | State::Blocked | State::Succeeded => {
                    t!("Read MCP resource {resource}", resource = resource)
                }
                State::Running => t!("Reading MCP resource {resource}", resource = resource),
                State::Failed => t!("MCP resource {resource} failed", resource = resource),
                State::Cancelled => t!("MCP resource {resource} cancelled", resource = resource),
            }
        }
        AIAgentActionType::CallMCPTool { name, .. } => {
            let name = single_line(name);
            // Append the originating server when known so MCP tool calls
            // surface both identities, with a deterministic no-server fallback.
            let suffix = server_name
                .map(|server| t!(" on {server}", server = server))
                .unwrap_or_default();
            match state {
                // Like the GUI's "Calling \"{name}\" MCP tool..." loading
                // text; the tool name is available before its args finish.
                State::Constructing if name.is_empty() => {
                    t!("Calling MCP tool{suffix}…", suffix = suffix)
                }
                State::Constructing => t!(
                    "Calling \"{name}\" MCP tool{suffix}…",
                    name = name,
                    suffix = suffix
                ),
                State::Pending | State::Blocked => {
                    t!("Call MCP tool {name}{suffix}", name = name, suffix = suffix)
                }
                State::Running => t!(
                    "Calling MCP tool {name}{suffix}",
                    name = name,
                    suffix = suffix
                ),
                State::Succeeded => t!(
                    "Called MCP tool {name}{suffix}",
                    name = name,
                    suffix = suffix
                ),
                State::Failed => t!(
                    "MCP tool {name}{suffix} failed",
                    name = name,
                    suffix = suffix
                ),
                State::Cancelled => t!(
                    "MCP tool {name}{suffix} cancelled",
                    name = name,
                    suffix = suffix
                ),
            }
        }
        AIAgentActionType::SuggestNewConversation { .. } => match state {
            State::Constructing => t!("Suggesting a new conversation…").to_owned(),
            State::Pending | State::Blocked | State::Running | State::Failed => {
                t!("Suggested starting a new conversation").to_owned()
            }
            State::Succeeded => match result {
                Some(AIAgentActionResultType::SuggestNewConversation(
                    SuggestNewConversationResult::Rejected,
                )) => t!("Continuing current conversation").to_owned(),
                _ => t!("New conversation started").to_owned(),
            },
            State::Cancelled => t!("New conversation suggestion cancelled").to_owned(),
        },
        AIAgentActionType::SuggestPrompt(_)
        | AIAgentActionType::InitProject
        | AIAgentActionType::OpenCodeReview => fallback_label(action, state),
        AIAgentActionType::ReadDocuments(request) => {
            let documents = count_label(
                request.document_ids.len(),
                "1 document",
                "{count} documents",
            );
            match state {
                State::Constructing => t!("Reading documents…").to_owned(),
                State::Pending | State::Blocked | State::Succeeded => {
                    t!("Read {documents}", documents = documents)
                }
                State::Running => t!("Reading {documents}", documents = documents),
                State::Failed => t!("Failed to read documents").to_owned(),
                State::Cancelled => t!("Cancelled reading documents").to_owned(),
            }
        }
        AIAgentActionType::EditDocuments(request) => match state {
            State::Pending | State::Blocked => t!("Update plan").to_owned(),
            State::Constructing | State::Running => t!("Updating plan…").to_owned(),
            State::Succeeded => t!(
                "Updated plan ({edits})",
                edits = count_label(request.diffs.len(), "1 edit", "{count} edits")
            ),
            State::Failed => t!("Failed to update plan").to_owned(),
            State::Cancelled => t!("Update plan cancelled").to_owned(),
        },
        AIAgentActionType::CreateDocuments(request) => match state {
            State::Pending | State::Blocked => t!("Create plan").to_owned(),
            State::Constructing | State::Running => t!("Generating plan…").to_owned(),
            State::Succeeded => {
                let count = request.documents.len();
                if count > 1 {
                    t!("Created {count} documents", count = count)
                } else {
                    t!("Created plan").to_owned()
                }
            }
            State::Failed => t!("Failed to create plan").to_owned(),
            State::Cancelled => t!("Create plan cancelled").to_owned(),
        },
        AIAgentActionType::ReadShellCommandOutput { .. } => match state {
            State::Pending | State::Blocked | State::Succeeded => {
                t!("Read command output").to_owned()
            }
            State::Constructing | State::Running => t!("Reading command output…").to_owned(),
            State::Failed => t!("Failed to read command output").to_owned(),
            State::Cancelled => t!("Read command output cancelled").to_owned(),
        },
        AIAgentActionType::UseComputer(request) => summary_label(&request.action_summary, state),
        AIAgentActionType::InsertCodeReviewComments { comments, .. } => {
            let comments = count_label(
                comments.len(),
                "1 review comment",
                "{count} review comments",
            );
            match state {
                State::Constructing => t!("Preparing review comments…").to_owned(),
                State::Pending | State::Blocked => t!("Insert {comments}", comments = comments),
                State::Running => t!("Inserting {comments}…", comments = comments),
                State::Succeeded => t!("Inserted {comments}", comments = comments),
                State::Failed => t!("Failed to insert review comments").to_owned(),
                State::Cancelled => t!("Insert review comments cancelled").to_owned(),
            }
        }
        AIAgentActionType::RequestComputerUse(request) => {
            summary_label(&request.task_summary, state)
        }
        AIAgentActionType::StartRecording { .. } => match state {
            State::Pending | State::Blocked => t!("Start recording").to_owned(),
            State::Constructing | State::Running => t!("Starting recording…").to_owned(),
            State::Succeeded => t!("Started screen recording").to_owned(),
            State::Failed => t!("Recording failed to start").to_owned(),
            State::Cancelled => t!("Start recording cancelled").to_owned(),
        },
        AIAgentActionType::StopRecording { .. } => match state {
            State::Pending | State::Blocked => t!("Stop recording").to_owned(),
            State::Constructing | State::Running => t!("Stopping recording…").to_owned(),
            State::Succeeded => match result {
                Some(AIAgentActionResultType::StopRecording(StopRecordingResult::Discarded)) => {
                    t!("Discarded screen recording").to_owned()
                }
                _ => t!("Saved screen recording").to_owned(),
            },
            State::Failed => t!("Failed to save recording").to_owned(),
            State::Cancelled => t!("Stop recording cancelled").to_owned(),
        },
        AIAgentActionType::ReadSkill(request) => {
            let skill = single_line(&request.skill.display_label());
            match state {
                State::Constructing => t!("Reading skill…").to_owned(),
                State::Pending | State::Blocked | State::Succeeded => {
                    t!("Read skill {skill}", skill = skill)
                }
                State::Running => t!("Reading skill {skill}", skill = skill),
                State::Failed => t!("Failed to read skill {skill}", skill = skill),
                State::Cancelled => t!("Cancelled reading skill {skill}", skill = skill),
            }
        }
        AIAgentActionType::FetchConversation { .. } => match state {
            State::Pending | State::Blocked => t!("Fetch conversation").to_owned(),
            State::Constructing | State::Running => t!("Fetching conversation…").to_owned(),
            State::Succeeded => t!("Fetched conversation").to_owned(),
            State::Failed => t!("Fetch conversation failed").to_owned(),
            State::Cancelled => t!("Fetch conversation cancelled").to_owned(),
        },
        AIAgentActionType::TransferShellCommandControlToUser { reason } => match state {
            State::Constructing => t!("Handing control to you…").to_owned(),
            State::Pending | State::Blocked | State::Running => {
                t!(
                    "Handing control to you: {reason}",
                    reason = single_line(reason)
                )
            }
            State::Succeeded => t!("You are in control").to_owned(),
            State::Failed => t!("Control transfer failed").to_owned(),
            State::Cancelled => t!("Control transfer cancelled").to_owned(),
        },
        AIAgentActionType::AskUserQuestion { questions } => match state {
            State::Constructing => t!("Preparing question…").to_owned(),
            State::Pending | State::Blocked | State::Running => t!(
                "Asking {questions}",
                questions = count_label(questions.len(), "1 question", "{count} questions")
            ),
            State::Succeeded => match result {
                Some(AIAgentActionResultType::AskUserQuestion(
                    AskUserQuestionResult::Success { answers },
                )) => {
                    let total = answers.len();
                    let answered = answers.iter().filter(|answer| !answer.is_skipped()).count();
                    if answered == 0 {
                        t!("Questions skipped").to_owned()
                    } else if answered == total && total == 1 {
                        t!("Answered question").to_owned()
                    } else if answered == total {
                        t!("Answered all {total} questions", total = total)
                    } else {
                        t!(
                            "Answered {answered} of {total} questions",
                            answered = answered,
                            total = total
                        )
                    }
                }
                Some(AIAgentActionResultType::AskUserQuestion(
                    AskUserQuestionResult::SkippedByAutoApprove { .. },
                )) => t!("Questions skipped").to_owned(),
                _ => t!("Answered questions").to_owned(),
            },
            State::Failed => t!("Questions failed").to_owned(),
            State::Cancelled => t!("Questions cancelled").to_owned(),
        },
    }
}

/// Shared label body for both file-glob action versions; only V2 results
/// carry a match count.
fn file_glob_label(
    patterns: &[String],
    path: Option<&str>,
    state: State,
    matched_count: Option<usize>,
) -> String {
    let patterns = single_line(&patterns.join(", "));
    let path = display_path(path.unwrap_or("."));
    match state {
        State::Constructing => t!("Finding files…").to_owned(),
        State::Pending | State::Blocked => {
            t!(
                "Find files matching {patterns} in {path}",
                patterns = patterns,
                path = path
            )
        }
        State::Running => t!(
            "Finding files matching {patterns} in {path}",
            patterns = patterns,
            path = path
        ),
        State::Succeeded => match matched_count {
            Some(count) => t!(
                "Found {files} matching {patterns}",
                files = count_label(count, "1 file", "{count} files"),
                patterns = patterns
            ),
            None => t!("Found files matching {patterns}", patterns = patterns),
        },
        State::Failed => t!("File search for {patterns} failed", patterns = patterns),
        State::Cancelled => t!("File search for {patterns} cancelled", patterns = patterns),
    }
}

/// Labels computer-use calls with their agent-supplied summary, marking only
/// terminal non-success states (matching the GUI, which shows the summary
/// verbatim).
fn summary_label(summary: &str, state: State) -> String {
    let summary = single_line(summary);
    match state {
        State::Constructing => t!("Preparing computer use…").to_owned(),
        State::Pending | State::Blocked | State::Running | State::Succeeded => summary,
        State::Failed => t!("{summary} — failed", summary = summary),
        State::Cancelled => t!("{summary} — cancelled", summary = summary),
    }
}

/// Generic label for action types without bespoke text, derived from the
/// action's user-friendly name.
fn fallback_label(action: &AIAgentActionType, state: State) -> String {
    let name = action.user_friendly_name();
    match state {
        State::Pending | State::Blocked => name,
        State::Constructing | State::Running => format!("{name}…"),
        State::Succeeded => t!("{name} — done", name = name),
        State::Failed => t!("{name} — failed", name = name),
        State::Cancelled => t!("{name} — cancelled", name = name),
    }
}

/// Collapses text to its first line, capped at [`MAX_INLINE_LEN`] chars, with
/// a trailing `…` when anything was trimmed.
fn single_line(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or_default().trim_end();
    let mut out: String = first_line.chars().take(MAX_INLINE_LEN).collect();
    if first_line.chars().count() > MAX_INLINE_LEN || text.lines().count() > 1 {
        out.push('…');
    }
    out
}

/// Renders a search path for display, mirroring the GUI's treatment of `.`.
fn display_path(path: &str) -> String {
    if path == "." {
        t!("the current directory").to_owned()
    } else {
        single_line(path)
    }
}

/// Returns the final path component, falling back to the input when there is none.
fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Summarizes file paths as comma-joined base names for up to 3 files, else a count.
fn files_summary<'a>(paths: impl ExactSizeIterator<Item = &'a String>) -> String {
    if paths.len() > 3 {
        return count_label(paths.len(), "1 file", "{count} files");
    }
    let names: Vec<String> = paths.map(|path| base_name(path)).collect();
    if names.is_empty() {
        t!("files").to_owned()
    } else {
        names.join(", ")
    }
}

/// Translates the label for a counted noun: `one` when `count` is 1, else `many` with its
/// `{count}` placeholder filled, e.g. `count_label(2, "1 file", "{count} files")` → "2 files".
fn count_label(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        t!(one).to_owned()
    } else {
        t!(many, count = count)
    }
}

#[cfg(test)]
#[path = "tool_call_labels_tests.rs"]
mod tests;
