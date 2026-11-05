use std::path::Path;

use serde_json::{Value, json};
use warp_multi_agent_api::message::tool_call::apply_file_diffs::FileDiff;
use warp_multi_agent_api::message::tool_call::read_files::File;
use warp_multi_agent_api::message::tool_call::{
    ApplyFileDiffs, ReadFiles, ReadShellCommandOutput, RunShellCommand, Tool,
    read_shell_command_output,
};
use warp_multi_agent_api::message::tool_call_result::Result as ToolResult;
use warp_multi_agent_api::request::input::tool_call_result::Result as InputToolResult;
use warp_multi_agent_api::{
    LongRunningShellCommandSnapshot, ShellCommandFinished, any_file_content,
    apply_file_diffs_result, read_files_result, read_shell_command_output_result,
    run_shell_command_result,
};

use crate::ollama::{ChatFunction, ChatToolCall};

const RUN_SHELL_COMMAND: &str = "run_shell_command";
const READ_FILE: &str = "read_file";
const EDIT_FILE: &str = "edit_file";
const READ_SHELL_OUTPUT: &str = "read_shell_output";

/// Longest tool output kept in the prompt. Small context windows can't absorb a full build log.
const MAX_TOOL_OUTPUT_CHARS: usize = 16_000;

pub(crate) fn definitions() -> Vec<Value> {
    vec![
        definition(
            RUN_SHELL_COMMAND,
            "Run a shell command and return its output.",
            &[("command", "The command to run.")],
        ),
        definition(
            READ_FILE,
            "Read a text file.",
            &[("path", "Path of the file to read.")],
        ),
        definition(
            EDIT_FILE,
            "Replace an exact piece of text in an existing file.",
            &[
                ("path", "Path of the file to edit."),
                ("old_text", "Exact text to replace, copied from the file."),
                ("new_text", "Text to put in its place."),
            ],
        ),
    ]
}

fn definition(name: &str, description: &str, params: &[(&str, &str)]) -> Value {
    let properties: serde_json::Map<String, Value> = params
        .iter()
        .map(|(param, about)| {
            (
                (*param).to_owned(),
                json!({ "type": "string", "description": about }),
            )
        })
        .collect();
    let required: Vec<&str> = params.iter().map(|(param, _)| *param).collect();
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": { "type": "object", "properties": properties, "required": required },
        },
    })
}

/// Parses a model's tool call, returning a message for the model when the call is unusable.
/// `cwd` resolves relative paths.
pub(crate) fn parse_tool_call(call: &ChatFunction, cwd: Option<&Path>) -> Result<Tool, String> {
    match call.name.as_str() {
        RUN_SHELL_COMMAND => Ok(Tool::RunShellCommand(RunShellCommand {
            command: string_arg(call, "command")?,
            // Commands always go through the user's permission settings: the model must never
            // get to declare its own command safe.
            is_read_only: false,
            is_risky: true,
            ..Default::default()
        })),
        READ_FILE => Ok(Tool::ReadFiles(ReadFiles {
            files: vec![File {
                name: string_arg(call, "path")?,
                line_ranges: Vec::new(),
            }],
        })),
        EDIT_FILE => {
            let file_path = string_arg(call, "path")?;
            let old_text = string_arg(call, "old_text")?;
            let new_text = string_arg(call, "new_text")?;
            let (search, replace) = read_file(&file_path, cwd)
                .and_then(|content| whole_line_edit(&content, &old_text, &new_text))
                .unwrap_or((old_text, new_text));
            Ok(Tool::ApplyFileDiffs(ApplyFileDiffs {
                summary: format!("Edit {file_path}"),
                diffs: vec![FileDiff {
                    file_path,
                    search,
                    replace,
                }],
                ..Default::default()
            }))
        }
        unknown => Err(format!("Unknown tool `{unknown}`.")),
    }
}

