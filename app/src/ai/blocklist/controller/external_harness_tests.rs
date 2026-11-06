use std::time::SystemTime;

use uuid::Uuid;
use warp_multi_agent_api::client_action::{AddMessagesToTask, CreateTask, UpdateTaskMessage};
use warp_multi_agent_api::message::tool_call::{CallMcpTool, RunShellCommand, Tool};
use warp_multi_agent_api::message::tool_call_result::Result as ToolResult;
use warp_multi_agent_api::message::{
    AgentOutput, Message as MessageKind, ToolCall, ToolCallResult, UserQuery,
};
use warp_multi_agent_api::response_event::{StreamFinished, stream_finished};
use warp_multi_agent_api::{
    ClientAction, Message, ResponseEvent, RunShellCommandResult, ShellCommandFinished, Task,
    client_action, response_event, run_shell_command_result,
};
use warpui::{App, AppContext};

use super::*;
use crate::ai::agent::api::serde_json_to_prost;
use crate::ai::agent::conversation::{AIConversation, ConversationDriver, ConversationStatus};
use crate::ai::agent::{AIAgentActionId, AIAgentInput, CancellationReason};
use crate::ai::blocklist::{CancellationRoute, QueuedQueryModel};
use crate::persistence::model::AgentConversationData;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

fn done() -> ResponseEvent {
    ResponseEvent {
        r#type: Some(response_event::Type::Finished(StreamFinished {
            reason: Some(stream_finished::Reason::Done(stream_finished::Done {})),
            ..Default::default()
        })),
    }
}

fn client_actions(actions: Vec<ClientAction>) -> ResponseEvent {
    ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions { actions },
        )),
    }
}

fn action(action: client_action::Action) -> ClientAction {
    ClientAction {
        action: Some(action),
    }
}

fn message(task_id: &str, request_id: &str, kind: MessageKind) -> Message {
    Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
        message: Some(kind),
        ..Default::default()
    }
}

fn add_messages(task_id: &str, messages: Vec<Message>) -> ClientAction {
    action(client_action::Action::AddMessagesToTask(
        AddMessagesToTask {
            task_id: task_id.to_owned(),
            messages,
        },
    ))
}

fn create_task(task_id: &str) -> ClientAction {
    action(client_action::Action::CreateTask(CreateTask {
        task: Some(Task {
            id: task_id.to_owned(),
            ..Default::default()
        }),
    }))
}

fn shell_tool_call(task_id: &str, request_id: &str, tool_call_id: &str) -> Message {
    message(
        task_id,
        request_id,
        MessageKind::ToolCall(ToolCall {
            tool_call_id: tool_call_id.to_owned(),
            tool: Some(Tool::RunShellCommand(RunShellCommand {
                command: "ls".to_owned(),
                ..Default::default()
            })),
        }),
    )
}

fn shell_tool_call_result(task_id: &str, request_id: &str, tool_call_id: &str) -> Message {
    #[allow(deprecated)]
    let result = RunShellCommandResult {
        command: "ls".to_owned(),
        output: "README.md".to_owned(),
        exit_code: 0,
        result: Some(run_shell_command_result::Result::CommandFinished(
            ShellCommandFinished {
                output: "README.md".to_owned(),
                exit_code: 0,
                command_id: tool_call_id.to_owned(),
                start_ts: None,
                finish_ts: None,
            },
        )),
    };
    message(
        task_id,
        request_id,
        MessageKind::ToolCallResult(ToolCallResult {
            tool_call_id: tool_call_id.to_owned(),
            context: None,
            result: Some(ToolResult::RunShellCommand(result)),
        }),
    )
}

fn root_task_id(ctx: &AppContext, id: AIConversationId) -> String {
    BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .get_root_task_id()
        .to_string()
}

fn user_query(task_id: &str, request_id: &str, query: &str) -> Message {
    message(
        task_id,
        request_id,
        MessageKind::UserQuery(UserQuery {
            query: query.into(),
            ..Default::default()
        }),
    )
}

fn status(ctx: &AppContext, id: AIConversationId) -> ConversationStatus {
    BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .status()
        .clone()
}

/// The cancellation reason of every root-task exchange, `None` for those not cancelled.
fn exchange_cancel_reasons(
    ctx: &AppContext,
    id: AIConversationId,
) -> Vec<Option<CancellationReason>> {
    BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .root_task_exchanges()
        .map(|exchange| exchange.output_status.cancel_reason().copied())
        .collect()
}

