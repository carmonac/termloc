use prost_types::value::Kind;
use prost_types::{ListValue, Struct};
use serde_json::{Map, Number, Value, json};
use warp_multi_agent_api::McpServerIdentity;
use warp_multi_agent_api::message::tool_call::{CallMcpTool, Tool};
use warp_multi_agent_api::request::McpContext;

use crate::ollama::ChatFunction;

const NAME_PREFIX: &str = "mcp_";

/// An MCP tool offered to the model.
#[derive(Clone, Debug)]
pub(crate) struct McpToolEntry {
    chat_name: String,
    tool_name: String,
    description: String,
    input_schema: Value,
    server_id: String,
    server_identity: Option<McpServerIdentity>,
}

/// Lists the tools of every server. Two servers may offer the same tool name; the first one wins,
/// because a chat tool name has to be unique.
pub(crate) fn entries(context: Option<&McpContext>) -> Vec<McpToolEntry> {
    let Some(context) = context else {
        return Vec::new();
    };
    let mut entries: Vec<McpToolEntry> = Vec::new();
    let server_tools = context.servers.iter().flat_map(|server| {
        server
            .tools
            .iter()
            .map(move |tool| (tool, server.id.clone(), server.identity.clone()))
    });
    for (tool, server_id, server_identity) in server_tools {
        let chat_name = chat_name(&tool.name);
        if entries.iter().any(|entry| entry.chat_name == chat_name) {
            continue;
        }
        entries.push(McpToolEntry {
            chat_name,
            tool_name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool
                .input_schema
                .as_ref()
                .map_or_else(|| json!({ "type": "object" }), struct_to_json),
            server_id,
            server_identity,
        });
    }
    entries
}

pub(crate) fn chat_name(tool_name: &str) -> String {
    format!("{NAME_PREFIX}{tool_name}")
}

impl McpToolEntry {
    pub(crate) fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.chat_name,
                "description": self.description,
                "parameters": self.input_schema,
            },
        })
    }

    pub(crate) fn matches(&self, call: &ChatFunction) -> bool {
        self.chat_name == call.name
    }

    pub(crate) fn tool_call(&self, call: &ChatFunction) -> Tool {
        Tool::CallMcpTool(CallMcpTool {
            name: self.tool_name.clone(),
            args: Some(json_to_struct(&call.arguments)),
            server_id: self.server_id.clone(),
            server_identity: self.server_identity.clone(),
        })
    }
}

pub(crate) fn struct_to_json(fields: &Struct) -> Value {
    Value::Object(
        fields
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), proto_value_to_json(value)))
            .collect(),
    )
}

fn proto_value_to_json(value: &prost_types::Value) -> Value {
    match &value.kind {
        Some(Kind::NumberValue(number)) => {
            Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        Some(Kind::StringValue(text)) => Value::String(text.clone()),
        Some(Kind::BoolValue(flag)) => Value::Bool(*flag),
        Some(Kind::StructValue(fields)) => struct_to_json(fields),
        Some(Kind::ListValue(list)) => {
            Value::Array(list.values.iter().map(proto_value_to_json).collect())
        }
        Some(Kind::NullValue(_)) | None => Value::Null,
    }
}

fn json_to_struct(value: &Value) -> Struct {
    let empty = Map::new();
    let object = value.as_object().unwrap_or(&empty);
    Struct {
        fields: object
            .iter()
            .map(|(key, value)| (key.clone(), json_to_proto_value(value)))
            .collect(),
    }
}

fn json_to_proto_value(value: &Value) -> prost_types::Value {
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(flag) => Kind::BoolValue(*flag),
        Value::Number(number) => Kind::NumberValue(number.as_f64().unwrap_or_default()),
        Value::String(text) => Kind::StringValue(text.clone()),
        Value::Array(values) => Kind::ListValue(ListValue {
            values: values.iter().map(json_to_proto_value).collect(),
        }),
        Value::Object(_) => Kind::StructValue(json_to_struct(value)),
    };
    prost_types::Value { kind: Some(kind) }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
