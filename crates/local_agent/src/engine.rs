use std::path::PathBuf;
use std::sync::Arc;

use futures::{Stream, StreamExt as _};
use instant::Instant;
use warp_multi_agent_api::message::summarization::{ConversationSummary, SummaryType};
use warp_multi_agent_api::message::tool_call::Tool;
use warp_multi_agent_api::message::{
    AgentOutput, AgentReasoning, Message as MessageKind, Summarization, ToolCall, ToolCallResult,
    UserQuery,
};
use warp_multi_agent_api::request::input::Type as InputType;
use warp_multi_agent_api::request::input::user_inputs::user_input::Input as UserInput;
use warp_multi_agent_api::response_event::stream_finished::{self, Reason};
use warp_multi_agent_api::{InputContext, Message, Request, ResponseEvent, Task};

use crate::events::{self, AGENT_OUTPUT_TEXT_PATH, AGENT_REASONING_TEXT_PATH, TurnEvents};
use crate::history;
use crate::ollama::{ChatBackend, ChatMessage, ChatOptions, ChatRequest, ChatToolCall};
use crate::oneshot::{OneShot, QuickModel};
use crate::prompt;
use crate::tools::{self, Toolbox};

const KEEP_ALIVE: &str = "30m";
/// How many times a model may retry after producing an unusable tool call in one turn.
const MAX_TOOL_CALL_REPAIRS: usize = 2;
/// Tool calls allowed since the user last spoke, so a confused model can't run forever.
const MAX_CHAINED_TOOL_CALLS: usize = 25;
/// Identical tool calls in a row that count as being stuck.
const MAX_REPEATED_TOOL_CALLS: usize = 3;
/// Share of the context window left for the model's reply and the tool definitions.
const RESERVED_CONTEXT_PERCENT: usize = 25;
/// Share of the available context at which a conversation gets summarized.
const SUMMARIZE_AT_PERCENT: usize = 80;

#[derive(Clone, Debug)]
pub struct LocalAgentConfig {
    pub model: String,
    pub num_ctx: u32,
    /// Writes the title of new conversations.
    pub title_model: Option<QuickModel>,
}

