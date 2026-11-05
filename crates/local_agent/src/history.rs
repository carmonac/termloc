use std::collections::HashMap;

use warp_multi_agent_api::Task;
use warp_multi_agent_api::message::Message as MessageKind;
use warp_multi_agent_api::message::summarization::SummaryType;

use crate::ollama::ChatMessage;
use crate::tools;

/// Renders a task as chat messages. Only messages after the latest conversation summary are kept,
/// because the summary stands in for everything before it.
pub(crate) fn chat_messages(task: &Task) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut tool_names = HashMap::new();
    for message in &task.messages {
        match message.message.as_ref() {
            Some(MessageKind::UserQuery(query)) => {
                messages.push(ChatMessage::new("user", query.query.as_str()));
            }
            Some(MessageKind::AgentOutput(output)) => {
                messages.push(ChatMessage::new("assistant", output.text.as_str()));
            }
            Some(MessageKind::ToolCall(call)) => {
                let Some(chat_call) = call.tool.as_ref().and_then(tools::chat_call_for_tool) else {
                    continue;
                };
                tool_names.insert(call.tool_call_id.as_str(), chat_call.function.name.clone());
                match messages.last_mut() {
                    Some(last) if last.role == "assistant" => last.tool_calls.push(chat_call),
                    _ => messages.push(ChatMessage {
                        tool_calls: vec![chat_call],
                        ..ChatMessage::new("assistant", "")
                    }),
                }
            }
            Some(MessageKind::ToolCallResult(result)) => {
                let name = tool_names
                    .get(result.tool_call_id.as_str())
                    .cloned()
                    .unwrap_or_default();
                messages.push(ChatMessage::tool_result(
                    name,
                    tools::result_text(result.result.as_ref()),
                ));
            }
            Some(MessageKind::Summarization(summarization)) => {
                if let Some(SummaryType::ConversationSummary(summary)) =
                    summarization.summary_type.as_ref()
                {
                    messages.clear();
                    messages.push(ChatMessage::new(
                        "user",
                        format!("Summary of the conversation so far:\n{}", summary.summary),
                    ));
                }
            }
            _ => {}
        }
    }
    messages
}