/// Opens a harness turn on `id` and echoes `query` into it, as the harness does before the
/// agent starts answering. The first turn also creates the root task, as the ACP mapper's
/// initial actions do.
fn begin_turn_with_query(
    controller: &mut BlocklistAIController,
    id: AIConversationId,
    query: &str,
    ctx: &mut ModelContext<BlocklistAIController>,
) -> ExternalHarnessTurn {
    let turn = controller
        .begin_external_harness_turn(id, Some("run-1".into()), ctx)
        .unwrap();
    let task_id = root_task_id(ctx, id);
    let root_is_optimistic = BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .get_root_task()
        .is_none_or(|task| task.source().is_none());
    let mut actions = Vec::new();
    if root_is_optimistic {
        actions.push(create_task(&task_id));
    }
    actions.push(add_messages(
        &task_id,
        vec![user_query(&task_id, &turn.request_id, query)],
    ));
    controller.apply_external_harness_event(&turn.stream_id, client_actions(actions), ctx);
    turn
}

#[test]
fn a_turn_cut_short_by_a_follow_up_ends_cancelled_with_the_status_still_in_progress() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = begin_turn_with_query(controller, id, "hello", ctx);
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                let reason = CancellationReason::FollowUpSubmitted {
                    is_for_same_conversation: true,
                };
                controller.cancel_external_harness_turn(&turn.stream_id, reason, ctx);
                assert_eq!(exchange_cancel_reasons(ctx, id), vec![Some(reason)]);
                assert_eq!(
                    status(ctx, id),
                    ConversationStatus::InProgress,
                    "the follow-up continues the conversation, so no terminal status is \
                     reported to the run"
                );

                // The follow-up's turn opens on the same conversation as usual.
                let next = begin_turn_with_query(controller, id, "steer", ctx);
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);
                controller.apply_external_harness_event(&next.stream_id, done(), ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Success);
                assert_eq!(exchange_cancel_reasons(ctx, id), vec![Some(reason), None]);
            });
        });
    });
}

#[test]
fn a_turn_cut_short_by_an_interrupt_ends_cancelled_and_marks_the_conversation_cancelled() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = begin_turn_with_query(controller, id, "hello", ctx);

                controller.cancel_external_harness_turn(
                    &turn.stream_id,
                    CancellationReason::ManuallyCancelled,
                    ctx,
                );
                assert_eq!(
                    exchange_cancel_reasons(ctx, id),
                    vec![Some(CancellationReason::ManuallyCancelled)]
                );
                assert_eq!(status(ctx, id), ConversationStatus::Cancelled);
            });
        });
    });
}

#[test]
fn a_cancel_falls_back_to_the_local_path_once_the_harness_stopped_listening() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                controller
                    .begin_external_harness_turn(id, Some("run-1".into()), ctx)
                    .unwrap();

                // The turn driver owns the receiver and dropped it when it stopped.
                let (sink, cancels) = async_channel::unbounded::<ExternalHarnessCancel>();
                controller.set_external_harness_cancel_sink(sink);
                drop(cancels);
                assert!(
                    !controller.cancel_via_external_harness(id, ctx),
                    "a stopped harness cannot take the cancellation"
                );
                assert_eq!(
                    controller.cancel_conversation_progress_routed(
                        id,
                        crate::ai::agent::CancellationReason::ManuallyCancelled,
                        ctx,
                    ),
                    CancellationRoute::Local,
                    "the caller must run its own cancel path instead"
                );

                // The driver releases the sinks when it stops, which closes the window for good.
                let (sink, _cancels) = async_channel::unbounded::<ExternalHarnessCancel>();
                controller.set_external_harness_cancel_sink(sink);
                controller.release_external_harness_sinks();
                assert!(!controller.cancel_via_external_harness(id, ctx));
            });
        });
    });
}

#[test]
fn queued_harness_prompts_are_shown_as_rows_until_hidden() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let prompt = ExternalHarnessPrompt {
                    text: "waiting".into(),
                    base: None,
                };
                let row = controller.show_external_harness_prompt_queued(id, &prompt, ctx);
                let queue = QueuedQueryModel::as_ref(ctx);
                assert_eq!(queue.queue(id).len(), 1);
                assert_eq!(queue.queue(id)[0].text(), "waiting");
                assert!(
                    queue.is_dispatch_blocked(id),
                    "the row is for display; nothing may dispatch it to Warp's agent"
                );

                controller.hide_external_harness_prompt_queued(id, row, ctx);
                assert!(!QueuedQueryModel::as_ref(ctx).has_queue(id));
            });
        });
    });
}

