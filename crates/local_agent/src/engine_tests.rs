use std::sync::Mutex;

use anyhow::anyhow;
use futures::executor::block_on;
use futures::future::BoxFuture;
use futures::{FutureExt as _, stream};
use serde_json::json;
use warp_multi_agent_api::client_action::Action;
use warp_multi_agent_api::request::input::tool_call_result::Result as InputToolResult;
use warp_multi_agent_api::request::input::user_inputs::UserInput as UserInputMessage;
use warp_multi_agent_api::request::input::{self, UserInputs};
use warp_multi_agent_api::request::{Input, TaskContext};
use warp_multi_agent_api::response_event::Type;
use warp_multi_agent_api::{
    LongRunningShellCommandSnapshot, RunShellCommandResult, ShellCommandFinished,
    run_shell_command_result,
};

use super::*;
use crate::ollama::{ChatChunk, ChatFunction, ChatStream};

/// Replays scripted model responses and records what the model was asked.
#[derive(Default)]
struct FakeBackend {
    responses: Mutex<Vec<Vec<ChatChunk>>>,
    requests: Mutex<Vec<ChatRequest>>,
    fails: bool,
}

impl FakeBackend {
    fn scripted(mut responses: Vec<Vec<ChatChunk>>) -> Arc<Self> {
        responses.reverse();
        Arc::new(Self {
            responses: Mutex::new(responses),
            ..Default::default()
        })
    }

    fn request(&self, index: usize) -> ChatRequest {
        self.requests.lock().unwrap()[index].clone()
    }
}

impl ChatBackend for FakeBackend {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'static, anyhow::Result<ChatStream>> {
        self.requests.lock().unwrap().push(request);
        let result = if self.fails {
            Err(anyhow!("connection refused"))
        } else {
            let chunks = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .expect("scripted response");
            let stream: ChatStream = stream::iter(chunks.into_iter().map(Ok)).boxed();
            Ok(stream)
        };
        async move { result }.boxed()
    }
}

fn text(content: &str) -> ChatChunk {
    ChatChunk {
        message: ChatMessage::new("assistant", content),
        ..Default::default()
    }
}

fn tool_call(name: &str, arguments: serde_json::Value) -> ChatChunk {
    ChatChunk {
        message: ChatMessage {
            tool_calls: vec![ChatToolCall {
                function: ChatFunction {
                    name: name.to_owned(),
                    arguments,
                },
            }],
            ..ChatMessage::new("assistant", "")
        },
        ..Default::default()
    }
}

fn config() -> LocalAgentConfig {
    LocalAgentConfig {
        model: "test-model".to_owned(),
        num_ctx: 4096,
    }
}

fn user_request(query: &str, tasks: Vec<Task>) -> Request {
    request_with_input(
        UserInput::UserQuery(input::UserQuery {
            query: query.to_owned(),
            ..Default::default()
        }),
        tasks,
    )
}

