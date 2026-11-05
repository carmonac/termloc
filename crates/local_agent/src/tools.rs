use std::path::Path;

use serde_json::{Value, json};
use warp_multi_agent_api::message::tool_call::apply_file_diffs::{FileDiff, NewFile};
use warp_multi_agent_api::message::tool_call::read_files::File;
use warp_multi_agent_api::message::tool_call::{
    ApplyFileDiffs, FileGlobV2, Grep, ReadFiles, ReadShellCommandOutput, RunShellCommand,
    SearchCodebase, Tool, read_shell_command_output,
};
use warp_multi_agent_api::message::tool_call_result::Result as ToolResult;
use warp_multi_agent_api::request::input::tool_call_result::Result as InputToolResult;
use warp_multi_agent_api::{
    AskUserQuestion, AskUserQuestionResult, CallMcpToolResult, FileContentLineRange,
    FileGlobV2Result, GrepResult, LongRunningShellCommandSnapshot, Request, SearchCodebaseResult,
    ShellCommandFinished, ToolType, any_file_content, apply_file_diffs_result, ask_user_question,
    ask_user_question_result, call_mcp_tool_result, file_glob_v2_result, grep_result,
    read_files_result, read_shell_command_output_result, run_shell_command_result,
    search_codebase_result,
};

use crate::events;
use crate::mcp::{self, McpToolEntry};
use crate::ollama::{ChatFunction, ChatToolCall};

const RUN_SHELL_COMMAND: &str = "run_shell_command";
const READ_FILE: &str = "read_file";
const EDIT_FILE: &str = "edit_file";
const CREATE_FILE: &str = "create_file";
const GREP: &str = "grep";
const GLOB: &str = "glob";
const SEARCH_CODEBASE: &str = "search_codebase";
const READ_SHELL_OUTPUT: &str = "read_shell_output";
const ASK_USER: &str = "ask_user";

/// How long to let a running command work before reporting on it. Short enough that a command
/// waiting for input doesn't stall the turn, long enough to finish most builds' next step.
const SHELL_OUTPUT_WAIT_SECONDS: i64 = 15;
const MAX_GLOB_MATCHES: i32 = 200;

/// Longest tool output kept in the prompt. Small context windows can't absorb a full build log.
const MAX_TOOL_OUTPUT_CHARS: usize = 16_000;

/// The tools a turn may use: what the client supports plus any configured MCP tools.
pub(crate) struct Toolbox {
    supported: Vec<i32>,
    mcp_tools: Vec<McpToolEntry>,
}

impl Toolbox {
    pub(crate) fn for_request(request: &Request) -> Self {
        Self {
            supported: request
                .settings
                .as_ref()
                .map(|settings| settings.supported_tools.clone())
                .unwrap_or_default(),
            mcp_tools: mcp::entries(request.mcp_context.as_ref()),
        }
    }

    /// A request that lists no tools places no restriction.
    fn supports(&self, tool_type: ToolType) -> bool {
        self.supported.is_empty() || self.supported.contains(&(tool_type as i32))
    }

    pub(crate) fn definitions(&self) -> Vec<Value> {
        let mut definitions: Vec<Value> = builtin_definitions()
            .into_iter()
            .filter(|(tool_type, _)| self.supports(*tool_type))
            .map(|(_, definition)| definition)
            .collect();
        if self.supports(ToolType::CallMcpTool) {
            definitions.extend(self.mcp_tools.iter().map(McpToolEntry::definition));
        }
        definitions
    }

    pub(crate) fn parse(&self, call: &ChatFunction, cwd: Option<&Path>) -> Result<Tool, String> {
        match self.mcp_tools.iter().find(|tool| tool.matches(call)) {
            Some(tool) => Ok(tool.tool_call(call)),
            None => parse_tool_call(call, cwd),
        }
    }
}