#[test]
fn a_turn_without_tool_calls_settles_when_its_stream_finishes() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = controller
                    .begin_external_harness_turn(id, Some("run-1".into()), ctx)
                    .unwrap();
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                let task_id = root_task_id(ctx, id);
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![
                        create_task(&task_id),
                        add_messages(
                            &task_id,
                            vec![message(
                                &task_id,
                                &turn.request_id,
                                MessageKind::UserQuery(UserQuery {
                                    query: "hello".into(),
                                    ..Default::default()
                                }),
                            )],
                        ),
                        add_messages(
                            &task_id,
                            vec![message(
                                &task_id,
                                &turn.request_id,
                                MessageKind::AgentOutput(AgentOutput {
                                    text: "hi there".into(),
                                }),
                            )],
                        ),
                    ]),
                    ctx,
                );
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                controller.apply_external_harness_event(&turn.stream_id, done(), ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Success);

                let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .unwrap();
                let queries: Vec<_> = conversation
                    .root_task_exchanges()
                    .flat_map(|exchange| exchange.input.iter())
                    .filter_map(|input| match input {
                        AIAgentInput::UserQuery { query, .. } => Some(query.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(queries, vec!["hello"]);
                assert_eq!(conversation.driver(), ConversationDriver::ExternalHarness);
            });
        });
    });
}

#[test]
fn tool_call_results_are_applied_as_finished_and_settle_in_the_next_stream() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let first = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                let task_id = root_task_id(ctx, id);
                controller.apply_external_harness_event(
                    &first.stream_id,
                    client_actions(vec![
                        create_task(&task_id),
                        add_messages(
                            &task_id,
                            vec![
                                message(
                                    &task_id,
                                    &first.request_id,
                                    MessageKind::UserQuery(UserQuery {
                                        query: "list files".into(),
                                        ..Default::default()
                                    }),
                                ),
                                shell_tool_call(&task_id, &first.request_id, "call-1"),
                            ],
                        ),
                    ]),
                    ctx,
                );
                controller.apply_external_harness_event(&first.stream_id, done(), ctx);
                assert_eq!(
                    status(ctx, id),
                    ConversationStatus::InProgress,
                    "a stream that announced a tool call leaves the turn open"
                );

                // The harness ran the tool itself; the result arrives in the next stream, as it
                // does from the MAA server.
                let second = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                controller.apply_external_harness_event(
                    &second.stream_id,
                    client_actions(vec![add_messages(
                        &task_id,
                        vec![
                            shell_tool_call_result(&task_id, &second.request_id, "call-1"),
                            message(
                                &task_id,
                                &second.request_id,
                                MessageKind::AgentOutput(AgentOutput {
                                    text: "There is one file.".into(),
                                }),
                            ),
                        ],
                    )]),
                    ctx,
                );
                assert!(
                    controller
                        .action_model
                        .as_ref(ctx)
                        .get_action_result(&AIAgentActionId::from("call-1".to_owned()))
                        .is_some(),
                    "the harness-supplied result must be recorded so the action model never \
                     tries to run the command locally"
                );

                controller.apply_external_harness_event(&second.stream_id, done(), ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Success);
            });
        });
    });
}

#[test]
fn a_tool_call_rewritten_by_a_message_update_is_stored_with_its_new_input() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = begin_turn_with_query(controller, id, "post in slack", ctx);
                let task_id = root_task_id(ctx, id);
                let mcp_call = |args: serde_json::Value| {
                    MessageKind::ToolCall(ToolCall {
                        tool_call_id: "call-1".to_owned(),
                        tool: Some(Tool::CallMcpTool(CallMcpTool {
                            name: "send_message_to_thread".to_owned(),
                            args: match serde_json_to_prost(args).unwrap().kind {
                                Some(prost_types::value::Kind::StructValue(args)) => Some(args),
                                other => panic!("expected a struct, got {other:?}"),
                            },
                            ..Default::default()
                        })),
                    })
                };
                let announced =
                    message(&task_id, &turn.request_id, mcp_call(serde_json::json!({})));
                let message_id = announced.id.clone();
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![add_messages(&task_id, vec![announced])]),
                    ctx,
                );
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![action(client_action::Action::UpdateTaskMessage(
                        UpdateTaskMessage {
                            task_id: task_id.clone(),
                            message: Some(Message {
                                id: message_id.clone(),
                                task_id: task_id.clone(),
                                request_id: turn.request_id.clone(),
                                message: Some(mcp_call(serde_json::json!({ "message": "Done" }))),
                                ..Default::default()
                            }),
                            mask: Some(prost_types::FieldMask {
                                paths: vec!["tool_call".to_owned()],
                            }),
                        },
                    ))]),
                    ctx,
                );

                let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .unwrap();
                let stored = conversation
                    .get_root_task()
                    .unwrap()
                    .messages()
                    .find(|message| message.id == message_id)
                    .expect("the announced call is stored");
                let Some(MessageKind::ToolCall(ToolCall {
                    tool: Some(Tool::CallMcpTool(call)),
                    ..
                })) = &stored.message
                else {
                    panic!("expected a CallMcpTool, got {:?}", stored.message);
                };
                assert!(call.args.as_ref().unwrap().fields.contains_key("message"));
            });
        });
    });
}