/// Runs one agent turn: a model response plus any tool calls for the client to execute. The client
/// starts the next turn by sending the tool results in a new request.
pub fn run_turn(
    backend: Arc<dyn ChatBackend>,
    config: LocalAgentConfig,
    request: Request,
) -> impl Stream<Item = ResponseEvent> + Send + 'static {
    async_stream::stream! {
        let toolbox = Toolbox::for_request(&request);
        let request_id = events::new_id();
        let conversation_id = request
            .metadata
            .as_ref()
            .map(|metadata| metadata.conversation_id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(events::new_id);
        yield events::init(conversation_id, request_id.clone());

        let tasks = request.task_context.map(|context| context.tasks).unwrap_or_default();
        let existing_task = root_task(tasks);
        let task_id = match &existing_task {
            Some(task) => task.id.clone(),
            None => {
                let task = Task { id: events::new_id(), ..Default::default() };
                let task_id = task.id.clone();
                yield events::create_task(task);
                task_id
            }
        };
        let turn = TurnEvents { task_id, request_id };

        let input = request.input.unwrap_or_default();
        let cwd = working_directory(input.context.as_ref(), existing_task.as_ref());
        let mut chat = vec![ChatMessage::new("system", prompt::system_prompt(input.context.as_ref()))];
        chat.extend(existing_task.as_ref().map(history::chat_messages).unwrap_or_default());

        let context = input.context;
        let summarize = match input.r#type {
            Some(InputType::UserInputs(user_inputs)) => {
                let inputs = user_inputs.inputs.into_iter().filter_map(|input| input.input);
                let echoes: Vec<Message> = inputs
                    .filter_map(|input| echo_input(&turn, input, context.clone()))
                    .collect();
                let waits: Vec<Tool> = echoes.iter().filter_map(pending_wait).collect();
                if !echoes.is_empty() {
                    let echoed = Task { messages: echoes.clone(), ..Default::default() };
                    chat.extend(new_input_messages(existing_task.as_ref(), echoed));
                    yield turn.add_messages(echoes);
                }
                if !waits.is_empty() {
                    yield turn.add_messages(tool_call_messages(&turn, waits));
                    yield events::finished(Reason::Done(stream_finished::Done {}));
                    return;
                }
                None
            }
            Some(InputType::ResumeConversation(_)) => None,
            Some(InputType::SummarizeConversation(summarize)) => Some(summarize.prompt),
            _ => {
                yield events::finished(internal_error(
                    "This request type is not supported by the local agent.",
                ));
                return;
            }
        };

        let available_tokens = available_context_tokens(config.num_ctx);
        if let Some(instructions) = summarize {
            history::trim_to_budget(&mut chat, available_tokens);
            let started_at = Instant::now();
            let summary = match summarize_chat(&backend, &config, chat, &instructions).await {
                Ok(summary) => summary,
                Err(error) => {
                    yield events::finished(internal_error(error));
                    return;
                }
            };
            yield turn.add_messages(vec![summary_message(&turn, summary, started_at)]);
            yield events::finished(Reason::Done(stream_finished::Done {}));
            return;
        }

        let summarize_at = available_tokens * SUMMARIZE_AT_PERCENT / 100;
        history::trim_to_budget(&mut chat, summarize_at);
        if history::estimated_tokens(&chat) > summarize_at {
            history::trim_to_budget(&mut chat, available_tokens);
            if history::estimated_tokens(&chat) > available_tokens {
                yield events::finished(context_window_exceeded());
                return;
            }
            let started_at = Instant::now();
            let system = chat.first().cloned();
            let summary = match summarize_chat(&backend, &config, chat, "").await {
                Ok(summary) => summary,
                Err(error) => {
                    yield events::finished(internal_error(error));
                    return;
                }
            };
            chat = system.into_iter().collect();
            chat.push(ChatMessage::new("user", history::summary_text(&summary)));
            chat.push(ChatMessage::new("user", prompt::CONTINUE_PROMPT));
            yield turn.add_messages(vec![summary_message(&turn, summary, started_at)]);
            if history::estimated_tokens(&chat) > available_tokens {
                yield events::finished(context_window_exceeded());
                return;
            }
        }

        let mut repairs = 0;
        loop {
            let chat_stream = backend.chat(chat_request(&config, chat.clone(), toolbox.definitions())).await;
            let mut stream = match chat_stream {
                Ok(stream) => stream,
                Err(error) => {
                    yield events::finished(internal_error(error.to_string()));
                    return;
                }
            };

            let mut output_id = None;
            let mut reasoning_id = None;
            let mut calls: Vec<ChatToolCall> = Vec::new();
            let mut hit_length_limit = false;
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield events::finished(internal_error(error.to_string()));
                        return;
                    }
                };
                hit_length_limit |= chunk.done_reason.as_deref() == Some("length");
                if !chunk.message.thinking.is_empty() {
                    let delta = MessageKind::AgentReasoning(AgentReasoning {
                        reasoning: chunk.message.thinking,
                        finished_duration: None,
                    });
                    yield stream_delta(&turn, &mut reasoning_id, delta, AGENT_REASONING_TEXT_PATH);
                }
                if !chunk.message.content.is_empty() {
                    let delta = MessageKind::AgentOutput(AgentOutput { text: chunk.message.content });
                    yield stream_delta(&turn, &mut output_id, delta, AGENT_OUTPUT_TEXT_PATH);
                }
                calls.extend(chunk.message.tool_calls);
            }

            if hit_length_limit {
                yield events::finished(Reason::MaxTokenLimit(stream_finished::ReachedMaxTokenLimit {}));
                return;
            }

            let parsed: Result<Vec<Tool>, String> =
                calls.iter().map(|call| toolbox.parse(&call.function, cwd.as_deref())).collect();
            if parsed.is_ok() && let Some(text) = runaway_notice(&chat, &calls) {
                let message = turn.message(MessageKind::AgentOutput(AgentOutput { text }));
                yield turn.add_messages(vec![message]);
                yield events::finished(Reason::Done(stream_finished::Done {}));
                return;
            }
            match parsed {
                Ok(parsed) => {
                    if !parsed.is_empty() {
                        yield turn.add_messages(tool_call_messages(&turn, parsed));
                    }
                    if existing_task.is_none()
                        && let Some(model) = config.title_model.clone()
                        && let Some(query) = first_query(&chat)
                        && let Ok(title) = OneShot::new(backend.clone(), model).title(&query).await
                    {
                        yield turn.update_description(title);
                    }
                    yield events::finished(Reason::Done(stream_finished::Done {}));
                    return;
                }
                Err(problem) if repairs < MAX_TOOL_CALL_REPAIRS => {
                    repairs += 1;
                    chat.push(ChatMessage { tool_calls: calls, ..ChatMessage::new("assistant", "") });
                    chat.push(ChatMessage::tool_result("", problem));
                }
                Err(problem) => {
                    yield events::finished(internal_error(format!(
                        "The model kept producing invalid tool calls: {problem}"
                    )));
                    return;
                }
            }
        }
    }
}

