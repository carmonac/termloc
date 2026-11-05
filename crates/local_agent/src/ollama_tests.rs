use futures::executor::block_on;
use futures::{StreamExt as _, stream};

use super::*;

fn parse(reads: Vec<&str>) -> Vec<anyhow::Result<ChatChunk>> {
    let reads: Vec<anyhow::Result<Vec<u8>>> = reads
        .into_iter()
        .map(|read| Ok(read.as_bytes().to_vec()))
        .collect();
    block_on(parse_ndjson(stream::iter(reads)).collect())
}

#[test]
fn parses_lines_split_across_reads() {
    let chunks = parse(vec![
        "{\"message\":{\"role\":\"assistant\",\"con",
        "tent\":\"hi\"}}\n{\"done\":true,\"done_reason\":\"stop\"}\n",
    ]);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].as_ref().unwrap().message.content, "hi");
    assert!(chunks[1].as_ref().unwrap().done);
}

#[test]
fn parses_final_line_without_newline() {
    let chunks = parse(vec!["{\"done\":true}"]);
    assert_eq!(chunks.len(), 1);
}

#[test]
fn reports_error_lines() {
    let chunks = parse(vec!["{\"error\":\"model not found\"}\n"]);
    let error = chunks[0].as_ref().unwrap_err().to_string();
    assert!(error.contains("model not found"));
}

#[test]
fn does_not_send_thinking_back_to_the_model() {
    let message = ChatMessage {
        thinking: "secret".to_owned(),
        ..ChatMessage::new("assistant", "hi")
    };
    let json = serde_json::to_string(&message).unwrap();
    assert!(!json.contains("secret"));
}

#[test]
fn model_details_expose_capabilities_and_context() {
    let details: ShowResponse = serde_json::from_value(serde_json::json!({
        "capabilities": ["completion", "tools"],
        "model_info": { "general.architecture": "qwen3", "qwen3.context_length": 40960 },
    }))
    .unwrap();

    let model = details.into_model("qwen3:8b".to_owned(), 5);

    assert!(model.supports_tools);
    assert!(!model.supports_vision);
    assert_eq!(model.context_length, Some(40_960));
}