fn builtin_definitions() -> Vec<(ToolType, Value)> {
    vec![
        (
            ToolType::RunShellCommand,
            definition(
                RUN_SHELL_COMMAND,
                "Run a shell command and return its output.",
                &[("command", "The command to run.")],
                &[],
            ),
        ),
        (
            ToolType::ReadFiles,
            definition(
                READ_FILE,
                "Read a text file, optionally only some of its lines.",
                &[("path", "Path of the file to read.")],
                &[
                    ("start_line", "First line to read, starting at 1."),
                    ("end_line", "Last line to read."),
                ],
            ),
        ),
        (
            ToolType::Grep,
            definition(
                GREP,
                "Find the files and line numbers that match a pattern.",
                &[("pattern", "Text or regular expression to search for.")],
                &[(
                    "path",
                    "Directory to search. Defaults to the working directory.",
                )],
            ),
        ),
        (
            ToolType::SearchCodebase,
            definition(
                SEARCH_CODEBASE,
                "Find code by what it does, in the repository of the working directory. Use it \
                 when you don't know the exact text to grep for.",
                &[("query", "What the code does, in plain words.")],
                &[],
            ),
        ),
        (
            ToolType::FileGlobV2,
            definition(
                GLOB,
                "List files whose path matches a glob pattern such as src/**/*.rs.",
                &[("pattern", "The glob pattern.")],
                &[(
                    "path",
                    "Directory to search. Defaults to the working directory.",
                )],
            ),
        ),
        (
            ToolType::ApplyFileDiffs,
            definition(
                EDIT_FILE,
                "Replace an exact piece of text in an existing file.",
                &[
                    ("path", "Path of the file to edit."),
                    ("old_text", "Exact text to replace, copied from the file."),
                    ("new_text", "Text to put in its place."),
                ],
                &[],
            ),
        ),
        (
            ToolType::ApplyFileDiffs,
            definition(
                CREATE_FILE,
                "Create a new file.",
                &[
                    ("path", "Path of the file to create."),
                    ("content", "Full content of the file."),
                ],
                &[],
            ),
        ),
        (
            ToolType::ReadShellCommandOutput,
            definition(
                READ_SHELL_OUTPUT,
                "Check on a command that is still running.",
                &[(
                    "command_id",
                    "The id reported when the command kept running.",
                )],
                &[],
            ),
        ),
        (ToolType::AskUserQuestion, ask_user_definition()),
    ]
}

fn definition(
    name: &str,
    description: &str,
    required: &[(&str, &str)],
    optional: &[(&str, &str)],
) -> Value {
    let properties: serde_json::Map<String, Value> = required
        .iter()
        .chain(optional)
        .map(|(param, about)| {
            (
                (*param).to_owned(),
                json!({ "type": "string", "description": about }),
            )
        })
        .collect();
    let required: Vec<&str> = required.iter().map(|(param, _)| *param).collect();
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": { "type": "object", "properties": properties, "required": required },
        },
    })
}

fn ask_user_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": ASK_USER,
            "description": "Ask the user a question when you can't continue without an answer.",
            "parameters": {
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question." },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Suggested answers. The user can also write their own.",
                    },
                },
                "required": ["question", "options"],
            },
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
                line_ranges: line_range(call).into_iter().collect(),
            }],
        })),
        GREP => Ok(Tool::Grep(Grep {
            queries: vec![string_arg(call, "pattern")?],
            path: optional_path(call),
        })),
        SEARCH_CODEBASE => Ok(Tool::SearchCodebase(SearchCodebase {
            query: string_arg(call, "query")?,
            ..Default::default()
        })),
        GLOB => Ok(Tool::FileGlobV2(FileGlobV2 {
            patterns: vec![string_arg(call, "pattern")?],
            search_dir: optional_path(call),
            max_matches: MAX_GLOB_MATCHES,
            ..Default::default()
        })),
        CREATE_FILE => {
            let file_path = string_arg(call, "path")?;
            Ok(Tool::ApplyFileDiffs(ApplyFileDiffs {
                summary: format!("Create {file_path}"),
                new_files: vec![NewFile {
                    file_path,
                    content: string_arg(call, "content")?,
                    allow_overwrite: false,
                }],
                ..Default::default()
            }))
        }
        READ_SHELL_OUTPUT => Ok(Tool::ReadShellCommandOutput(ReadShellCommandOutput {
            command_id: string_arg(call, "command_id")?,
            delay: Some(shell_output_wait()),
        })),
        ASK_USER => Ok(Tool::AskUserQuestion(ask_user_question_tool(call)?)),
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

