use serde_json::json;
use warp_multi_agent_api::read_files_result::AnyFilesSuccess;
use warp_multi_agent_api::{AnyFileContent, FileContent, ReadFilesResult};

use super::*;

fn call(name: &str, arguments: Value) -> ChatFunction {
    ChatFunction {
        name: name.to_owned(),
        arguments,
    }
}

#[test]
fn shell_commands_are_always_risky() {
    let tool = parse_tool_call(&call(RUN_SHELL_COMMAND, json!({ "command": "ls" })), None).unwrap();
    let Tool::RunShellCommand(shell) = tool else {
        panic!("expected a shell command");
    };
    assert!(shell.is_risky);
    assert!(!shell.is_read_only);
}

#[test]
fn edit_maps_to_search_and_replace() {
    let tool = parse_tool_call(
        &call(
            EDIT_FILE,
            json!({ "path": "missing.rs", "old_text": "x", "new_text": "y" }),
        ),
        None,
    )
    .unwrap();
    let Tool::ApplyFileDiffs(edit) = tool else {
        panic!("expected a file edit");
    };
    assert!(!edit.summary.is_empty());
    assert_eq!(edit.diffs[0].file_path, "missing.rs");
    assert_eq!(edit.diffs[0].search, "x");
    assert_eq!(edit.diffs[0].replace, "y");
}

#[test]
fn tool_calls_round_trip_through_history() {
    let original = call(
        EDIT_FILE,
        json!({ "path": "missing", "old_text": "b", "new_text": "c" }),
    );
    let tool = parse_tool_call(&original, None).unwrap();
    assert_eq!(chat_call_for_tool(&tool).unwrap().function, original);
}

#[test]
fn rejects_unknown_tools_and_missing_arguments() {
    assert!(
        parse_tool_call(&call("nope", json!({})), None)
            .unwrap_err()
            .contains("nope")
    );
    let error = parse_tool_call(&call(READ_FILE, json!({})), None).unwrap_err();
    assert!(error.contains("path"));
}

#[test]
fn truncation_respects_char_boundaries() {
    let text = "é".repeat(MAX_TOOL_OUTPUT_CHARS);
    let truncated = truncate_middle(text);
    assert!(truncated.contains("[output truncated]"));
    assert!(truncated.len() < MAX_TOOL_OUTPUT_CHARS * 2);
}

#[test]
fn text_content_of_any_file_read_reaches_the_model() {
    let result = ToolResult::ReadFiles(ReadFilesResult {
        result: Some(read_files_result::Result::AnyFilesSuccess(
            AnyFilesSuccess {
                files: vec![AnyFileContent {
                    content: Some(any_file_content::Content::TextContent(FileContent {
                        file_path: "notes.txt".to_owned(),
                        content: "hello world".to_owned(),
                        ..Default::default()
                    })),
                }],
                failed_reads: Vec::new(),
            },
        )),
    });
    let text = result_text(Some(&result));
    assert!(text.contains("notes.txt"));
    assert!(text.contains("hello world"));
}

#[test]
fn partial_line_edits_widen_to_whole_lines() {
    let edit = whole_line_edit("first\nhello world\nlast\n", "world", "ollama");
    assert_eq!(
        edit,
        Some(("hello world".to_owned(), "hello ollama".to_owned()))
    );
}

#[test]
fn multi_line_edits_keep_surrounding_text() {
    let edit = whole_line_edit("a b\nc d\n", "b\nc", "x");
    assert_eq!(edit, Some(("a b\nc d".to_owned(), "a x d".to_owned())));
}

#[test]
fn ambiguous_missing_or_empty_text_is_not_widened() {
    assert_eq!(whole_line_edit("aa aa\n", "aa", "b"), None);
    assert_eq!(whole_line_edit("aaa\n", "aa", "b"), None);
    assert_eq!(whole_line_edit("hello\n", "nope", "b"), None);
    assert_eq!(whole_line_edit("hello\n", "", "b"), None);
}

#[test]
fn edit_reads_relative_paths_from_cwd() {
    let dir = std::env::temp_dir().join(format!("local_agent_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "hello world\n").unwrap();

    let tool = parse_tool_call(
        &call(
            EDIT_FILE,
            json!({ "path": "notes.txt", "old_text": "world", "new_text": "ollama" }),
        ),
        Some(&dir),
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let Tool::ApplyFileDiffs(edit) = tool else {
        panic!("expected a file edit");
    };
    assert_eq!(edit.diffs[0].search, "hello world");
    assert_eq!(edit.diffs[0].replace, "hello ollama");
}

#[test]
fn widening_handles_multibyte_text() {
    let edit = whole_line_edit("año é fin\n", "é", "e");
    assert_eq!(edit, Some(("año é fin".to_owned(), "año e fin".to_owned())));
}
