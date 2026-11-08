use std::collections::HashMap;

use ai::skills::SkillPathOrigin;
use warp_multi_agent_api as api;
use warp_multi_agent_api::client_action::{Action, MoveMessagesToNewTask};
use warp_multi_agent_api::message::request_metadata;
use warpui::{App, EntityId};

use super::*;
use crate::ai::agent::conversation::{AIConversation, AIConversationId};
use crate::ai::blocklist::ResponseStreamId;
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;

fn timestamp(seconds: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds, nanos: 0 }
}

fn duration(seconds: i64, nanos: i32) -> prost_types::Duration {
    prost_types::Duration { seconds, nanos }
}

fn record_message(request_id: &str, with_charges: bool) -> api::Message {
    let charges = with_charges.then(|| api::RequestCharges {
        usage_by_category: HashMap::from([(
            "primary_agent".to_string(),
            api::ChargedUsage {
                direct_api_inference_usage: HashMap::from([(
                    "Claude Sonnet 4.6".to_string(),
                    api::InferenceUsage {
                        token_count: Some(api::TokenCount {
                            input: 1000,
                            output: 200,
                            input_cache_read: 50,
                            input_cache_write: 0,
                        }),
                        token_cost: Some(api::TokenCost {
                            input_cost_in_cents: 0.30,
                            output_cost_in_cents: 0.30,
                            input_cache_read_cost_in_cents: 0.01,
                            input_cache_write_cost_in_cents: 0.0,
                            input_cost_in_credits: 0.10,
                            output_cost_in_credits: 0.20,
                            input_cache_read_cost_in_credits: 0.02,
                            input_cache_write_cost_in_credits: 0.0,
                        }),
                        web_search_count: 1,
                        web_search_cost_in_cents: 0.5,
                        web_search_cost_in_credits: 0.5,
                        ..Default::default()
                    },
                )]),
                byok_inference_usage: HashMap::new(),
                custom_endpoint_inference_usage: HashMap::new(),
                platform_usage_in_cents: 2.0,
                platform_usage_duration: Some(duration(90, 0)),
                platform_usage_in_credits: 1.0,
            },
        )]),
    });

    api::Message {
        id: format!("msg-{request_id}"),
        task_id: "root".to_string(),
        request_id: request_id.to_string(),
        timestamp: Some(timestamp(1_000_012)),
        message: Some(api::message::Message::RequestMetadata(
            api::message::RequestMetadata {
                timing: Some(api::RequestTiming {
                    request_timespan: Some(api::TimeSpan {
                        started_at: Some(timestamp(1_000_000)),
                        ended_at: Some(timestamp(1_000_012)),
                    }),
                    first_token_at: Some(timestamp(1_000_001)),
                    llm_generation_timespans: vec![api::TimeSpan {
                        started_at: Some(timestamp(1_000_004)),
                        ended_at: Some(timestamp(1_000_011)),
                    }],
                }),
                charges,
                tool_call_summary: Some(request_metadata::ToolCallSummary {
                    tool_calls: 4,
                    commands_executed: 2,
                    files_changed: 3,
                    lines_added: 40,
                    lines_removed: 8,
                }),
                context_window: Some(request_metadata::ContextWindow { usage: 0.42 }),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

#[test]
fn decodes_a_completed_record() {
    let record =
        RequestMetadataRecord::from_message(&record_message("req-1", true)).expect("record");

    assert_eq!(record.request_id, "req-1");
    assert_eq!(record.time_to_first_token_ms(), Some(1000));
    assert_eq!(record.request_duration_ms(), Some(12_000));
    assert_eq!(record.llm_generation_ms(), Some(7000));
    assert_eq!(record.total_tokens(), 1250);
    assert_eq!(record.model_charges.len(), 1);
    assert_eq!(
        record.model_charges[0].usage_type,
        InferenceUsageType::DirectApi
    );
    assert!((record.inference_cost_in_cents() - 1.11).abs() < 1e-5);
    assert!((record.inference_cost_in_credits() - 0.82).abs() < 1e-5);
    assert!((record.platform_cost_in_cents() - 2.0).abs() < 1e-5);
    assert!((record.platform_cost_in_credits() - 1.0).abs() < 1e-5);
    assert!((record.total_cost_in_cents() - 3.11).abs() < 1e-5);
    assert!((record.total_cost_in_credits() - 1.82).abs() < 1e-5);
    assert_eq!(record.platform_charges[0].duration_seconds, 90.0);
    assert_eq!(record.tool_calls, Some(4));
    assert_eq!(record.commands_executed, Some(2));
    assert_eq!(record.files_changed, Some(3));
    assert_eq!(record.lines_added, Some(40));
    assert_eq!(record.lines_removed, Some(8));
    assert_eq!(record.context_window_usage, Some(0.42));
}

#[test]
fn errored_record_has_no_charges_but_keeps_timing() {
    let record =
        RequestMetadataRecord::from_message(&record_message("req-err", false)).expect("record");

    assert!(record.model_charges.is_empty());
    assert!(record.platform_charges.is_empty());
    assert_eq!(record.total_cost_in_cents(), 0.0);
    assert_eq!(record.request_duration_ms(), Some(12_000));
}

#[test]
fn non_record_messages_are_ignored() {
    let message = api::Message {
        id: "m".to_string(),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: "hello".to_string(),
            },
        )),
        ..Default::default()
    };
    assert!(RequestMetadataRecord::from_message(&message).is_none());

    let messages = [
        message,
        record_message("req-1", true),
        record_message("req-2", false),
    ];
    let records: Vec<_> = messages
        .iter()
        .filter_map(RequestMetadataRecord::from_message)
        .collect();
    assert_eq!(
        records
            .iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        ["req-1", "req-2"]
    );
}

fn turn_messages(request_id: &str, seconds: i64) -> Vec<api::Message> {
    let user_query = api::Message {
        id: format!("query-{request_id}"),
        task_id: "root".to_string(),
        request_id: request_id.to_string(),
        timestamp: Some(timestamp(seconds)),
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: format!("question for {request_id}"),
            ..Default::default()
        })),
        ..Default::default()
    };
    let agent_output = api::Message {
        id: format!("output-{request_id}"),
        task_id: "root".to_string(),
        request_id: request_id.to_string(),
        timestamp: Some(timestamp(seconds + 1)),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: format!("answer for {request_id}"),
            },
        )),
        ..Default::default()
    };
    let mut record = record_message(request_id, true);
    record.timestamp = Some(timestamp(seconds + 2));
    vec![user_query, agent_output, record]
}