fn read_file(path: &str, cwd: Option<&Path>) -> Option<String> {
    let path = Path::new(path);
    let path = match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_owned(),
    };
    std::fs::read_to_string(path).ok()
}

/// Widens a text replacement to the whole lines containing it, because the client matches diffs
/// line by line and models naturally quote just the words they want to change. Returns `None`
/// unless `old_text` occurs exactly once.
fn whole_line_edit(content: &str, old_text: &str, new_text: &str) -> Option<(String, String)> {
    let first_char_len = old_text.chars().next()?.len_utf8();
    let start = content.find(old_text)?;
    let end = start + old_text.len();
    if content[start + first_char_len..].contains(old_text) {
        return None;
    }
    let line_start = content[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = content[end..]
        .find('\n')
        .map_or(content.len(), |index| end + index);
    let search = content[line_start..line_end].to_owned();
    let replace = format!(
        "{}{new_text}{}",
        &content[line_start..start],
        &content[end..line_end]
    );
    Some((search, replace))
}

fn string_arg(call: &ChatFunction, name: &str) -> Result<String, String> {
    call.arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Tool `{}` needs a string argument `{name}`.", call.name))
}

/// Rebuilds the model-facing call for a tool stored in the conversation history.
pub(crate) fn chat_call_for_tool(tool: &Tool) -> Option<ChatToolCall> {
    let (name, arguments) = match tool {
        Tool::RunShellCommand(shell) => (RUN_SHELL_COMMAND, json!({ "command": shell.command })),
        Tool::ReadFiles(read) => (
            READ_FILE,
            json!({ "path": read.files.first().map(|file| file.name.as_str()) }),
        ),
        Tool::ApplyFileDiffs(edit) => {
            let diff = edit.diffs.first()?;
            (
                EDIT_FILE,
                json!({
                    "path": diff.file_path,
                    "old_text": diff.search,
                    "new_text": diff.replace,
                }),
            )
        }
        Tool::ReadShellCommandOutput(read) => {
            (READ_SHELL_OUTPUT, json!({ "command_id": read.command_id }))
        }
        // The engine only ever emits the tools above.
        _ => return None,
    };
    Some(ChatToolCall {
        function: ChatFunction {
            name: name.to_owned(),
            arguments,
        },
    })
}

/// Converts a request-side tool result to its history representation. Results of tools the engine
/// doesn't offer have no history form.
pub(crate) fn history_result(result: InputToolResult) -> Option<ToolResult> {
    match result {
        InputToolResult::RunShellCommand(result) => Some(ToolResult::RunShellCommand(result)),
        InputToolResult::ReadFiles(result) => Some(ToolResult::ReadFiles(result)),
        InputToolResult::ApplyFileDiffs(result) => Some(ToolResult::ApplyFileDiffs(result)),
        InputToolResult::ReadShellCommandOutput(result) => {
            Some(ToolResult::ReadShellCommandOutput(result))
        }
        _ => None,
    }
}

pub(crate) fn result_text(result: Option<&ToolResult>) -> String {
    let text = match result {
        Some(ToolResult::RunShellCommand(shell)) => shell_result_text(shell),
        Some(ToolResult::ReadFiles(read)) => read_result_text(read),
        Some(ToolResult::ApplyFileDiffs(edit)) => match &edit.result {
            Some(apply_file_diffs_result::Result::Success(_)) => "Edit applied.".to_owned(),
            Some(apply_file_diffs_result::Result::Error(error)) => {
                format!("Edit failed: {}", error.message)
            }
            None => "Edit finished with no result.".to_owned(),
        },
        Some(ToolResult::ReadShellCommandOutput(read)) => match &read.result {
            Some(read_shell_command_output_result::Result::CommandFinished(finished)) => {
                finished_text(finished)
            }
            Some(read_shell_command_output_result::Result::LongRunningCommandSnapshot(
                snapshot,
            )) => running_text(snapshot),
            Some(read_shell_command_output_result::Result::Error(_)) | None => {
                "The command's output could not be read.".to_owned()
            }
        },
        Some(ToolResult::Cancel(_)) => "The user cancelled this tool call.".to_owned(),
        _ => "The tool returned no result.".to_owned(),
    };
    truncate_middle(text)
}

fn shell_result_text(shell: &warp_multi_agent_api::RunShellCommandResult) -> String {
    match &shell.result {
        Some(run_shell_command_result::Result::CommandFinished(finished)) => {
            finished_text(finished)
        }
        Some(run_shell_command_result::Result::LongRunningCommandSnapshot(snapshot)) => {
            running_text(snapshot)
        }
        Some(run_shell_command_result::Result::PermissionDenied(_)) => {
            "The command was denied permission to run.".to_owned()
        }
        Some(run_shell_command_result::Result::TerminalBusy(_)) => {
            "The terminal is busy, so the command was not started.".to_owned()
        }
        None => "The command finished with no result.".to_owned(),
    }
}

fn finished_text(finished: &ShellCommandFinished) -> String {
    format!("Exit code {}\n{}", finished.exit_code, finished.output)
}

fn running_text(snapshot: &LongRunningShellCommandSnapshot) -> String {
    format!(
        "The command is still running. Output so far:\n{}",
        snapshot.output
    )
}

/// The tool call that waits for a command the client reported as still running. Used for the
/// first report only, so a command that never ends (a server) reaches the model. Full-screen
/// programs only end on input, so they go straight to the model.
pub(crate) fn wait_for_command(result: &ToolResult) -> Option<Tool> {
    let ToolResult::RunShellCommand(shell) = result else {
        return None;
    };
    let Some(run_shell_command_result::Result::LongRunningCommandSnapshot(snapshot)) =
        &shell.result
    else {
        return None;
    };
    if snapshot.is_alt_screen_active {
        return None;
    }
    Some(Tool::ReadShellCommandOutput(ReadShellCommandOutput {
        command_id: snapshot.command_id.clone(),
        delay: Some(read_shell_command_output::Delay::OnCompletion(())),
    }))
}

fn read_result_text(read: &warp_multi_agent_api::ReadFilesResult) -> String {
    match &read.result {
        Some(read_files_result::Result::TextFilesSuccess(success)) => success
            .files
            .iter()
            .map(|file| format!("{}:\n{}", file.file_path, file.content))
            .chain(failed_reads_text(&success.failed_reads))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(read_files_result::Result::AnyFilesSuccess(success)) => success
            .files
            .iter()
            .map(|file| match &file.content {
                Some(any_file_content::Content::TextContent(text)) => {
                    format!("{}:\n{}", text.file_path, text.content)
                }
                Some(any_file_content::Content::BinaryContent(binary)) => {
                    format!("{} is a binary file and can't be shown.", binary.file_path)
                }
                None => "A file had no content.".to_owned(),
            })
            .chain(failed_reads_text(&success.failed_reads))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(read_files_result::Result::Error(error)) => {
            format!("Could not read the file: {}", error.message)
        }
        None => "The read finished with no result.".to_owned(),
    }
}

fn failed_reads_text(
    failed_reads: &[read_files_result::FailedRead],
) -> impl Iterator<Item = String> {
    failed_reads
        .iter()
        .map(|failed| format!("Could not read {}: {}", failed.path, failed.message))
}

/// Keeps the head and tail of long output, where commands report what they did and how it ended.
fn truncate_middle(text: String) -> String {
    if text.len() <= MAX_TOOL_OUTPUT_CHARS {
        return text;
    }
    let half = MAX_TOOL_OUTPUT_CHARS / 2;
    let head = text.floor_char_boundary(half);
    let tail = text.ceil_char_boundary(text.len() - half);
    format!("{}\n[output truncated]\n{}", &text[..head], &text[tail..])
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