fn shell_output_wait() -> read_shell_command_output::Delay {
    read_shell_command_output::Delay::Duration(prost_types::Duration {
        seconds: SHELL_OUTPUT_WAIT_SECONDS,
        nanos: 0,
    })
}

/// Whether calling a tool again with the same arguments is legitimate progress.
pub(crate) fn may_repeat(call: &ChatToolCall) -> bool {
    call.function.name == READ_SHELL_OUTPUT
}

fn ask_user_question_tool(call: &ChatFunction) -> Result<AskUserQuestion, String> {
    let options = call
        .arguments
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(Value::as_str)
                .map(|label| ask_user_question::Option {
                    label: label.to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(AskUserQuestion {
        questions: vec![ask_user_question::Question {
            question_id: events::new_id(),
            question: string_arg(call, "question")?,
            question_type: Some(ask_user_question::question::QuestionType::MultipleChoice(
                ask_user_question::MultipleChoice {
                    options,
                    recommended_option_index: -1,
                    is_multiselect: false,
                    supports_other: true,
                },
            )),
        }],
    })
}

fn line_range(call: &ChatFunction) -> Option<FileContentLineRange> {
    let start = line_arg(call, "start_line");
    let end = line_arg(call, "end_line");
    if start.is_none() && end.is_none() {
        return None;
    }
    Some(FileContentLineRange {
        start: start.unwrap_or(1),
        end: end.unwrap_or(u32::MAX),
    })
}

/// Models send numbers either way, whatever the schema says.
fn line_arg(call: &ChatFunction, name: &str) -> Option<u32> {
    let value = call.arguments.get(name)?;
    value
        .as_u64()
        .and_then(|line| u32::try_from(line).ok())
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn optional_path(call: &ChatFunction) -> String {
    call.arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .unwrap_or(".")
        .to_owned()
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
        Tool::RunShellCommand(shell) => (
            RUN_SHELL_COMMAND.to_owned(),
            json!({ "command": shell.command }),
        ),
        Tool::ReadFiles(read) => (
            READ_FILE.to_owned(),
            json!({ "path": read.files.first().map(|file| file.name.as_str()) }),
        ),
        Tool::ApplyFileDiffs(edit) => match (edit.new_files.first(), edit.diffs.first()) {
            (Some(file), _) => (
                CREATE_FILE.to_owned(),
                json!({ "path": file.file_path, "content": file.content }),
            ),
            (None, Some(diff)) => (
                EDIT_FILE.to_owned(),
                json!({
                    "path": diff.file_path,
                    "old_text": diff.search,
                    "new_text": diff.replace,
                }),
            ),
            (None, None) => return None,
        },
        Tool::Grep(grep) => (
            GREP.to_owned(),
            json!({ "pattern": grep.queries.first(), "path": grep.path }),
        ),
        Tool::SearchCodebase(search) => {
            (SEARCH_CODEBASE.to_owned(), json!({ "query": search.query }))
        }
        Tool::FileGlobV2(glob) => (
            GLOB.to_owned(),
            json!({ "pattern": glob.patterns.first(), "path": glob.search_dir }),
        ),
        Tool::ReadShellCommandOutput(read) => (
            READ_SHELL_OUTPUT.to_owned(),
            json!({ "command_id": read.command_id }),
        ),
        Tool::AskUserQuestion(ask) => {
            let question = ask.questions.first()?;
            (
                ASK_USER.to_owned(),
                json!({ "question": question.question }),
            )
        }
        Tool::CallMcpTool(call) => (
            mcp::chat_name(&call.name),
            call.args
                .as_ref()
                .map_or_else(|| json!({}), mcp::struct_to_json),
        ),
        // The engine only ever emits the tools above.
        _ => return None,
    };
    Some(ChatToolCall {
        function: ChatFunction { name, arguments },
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
        InputToolResult::Grep(result) => Some(ToolResult::Grep(result)),
        InputToolResult::FileGlobV2(result) => Some(ToolResult::FileGlobV2(result)),
        InputToolResult::SearchCodebase(result) => Some(ToolResult::SearchCodebase(result)),
        InputToolResult::CallMcpTool(result) => Some(ToolResult::CallMcpTool(result)),
        InputToolResult::AskUserQuestion(result) => Some(ToolResult::AskUserQuestion(result)),
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
        Some(ToolResult::Grep(grep)) => grep_text(grep),
        Some(ToolResult::FileGlobV2(glob)) => glob_text(glob),
        Some(ToolResult::SearchCodebase(search)) => search_codebase_text(search),
        Some(ToolResult::CallMcpTool(call)) => mcp_text(call),
        Some(ToolResult::AskUserQuestion(ask)) => ask_user_text(ask),
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
        "The command is still running with command_id {}. If it is waiting for input, tell the \
         user what to type in the terminal; you can't type into it. Output so far:\n{}",
        snapshot.command_id, snapshot.output
    )
}

fn search_codebase_text(search: &SearchCodebaseResult) -> String {
    match &search.result {
        Some(search_codebase_result::Result::Success(success)) if success.files.is_empty() => {
            "No relevant code found.".to_owned()
        }
        Some(search_codebase_result::Result::Success(success)) => success
            .files
            .iter()
            .map(|file| format!("{}\n{}", file.file_path, file.content))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(search_codebase_result::Result::Error(error)) => {
            format!("Search failed: {}", error.message)
        }
        None => "The search finished with no result.".to_owned(),
    }
}

fn grep_text(grep: &GrepResult) -> String {
    match &grep.result {
        Some(grep_result::Result::Success(success)) if success.matched_files.is_empty() => {
            "No matches.".to_owned()
        }
        Some(grep_result::Result::Success(success)) => success
            .matched_files
            .iter()
            .map(|file| {
                let lines: Vec<String> = file
                    .matched_lines
                    .iter()
                    .map(|line| line.line_number.to_string())
                    .collect();
                format!("{}: lines {}", file.file_path, lines.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(grep_result::Result::Error(error)) => format!("Search failed: {}", error.message),
        None => "The search finished with no result.".to_owned(),
    }
}

fn glob_text(glob: &FileGlobV2Result) -> String {
    match &glob.result {
        Some(file_glob_v2_result::Result::Success(success)) if success.matched_files.is_empty() => {
            "No matching files.".to_owned()
        }
        Some(file_glob_v2_result::Result::Success(success)) => success
            .matched_files
            .iter()
            .map(|file| file.file_path.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(file_glob_v2_result::Result::Error(error)) => {
            format!("Search failed: {}", error.message)
        }
        None => "The search finished with no result.".to_owned(),
    }
}

fn mcp_text(call: &CallMcpToolResult) -> String {
    use call_mcp_tool_result::success::result::Result as McpContent;
    match &call.result {
        Some(call_mcp_tool_result::Result::Success(success)) => success
            .results
            .iter()
            .map(|result| match &result.result {
                Some(McpContent::Text(text)) => text.text.clone(),
                _ => "[content that can't be shown]".to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(call_mcp_tool_result::Result::Error(error)) => {
            format!("The tool failed: {}", error.message)
        }
        None => "The tool finished with no result.".to_owned(),
    }
}

fn ask_user_text(ask: &AskUserQuestionResult) -> String {
    use ask_user_question_result::answer_item::Answer;
    match &ask.result {
        Some(ask_user_question_result::Result::Success(success)) => success
            .answers
            .iter()
            .map(|item| match &item.answer {
                Some(Answer::MultipleChoice(choice)) => {
                    let mut answers = choice.selected_options.clone();
                    if !choice.other_text.is_empty() {
                        answers.push(choice.other_text.clone());
                    }
                    format!("The user answered: {}", answers.join(", "))
                }
                Some(Answer::Skipped(())) | None => "The user skipped the question.".to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(ask_user_question_result::Result::Error(error)) => {
            format!("The question failed: {}", error.message)
        }
        None => "The question finished with no answer.".to_owned(),
    }
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
        delay: Some(shell_output_wait()),
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