/// Every block reads its own turn's record, not only the newest one: after a restore, each
/// exchange must resolve to the record the server appended to *its* request.
#[test]
fn every_restored_exchange_resolves_its_own_record() {
    let mut messages = turn_messages("req-1", 1_000);
    messages.extend(turn_messages("req-2", 2_000));
    // The third turn was cancelled before its record reached this client.
    let mut cancelled = turn_messages("req-3", 3_000);
    cancelled.pop();
    messages.extend(cancelled);
    messages.extend(turn_messages("req-4", 4_000));

    let task = api::Task {
        id: "root".to_string(),
        messages,
        ..Default::default()
    };
    let conversation = AIConversation::new_restored(AIConversationId::new(), vec![task], None)
        .expect("restored conversation");

    let exchange_ids: Vec<_> = conversation
        .root_task_exchanges()
        .map(|exchange| exchange.id)
        .collect();
    assert_eq!(exchange_ids.len(), 4, "one exchange per request");

    let resolved: Vec<Vec<String>> = exchange_ids
        .iter()
        .map(|exchange_id| {
            conversation
                .request_metadata_records_for_exchange(*exchange_id)
                .into_iter()
                .map(|record| record.request_id)
                .collect()
        })
        .collect();
    assert_eq!(
        resolved,
        [
            vec!["req-1".to_string()],
            vec!["req-2".to_string()],
            vec![],
            vec!["req-4".to_string()],
        ]
    );
}