fn first_query(chat: &[ChatMessage]) -> Option<String> {
    chat.iter()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
}

fn available_context_tokens(num_ctx: u32) -> usize {
    num_ctx as usize * (100 - RESERVED_CONTEXT_PERCENT) / 100
}

async fn summarize_chat(
    backend: &Arc<dyn ChatBackend>,
    config: &LocalAgentConfig,
    mut chat: Vec<ChatMessage>,
    instructions: &str,
) -> Result<String, String> {
    let mut prompt_text = prompt::SUMMARY_PROMPT.to_owned();
    if !instructions.is_empty() {
        prompt_text.push_str(&format!("\n{instructions}"));
    }
    chat.push(ChatMessage::new("user", prompt_text));
    let mut stream = backend
        .chat(chat_request(config, chat, Vec::new()))
        .await
        .map_err(|error| error.to_string())?;
    let mut summary = String::new();
    while let Some(chunk) = stream.next().await {
        summary.push_str(&chunk.map_err(|error| error.to_string())?.message.content);
    }
    Ok(summary)
}

fn summary_message(turn: &TurnEvents, summary: String, started_at: Instant) -> Message {
    turn.message(MessageKind::Summarization(Summarization {
        finished_duration: prost_types::Duration::try_from(started_at.elapsed()).ok(),
        summary_type: Some(SummaryType::ConversationSummary(ConversationSummary {
            summary,
            ..Default::default()
        })),
    }))
}

/// Explains why the turn stops, if `new_calls` would be too many or the model is going in circles.
fn runaway_notice(chat: &[ChatMessage], new_calls: &[ChatToolCall]) -> Option<String> {
    if new_calls.is_empty() {
        return None;
    }
    let user_turn_start = chat
        .iter()
        .rposition(|message| message.role == "user")
        .map_or(0, |index| index + 1);
    let previous_calls = chat[user_turn_start..]
        .iter()
        .flat_map(|message| &message.tool_calls);
    let calls: Vec<&ChatToolCall> = previous_calls.chain(new_calls).collect();
    if calls.len() > MAX_CHAINED_TOOL_CALLS {
        return Some(format!(
            "I stopped after {MAX_CHAINED_TOOL_CALLS} tool calls in a row. \
             Tell me if you want me to continue."
        ));
    }
    let repeated = calls.last().is_some_and(|last| {
        !tools::may_repeat(last)
            && calls.len() >= MAX_REPEATED_TOOL_CALLS
            && calls[calls.len() - MAX_REPEATED_TOOL_CALLS..]
                .iter()
                .all(|call| call == last)
    });
    repeated.then(|| {
        "I stopped because I was repeating the same tool call without making progress.".to_owned()
    })
}

