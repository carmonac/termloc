use std::collections::HashMap;

use warp_multi_agent_api::Task;
use warp_multi_agent_api::message::Message as MessageKind;
use warp_multi_agent_api::message::summarization::SummaryType;

use crate::ollama::ChatMessage;
use crate::tools;

/// Rough size of a token, which is all a budget needs: Ollama doesn't expose its tokenizer.
const CHARS_PER_TOKEN: usize = 3;
/// Tool results in the last few messages stay whole while older ones are cut first.
const RECENT_MESSAGES: usize = 6;
const OLD_RESULT_CHARS: usize = 1_500;
const MIN_RESULT_CHARS: usize = 300;

/// Estimates how many tokens `messages` use.
pub(crate) fn estimated_tokens(messages: &[ChatMessage]) -> usize {
    let chars: usize = messages
        .iter()
        .map(|message| {
            let calls: usize = message
                .tool_calls
                .iter()
                .map(|call| call.function.arguments.to_string().len())
                .sum();
            message.content.len() + calls
        })
        .sum();
    chars / CHARS_PER_TOKEN
}

/// Shortens tool results until `messages` fit in `budget` tokens or nothing more can be cut. Old
/// results go first, because the model has already acted on them.
pub(crate) fn trim_to_budget(messages: &mut [ChatMessage], budget: usize) {
    let recent_start = messages.len().saturating_sub(RECENT_MESSAGES);
    for (end, max_chars) in [
        (recent_start, OLD_RESULT_CHARS),
        (recent_start, MIN_RESULT_CHARS),
        (messages.len(), OLD_RESULT_CHARS),
        (messages.len(), MIN_RESULT_CHARS),
    ] {
        for message in messages[..end].iter_mut().filter(|m| m.role == "tool") {
            if estimated_tokens(std::slice::from_ref(message)) * CHARS_PER_TOKEN > max_chars {
                message.content = shorten(&message.content, max_chars);
            }
        }
        if estimated_tokens(messages) <= budget {
            return;
        }
    }
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(max_chars);
    format!("{}\n[shortened to save context]", &text[..end])
}

pub(crate) fn summary_text(summary: &str) -> String {
    format!("Summary of the conversation so far:\n{summary}")
}

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
                    messages.push(ChatMessage::new("user", summary_text(&summary.summary)));
                }
            }
            _ => {}
        }
    }
    messages
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