/// A tool-result round trip: the request the client sends after executing a tool call. It has no
/// user query, so it belongs to the turn started by the last user query before it.
fn tool_round_trip_messages(request_id: &str, seconds: i64) -> Vec<api::Message> {
    let tool_result = api::Message {
        id: format!("result-{request_id}"),
        task_id: "root".to_string(),
        request_id: request_id.to_string(),
        timestamp: Some(timestamp(seconds)),
        message: Some(api::message::Message::ToolCallResult(
            api::message::ToolCallResult {
                tool_call_id: format!("call-{request_id}"),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let agent_output = api::Message {
        id: format!("output-{request_id}"),
        task_id: "root".to_string(),
        request_id: request_id.to_string(),
        timestamp: Some(timestamp(seconds + 1)),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: format!("answer after tool for {request_id}"),
            },
        )),
        ..Default::default()
    };
    let mut record = record_message(request_id, true);
    record.timestamp = Some(timestamp(seconds + 2));
    vec![tool_result, agent_output, record]
}

/// Summarization (`Action::MoveMessagesToNewTask`) moves an exchange's messages into a subtask
/// while the exchange's client representation keeps naming them: the record lookup must
/// resolve message membership across the task store, or the icons disappear after a move.
#[test]
fn tool_round_trips_group_into_the_user_query_turn() {
    // Turn A: query (req-1) → tool round trip (req-2) → tool round trip (req-3).
    let mut messages = turn_messages("req-1", 1_000);
    messages.extend(tool_round_trip_messages("req-2", 2_000));
    messages.extend(tool_round_trip_messages("req-3", 3_000));
    // Turn B: a single-request query.
    messages.extend(turn_messages("req-4", 4_000));

    let task = api::Task {
        id: "root".to_string(),
        messages,
        ..Default::default()
    };
    let conversation = AIConversation::new_restored(AIConversationId::new(), vec![task], None)
        .expect("restored conversation");
    let exchange_ids: Vec<_> = conversation
        .root_task_exchanges()
        .map(|exchange| exchange.id)
        .collect();
    assert_eq!(exchange_ids.len(), 4, "one exchange per request");
    let [first, second, third, fourth] = exchange_ids[..] else {
        unreachable!()
    };

    for id in [first, second, third] {
        assert_eq!(
            conversation.turn_exchange_ids(id),
            vec![first, second, third]
        );
    }
    assert_eq!(conversation.turn_exchange_ids(fourth), vec![fourth]);

    assert!(!conversation.is_last_exchange_in_turn(first));
    assert!(!conversation.is_last_exchange_in_turn(second));
    assert!(conversation.is_last_exchange_in_turn(third));
    assert!(conversation.is_last_exchange_in_turn(fourth));

    let turn_a_request_ids: Vec<String> = conversation
        .request_metadata_records_for_turn(third)
        .into_iter()
        .map(|record| record.request_id)
        .collect();
    assert_eq!(turn_a_request_ids, ["req-1", "req-2", "req-3"]);
    // Asking via any exchange of the turn yields the same set.
    assert_eq!(
        conversation.request_metadata_records_for_turn(first),
        conversation.request_metadata_records_for_turn(third)
    );
    let turn_b_request_ids: Vec<String> = conversation
        .request_metadata_records_for_turn(fourth)
        .into_iter()
        .map(|record| record.request_id)
        .collect();
    assert_eq!(turn_b_request_ids, ["req-4"]);

    let turn_records = conversation.request_metadata_records_for_turn(third);
    let total_cost_in_cents: f32 = turn_records
        .iter()
        .map(|record| record.total_cost_in_cents())
        .sum();
    assert!((total_cost_in_cents - 3.0 * 3.11).abs() < 1e-4);
}

#[test]
fn records_resolve_after_a_summarization_move() {
    App::test((), |mut app| async move {
        let history_model =
            app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], vec![], &[]));

        let mut messages = turn_messages("req-1", 1_000);
        messages.extend(turn_messages("req-2", 2_000));
        let task = api::Task {
            id: "root".to_string(),
            messages,
            ..Default::default()
        };
        let mut conversation =
            AIConversation::new_restored(AIConversationId::new(), vec![task], None)
                .expect("restored conversation");
        let exchange_ids: Vec<_> = conversation
            .root_task_exchanges()
            .map(|exchange| exchange.id)
            .collect();
        let [first, second] = exchange_ids[..] else {
            unreachable!()
        };

        // Move the whole first turn into a summarization subtask, the way the server's
        // summarization sub-agent relocates earlier conversation messages.
        let action = Action::MoveMessagesToNewTask(MoveMessagesToNewTask {
            source_task_id: "root".to_string(),
            new_task: Some(api::Task {
                id: "summary-sub".to_string(),
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root".to_string(),
                }),
                ..Default::default()
            }),
            first_message_id: "query-req-1".to_string(),
            last_message_id: "msg-req-1".to_string(),
            expected_message_count: 3,
            replacement_messages: Vec::new(),
        });
        history_model.update(&mut app, |_, ctx| {
            conversation
                .apply_client_action(
                    &ResponseStreamId::new_for_test(),
                    EntityId::new(),
                    action,
                    &SkillPathOrigin::Unavailable,
                    ctx,
                )
                .expect("move should apply");
        });

        // Both exchanges still resolve their own records across the task store.
        for (exchange_id, request_id) in [(first, "req-1"), (second, "req-2")] {
            let resolved: Vec<String> = conversation
                .request_metadata_records_for_exchange(exchange_id)
                .into_iter()
                .map(|record| record.request_id)
                .collect();
            assert_eq!(resolved, [request_id.to_string()]);
        }
    });
}