fn request_with_input(user_input: UserInput, tasks: Vec<Task>) -> Request {
    Request {
        task_context: Some(TaskContext { tasks }),
        input: Some(Input {
            r#type: Some(InputType::UserInputs(UserInputs {
                inputs: vec![UserInputMessage {
                    input: Some(user_input),
                }],
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn run(backend: Arc<FakeBackend>, request: Request) -> Vec<ResponseEvent> {
    block_on(run_turn(backend, config(), request).collect())
}

/// Applies a turn's actions the way the client does, for feeding into the next request.
fn apply(tasks: &mut Vec<Task>, events: &[ResponseEvent]) {
    for event in events {
        let Some(Type::ClientActions(client_actions)) = &event.r#type else {
            continue;
        };
        for action in client_actions
            .actions
            .iter()
            .filter_map(|a| a.action.clone())
        {
            match action {
                Action::CreateTask(create) => tasks.push(create.task.unwrap()),
                Action::AddMessagesToTask(add) => {
                    let task = tasks
                        .iter_mut()
                        .find(|task| task.id == add.task_id)
                        .unwrap();
                    task.messages.extend(add.messages);
                }
                Action::AppendToMessageContent(append) => {
                    let delta = append.message.unwrap();
                    let task = tasks
                        .iter_mut()
                        .find(|task| task.id == append.task_id)
                        .unwrap();
                    let target = task.messages.iter_mut().find(|m| m.id == delta.id).unwrap();
                    if let (
                        Some(MessageKind::AgentOutput(output)),
                        Some(MessageKind::AgentOutput(delta)),
                    ) = (&mut target.message, delta.message)
                    {
                        output.text.push_str(&delta.text);
                    }
                }
                other => panic!("unexpected action {other:?}"),
            }
        }
    }
}

fn finish_reason(events: &[ResponseEvent]) -> Reason {
    match &events.last().unwrap().r#type {
        Some(Type::Finished(finished)) => finished.reason.clone().unwrap(),
        other => panic!("stream did not finish: {other:?}"),
    }
}

fn tool_call_id(tasks: &[Task]) -> String {
    tasks[0]
        .messages
        .iter()
        .find_map(|message| match &message.message {
            Some(MessageKind::ToolCall(call)) => Some(call.tool_call_id.clone()),
            _ => None,
        })
        .expect("a tool call")
}

#[test]
fn first_turn_inits_creates_task_and_streams_text() {
    let backend = FakeBackend::scripted(vec![vec![text("Hel"), text("lo")]]);
    let events = run(backend, user_request("hi", Vec::new()));

    assert!(matches!(events[0].r#type, Some(Type::Init(_))));
    assert!(matches!(finish_reason(&events), Reason::Done(_)));

    let mut tasks = Vec::new();
    apply(&mut tasks, &events);
    assert_eq!(tasks.len(), 1);
    assert!(matches!(
        tasks[0].messages[0].message,
        Some(MessageKind::UserQuery(_))
    ));
    let Some(MessageKind::AgentOutput(output)) = &tasks[0].messages[1].message else {
        panic!("expected agent output");
    };
    assert_eq!(output.text, "Hello");
}

#[test]
fn tool_result_reaches_the_model_on_the_next_turn() {
    let backend = FakeBackend::scripted(vec![
        vec![tool_call("run_shell_command", json!({ "command": "ls" }))],
        vec![text("done")],
    ]);
    let mut tasks = Vec::new();
    apply(
        &mut tasks,
        &run(backend.clone(), user_request("list", Vec::new())),
    );

    let result = UserInput::ToolCallResult(input::ToolCallResult {
        tool_call_id: tool_call_id(&tasks),
        result: Some(InputToolResult::RunShellCommand(RunShellCommandResult {
            command: "ls".to_owned(),
            result: Some(run_shell_command_result::Result::CommandFinished(
                ShellCommandFinished {
                    output: "a.txt".to_owned(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        })),
    });
    let events = run(backend.clone(), request_with_input(result, tasks.clone()));
    assert!(matches!(finish_reason(&events), Reason::Done(_)));

    let messages = backend.request(1).messages;
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);
    assert_eq!(messages[2].tool_calls[0].function.name, "run_shell_command");
    assert_eq!(messages[3].tool_name.as_deref(), Some("run_shell_command"));
    assert!(messages[3].content.contains("a.txt"));

    // The result is echoed, so a third turn still sees it.
    apply(&mut tasks, &events);
    assert!(
        tasks[0]
            .messages
            .iter()
            .any(|message| matches!(message.message, Some(MessageKind::ToolCallResult(_))))
    );
}

#[test]
fn running_command_is_awaited_without_asking_the_model() {
    let backend = FakeBackend::scripted(vec![vec![tool_call(
        "run_shell_command",
        json!({ "command": "sleep 30" }),
    )]]);
    let mut tasks = Vec::new();
    apply(
        &mut tasks,
        &run(backend.clone(), user_request("wait", Vec::new())),
    );

    let snapshot = UserInput::ToolCallResult(input::ToolCallResult {
        tool_call_id: tool_call_id(&tasks),
        result: Some(InputToolResult::RunShellCommand(RunShellCommandResult {
            result: Some(
                run_shell_command_result::Result::LongRunningCommandSnapshot(
                    LongRunningShellCommandSnapshot {
                        command_id: "block-1".to_owned(),
                        ..Default::default()
                    },
                ),
            ),
            ..Default::default()
        })),
    });
    let events = run(backend.clone(), request_with_input(snapshot, tasks.clone()));
    assert!(matches!(finish_reason(&events), Reason::Done(_)));
    assert_eq!(backend.requests.lock().unwrap().len(), 1);

    apply(&mut tasks, &events);
    let Some(MessageKind::ToolCall(call)) = &tasks[0].messages.last().unwrap().message else {
        panic!("expected a tool call");
    };
    let Some(Tool::ReadShellCommandOutput(read)) = &call.tool else {
        panic!("expected a wait for the command");
    };
    assert_eq!(read.command_id, "block-1");
}

#[test]
fn unusable_tool_call_is_repaired_without_reaching_the_client() {
    let backend = FakeBackend::scripted(vec![
        vec![tool_call("run_shell_command", json!({}))],
        vec![text("fixed")],
    ]);
    let events = run(backend.clone(), user_request("go", Vec::new()));
    assert!(matches!(finish_reason(&events), Reason::Done(_)));

    let mut tasks = Vec::new();
    apply(&mut tasks, &events);
    assert!(
        !tasks[0]
            .messages
            .iter()
            .any(|message| matches!(message.message, Some(MessageKind::ToolCall(_))))
    );
    assert!(
        backend
            .request(1)
            .messages
            .last()
            .unwrap()
            .content
            .contains("command")
    );
}

#[test]
fn gives_up_after_repeated_unusable_tool_calls() {
    let bad = || vec![tool_call("nope", json!({}))];
    let backend = FakeBackend::scripted(vec![bad(), bad(), bad()]);
    let events = run(backend, user_request("go", Vec::new()));
    assert!(matches!(finish_reason(&events), Reason::InternalError(_)));
}

#[test]
fn backend_failure_finishes_the_stream_with_a_message() {
    let backend = Arc::new(FakeBackend {
        fails: true,
        ..Default::default()
    });
    let events = run(backend, user_request("go", Vec::new()));
    let Reason::InternalError(error) = finish_reason(&events) else {
        panic!("expected an internal error");
    };
    assert!(error.message.contains("connection refused"));
}

#[test]
fn summary_replaces_earlier_history() {
    let backend = FakeBackend::scripted(vec![
        vec![text("first answer")],
        vec![text("the summary")],
        vec![text("second answer")],
    ]);
    let mut tasks = Vec::new();
    apply(
        &mut tasks,
        &run(backend.clone(), user_request("first", Vec::new())),
    );

    let summarize = Request {
        task_context: Some(TaskContext {
            tasks: tasks.clone(),
        }),
        input: Some(Input {
            r#type: Some(InputType::SummarizeConversation(
                input::SummarizeConversation {
                    prompt: String::new(),
                },
            )),
            ..Default::default()
        }),
        ..Default::default()
    };
    let events = run(backend.clone(), summarize);
    assert!(backend.request(1).tools.is_empty());
    apply(&mut tasks, &events);

    run(backend.clone(), user_request("second", tasks));
    let messages = backend.request(2).messages;
    let contents: Vec<&str> = messages
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(contents.len(), 3);
    assert!(contents[1].contains("the summary"));
    assert_eq!(contents[2], "second");
}

#[test]
fn reuses_the_existing_root_task() {
    let backend = FakeBackend::scripted(vec![vec![text("ok")]]);
    let task = Task {
        id: "root".to_owned(),
        ..Default::default()
    };
    let events = run(backend, user_request("hi", vec![task]));
    assert!(!events.iter().any(|event| {
        match &event.r#type {
            Some(Type::ClientActions(client_actions)) => client_actions
                .actions
                .iter()
                .any(|action| matches!(action.action, Some(Action::CreateTask(_)))),
            _ => false,
        }
    }));
}

/// Needs a running Ollama. Run with `LOCAL_AGENT_TEST_MODEL=<model> cargo test -p local_agent -- --ignored`.
#[test]
#[ignore]
fn live_model_calls_a_tool() {
    let model = std::env::var("LOCAL_AGENT_TEST_MODEL").expect("LOCAL_AGENT_TEST_MODEL");
    let backend = Arc::new(crate::OllamaClient::new("http://localhost:11434"));
    let config = LocalAgentConfig {
        model,
        num_ctx: 4096,
    };
    let request = user_request("Use a tool to run the command `ls`.", Vec::new());
    let events: Vec<ResponseEvent> = block_on(run_turn(backend, config, request).collect());
    assert!(
        matches!(finish_reason(&events), Reason::Done(_)),
        "{events:?}"
    );

    let mut tasks = Vec::new();
    apply_ignoring_reasoning(&mut tasks, &events);
    assert!(!tool_call_id(&tasks).is_empty());
}

fn apply_ignoring_reasoning(tasks: &mut Vec<Task>, events: &[ResponseEvent]) {
    let events: Vec<ResponseEvent> = events
        .iter()
        .filter(|event| !is_reasoning_append(event))
        .cloned()
        .collect();
    apply(tasks, &events);
}

fn is_reasoning_append(event: &ResponseEvent) -> bool {
    let Some(Type::ClientActions(client_actions)) = &event.r#type else {
        return false;
    };
    client_actions
        .actions
        .iter()
        .any(|action| match &action.action {
            Some(Action::AppendToMessageContent(append)) => {
                append.mask.as_ref().is_some_and(|mask| {
                    mask.paths
                        .iter()
                        .any(|path| path == AGENT_REASONING_TEXT_PATH)
                })
            }
            _ => false,
        })
}