#[test]
fn a_turn_on_a_restored_conversation_extends_its_root_task_under_the_same_server_id() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let root = "restored-root";
                let restored = AIConversation::new_restored(
                    AIConversationId::new(),
                    vec![Task {
                        id: root.to_owned(),
                        messages: vec![
                            user_query(root, "prior-request", "before"),
                            message(
                                root,
                                "prior-request",
                                MessageKind::AgentOutput(AgentOutput {
                                    text: "earlier answer".into(),
                                }),
                            ),
                        ],
                        ..Default::default()
                    }],
                    Some(AgentConversationData {
                        server_conversation_token: Some("server-conversation".into()),
                        ..Default::default()
                    }),
                )
                .unwrap();
                let id = restored.id();
                let terminal_surface_id = controller.terminal_surface_id;
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                    history.restore_conversations(terminal_surface_id, vec![restored], ctx);
                });
                assert_eq!(
                    controller.bind_native_prompt_conversation(Some(id), ctx),
                    id
                );

                let turn = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                assert_eq!(root_task_id(ctx, id), root);
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![add_messages(
                        root,
                        vec![user_query(root, &turn.request_id, "after")],
                    )]),
                    ctx,
                );
                controller.apply_external_harness_event(&turn.stream_id, done(), ctx);

                let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .unwrap();
                assert_eq!(
                    conversation
                        .server_conversation_token()
                        .map(|token| token.as_str()),
                    Some("server-conversation")
                );
                let tasks: Vec<_> = conversation
                    .all_tasks()
                    .filter_map(|task| task.source_for_persistence())
                    .collect();
                assert_eq!(tasks.len(), 1, "the turn must not add a second root");
                let queries: Vec<_> = tasks[0]
                    .messages
                    .iter()
                    .filter_map(|message| match &message.message {
                        Some(MessageKind::UserQuery(query)) => Some(query.query.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(queries, vec!["before", "after"]);
            });
        });
    });
}

#[test]
fn cancelling_a_harness_driven_conversation_goes_to_the_cancel_sink() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let (sink, cancels) = async_channel::unbounded();
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                assert!(
                    !controller.cancel_via_external_harness(id, ctx),
                    "without a sink the caller cancels the conversation itself"
                );
                controller.set_external_harness_cancel_sink(sink);
                assert!(
                    !controller.cancel_via_external_harness(id, ctx),
                    "before the harness opens a turn the conversation is not its to cancel"
                );

                controller
                    .begin_external_harness_turn(id, Some("run-1".into()), ctx)
                    .unwrap();
                assert!(controller.cancel_via_external_harness(id, ctx));
                assert_eq!(cancels.try_recv().unwrap(), ExternalHarnessCancel);
                assert_eq!(
                    status(ctx, id),
                    ConversationStatus::InProgress,
                    "the harness settles the status once the agent answers the cancel"
                );

                // The generic cancel path routes the same way, so a local stop never reaches
                // the PTY.
                controller.cancel_conversation_progress(
                    id,
                    crate::ai::agent::CancellationReason::ManuallyCancelled,
                    ctx,
                );
                assert_eq!(cancels.try_recv().unwrap(), ExternalHarnessCancel);
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                controller.mark_external_harness_conversation_cancelled(id, ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Cancelled);
            });
        });
    });
}

#[test]
fn relayed_prompts_go_to_the_prompt_sink_instead_of_the_queue() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let (sink, prompts) = async_channel::unbounded();
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                controller.set_external_harness_prompt_sink(sink);
                assert!(
                    controller.relay_prompt_to_external_harness(ExternalHarnessPrompt {
                        text: "follow-up".into(),
                        base: None,
                    })
                );
                let relayed = prompts.try_recv().unwrap();
                assert_eq!(relayed.text, "follow-up");
                assert!(!QueuedQueryModel::as_ref(ctx).has_queue(id));
                assert_eq!(
                    BlocklistAIHistoryModel::as_ref(ctx)
                        .conversation(&id)
                        .unwrap()
                        .exchange_count(),
                    0,
                    "the harness owns the turn; nothing is sent to Warp's agent"
                );
            });
        });
    });
}