/// After a restore (or fork), the moved messages live in the summary subtask and the exchange
/// is rebuilt there rather than in the root: the non-root singleton turn fallback plus the
/// cross-task lookup must still resolve the record.
#[test]
fn records_resolve_in_restored_summarized_history() {
    use crate::ai::agent::task::TaskId;
    use crate::test_util::ai_agent_tasks::create_subagent_tool_call_message;

    // The persisted shape after a summarization move: the root keeps the summarization
    // sub-agent call (the replacement message the move inserted), and the moved turn —
    // including its record — lives in the summary subtask.
    let mut summarization_call = create_subagent_tool_call_message(
        "call-sum",
        "root",
        "summary-sub",
        Some(warp_multi_agent_api::message::tool_call::subagent::Metadata::Summarization(())),
    );
    summarization_call.request_id = "req-sum".to_string();
    summarization_call.timestamp = Some(timestamp(900));

    let root_task = api::Task {
        id: "root".to_string(),
        messages: vec![summarization_call],
        ..Default::default()
    };
    let subtask = api::Task {
        id: "summary-sub".to_string(),
        messages: turn_messages("req-1", 1_000),
        dependencies: Some(api::task::Dependencies {
            parent_task_id: "root".to_string(),
        }),
        ..Default::default()
    };
    let conversation =
        AIConversation::new_restored(AIConversationId::new(), vec![root_task, subtask], None)
            .expect("restored conversation");

    // The moved turn's exchange was rebuilt in the subtask, not the root.
    let subtask = conversation
        .all_tasks()
        .find(|task| task.id() == &TaskId::new("summary-sub".to_string()))
        .expect("summary subtask in the task store");
    let exchanges: Vec<_> = subtask.exchanges().collect();
    assert_eq!(
        exchanges.len(),
        1,
        "the subtask rebuilds exactly one exchange"
    );
    let exchange = exchanges[0];

    // A non-root exchange is its own turn (the singleton fallback)...
    assert_eq!(
        conversation.turn_exchange_ids(exchange.id),
        vec![exchange.id]
    );
    // ...and its record still resolves across the task store.
    let resolved: Vec<String> = conversation
        .request_metadata_records_for_exchange(exchange.id)
        .into_iter()
        .map(|record| record.request_id)
        .collect();
    assert_eq!(resolved, ["req-1".to_string()]);
}
