use prost_types::Struct;
use serde_json::json;
use warp_multi_agent_api::message::tool_call::Tool;
use warp_multi_agent_api::request::McpContext;
use warp_multi_agent_api::request::mcp_context::{McpServer, McpTool};

use super::*;

fn server(id: &str, tool_names: &[&str]) -> McpServer {
    McpServer {
        id: id.to_owned(),
        tools: tool_names
            .iter()
            .map(|name| McpTool {
                name: (*name).to_owned(),
                input_schema: Some(Struct::default()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn tool_names_are_unique_and_keep_their_server() {
    let context = McpContext {
        servers: vec![
            server("one", &["search"]),
            server("two", &["search", "fetch"]),
        ],
        ..Default::default()
    };

    let entries = entries(Some(&context));

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].server_id, "one");
    assert_eq!(entries[1].chat_name, "mcp_fetch");
}

#[test]
fn arguments_survive_the_trip_through_protobuf() {
    let arguments = json!({ "query": "x", "limit": 3.0, "tags": ["a"], "nested": { "ok": true } });
    let context = McpContext {
        servers: vec![server("one", &["search"])],
        ..Default::default()
    };
    let call = ChatFunction {
        name: "mcp_search".to_owned(),
        arguments: arguments.clone(),
    };

    let tool = entries(Some(&context))[0].tool_call(&call);

    let Tool::CallMcpTool(call) = tool else {
        panic!("expected an MCP call");
    };
    assert_eq!(call.name, "search");
    assert_eq!(struct_to_json(&call.args.unwrap()), arguments);
}