/// Tool-result requests may carry no context, so fall back to the latest user query's.
fn working_directory(context: Option<&InputContext>, task: Option<&Task>) -> Option<PathBuf> {
    let from_history = || {
        task?
            .messages
            .iter()
            .rev()
            .find_map(|message| match &message.message {
                Some(MessageKind::UserQuery(query)) => query.context.as_ref(),
                _ => None,
            })
    };
    context
        .filter(|context| context.directory.is_some())
        .or_else(from_history)?
        .directory
        .as_ref()
        .filter(|directory| !directory.pwd.is_empty())
        .map(|directory| PathBuf::from(&directory.pwd))
}

fn root_task(tasks: Vec<Task>) -> Option<Task> {
    tasks.into_iter().find(|task| {
        task.dependencies
            .as_ref()
            .is_none_or(|dependencies| dependencies.parent_task_id.is_empty())
    })
}

fn chat_request(
    config: &LocalAgentConfig,
    messages: Vec<ChatMessage>,
    tools: Vec<serde_json::Value>,
) -> ChatRequest {
    ChatRequest {
        model: config.model.clone(),
        messages,
        tools,
        options: ChatOptions {
            num_ctx: config.num_ctx,
        },
        stream: true,
        keep_alive: KEEP_ALIVE,
        think: None,
    }
}

fn tool_call_messages(turn: &TurnEvents, tools: Vec<Tool>) -> Vec<Message> {
    tools
        .into_iter()
        .map(|tool| {
            turn.message(MessageKind::ToolCall(ToolCall {
                tool_call_id: events::new_id(),
                tool: Some(tool),
            }))
        })
        .collect()
}

fn pending_wait(message: &Message) -> Option<Tool> {
    match message.message.as_ref() {
        Some(MessageKind::ToolCallResult(result)) => {
            result.result.as_ref().and_then(tools::wait_for_command)
        }
        _ => None,
    }
}

/// The first delta of a stream creates its message; later ones append to it.
fn stream_delta(
    turn: &TurnEvents,
    message_id: &mut Option<String>,
    delta: MessageKind,
    path: &str,
) -> ResponseEvent {
    match message_id {
        Some(id) => turn.append(id.clone(), delta, path),
        None => {
            let message = turn.message(delta);
            *message_id = Some(message.id.clone());
            turn.add_messages(vec![message])
        }
    }
}

/// The server, not the client, records inputs in the task, so they must be echoed or the next
/// request would be missing them.
fn echo_input(
    turn: &TurnEvents,
    input: UserInput,
    context: Option<InputContext>,
) -> Option<Message> {
    match input {
        UserInput::UserQuery(query) => Some(turn.message(MessageKind::UserQuery(UserQuery {
            query: query.query,
            context,
            ..Default::default()
        }))),
        UserInput::ToolCallResult(result) => {
            Some(turn.message(MessageKind::ToolCallResult(ToolCallResult {
                tool_call_id: result.tool_call_id,
                context: None,
                result: result.result.and_then(tools::history_result),
            })))
        }
        _ => None,
    }
}

/// Renders just-echoed inputs, resolving tool names against the calls already in the task.
fn new_input_messages(existing_task: Option<&Task>, echoed: Task) -> Vec<ChatMessage> {
    let Some(existing) = existing_task else {
        return history::chat_messages(&echoed);
    };
    let existing_len = history::chat_messages(existing).len();
    let mut combined = existing.clone();
    combined.messages.extend(echoed.messages);
    history::chat_messages(&combined).split_off(existing_len)
}

fn context_window_exceeded() -> Reason {
    Reason::ContextWindowExceeded(stream_finished::ContextWindowExceeded {})
}

fn internal_error(message: impl Into<String>) -> Reason {
    Reason::InternalError(stream_finished::InternalError {
        message: message.into(),
    })
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
