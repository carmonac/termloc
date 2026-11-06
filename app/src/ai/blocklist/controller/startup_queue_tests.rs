use warp_core::features::FeatureFlag;
use warpui::App;

use super::*;
use crate::ai::agent::AIAgentInput;
use crate::ai::agent::conversation::ConversationStatus;
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::ai::blocklist::{QueuedQuery, QueuedQueryOrigin};
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

#[test]
fn local_steering_row_filed_while_idle_dispatches_only_from_a_clean_idle_state() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let controller = terminal.read(&app, |view, _| view.ai_controller().clone());
        let (id, task_id, streams) = controller.update(&mut app, |controller, ctx| {
            let id = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.start_new_conversation(terminal.id(), false, false, ctx)
            });
            controller.send_user_query_in_conversation("initial".into(), id, ctx);
            let task_id = BlocklistAIHistoryModel::as_ref(ctx)
                .conversation(&id)
                .unwrap()
                .get_root_task_id()
                .clone();
            let streams = controller
                .in_flight_response_streams
                .stream_ids_for_conversation(id, ctx);
            (id, task_id, streams)
        });
        let file_steering_row = |app: &mut App| {
            QueuedQueryModel::handle(app).update(app, |queue, ctx| {
                if let Some(row) = queue.pop_front(id, ctx) {
                    queue.append(id, row, ctx);
                } else {
                    queue.append(
                        id,
                        QueuedQuery::new("correction".into(), QueuedQueryOrigin::LocalSteering),
                        ctx,
                    );
                }
            });
        };
        // Mid-turn with a live stream: the row waits for a request boundary.
        file_steering_row(&mut app);
        controller.update(&mut app, |controller, ctx| {
            assert_eq!(
                controller
                    .in_flight_response_streams
                    .stream_ids_for_conversation(id, ctx),
                streams
            );
            assert_eq!(QueuedQueryModel::as_ref(ctx).queue(id).len(), 1);
            for stream in &streams {
                controller.in_flight_response_streams.cleanup_stream(stream);
            }
        });
        for status in [
            ConversationStatus::InProgress,
            ConversationStatus::Error,
            ConversationStatus::TransientError,
            ConversationStatus::Cancelled,
        ] {
            BlocklistAIHistoryModel::handle(&app).update(&mut app, |history, _| {
                history
                    .conversation_mut(&id)
                    .unwrap()
                    .set_status_for_test(status)
            });
            file_steering_row(&mut app);
            QueuedQueryModel::handle(&app).read(&app, |queue, _| {
                assert_eq!(queue.queue(id).len(), 1);
            });
        }
        controller.update(&mut app, |controller, ctx| {
            assert!(
                controller
                    .steer_head_prompt_for_request(id, &task_id, true, ctx)
                    .is_none()
            );
        });
        BlocklistAIHistoryModel::handle(&app).update(&mut app, |history, _| {
            history
                .conversation_mut(&id)
                .unwrap()
                .set_status_for_test(ConversationStatus::Success)
        });
        file_steering_row(&mut app);
        QueuedQueryModel::handle(&app).read(&app, |queue, _| {
            assert!(!queue.has_queue(id));
        });
        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                user_queries_in_order(history, id),
                ["initial", "correction"]
            );
        });
    });
}

/// The text of every `UserQuery` input across `id`'s root-task exchanges, in the order they
/// were appended to history.
fn user_queries_in_order(history: &BlocklistAIHistoryModel, id: AIConversationId) -> Vec<String> {
    history
        .conversation(&id)
        .unwrap()
        .root_task_exchanges()
        .flat_map(|exchange| exchange.input.iter())
        .filter_map(|input| match input {
            AIAgentInput::UserQuery { query, .. } => Some(query.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn dispatch_queued_warp_agent_prompt_with_an_explicit_id_targets_that_row_not_the_head() {
    // Regression test: "Send now" on a specific queued row must dispatch that exact row, even
    // when it isn't the head, rather than silently substituting whatever's at the head.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        let terminal = add_window_with_terminal(&mut app, None);
        let controller = terminal.read(&app, |terminal, _| terminal.ai_controller().clone());
        let (id, second_row_id) = controller.update(&mut app, |controller, ctx| {
            let id = controller.bind_native_prompt_conversation(None, ctx);
            QueuedQueryModel::handle(ctx).update(ctx, |queue, ctx| {
                for text in ["prompt2", "prompt3"] {
                    queue.append(
                        id,
                        QueuedQuery::new(text.into(), QueuedQueryOrigin::QueueSlashCommand),
                        ctx,
                    );
                }
            });
            let second_row_id = QueuedQueryModel::as_ref(ctx).queue(id)[1].id();
            (id, second_row_id)
        });
        terminal.update(&mut app, |terminal, ctx| {
            terminal.enter_agent_view(None, Some(id), AgentViewEntryOrigin::Cli, ctx);
        });

        controller.update(&mut app, |controller, ctx| {
            QueuedQueryModel::handle(ctx).update(ctx, |queue, ctx| {
                queue.finish_native_setup(id, ctx);
            });
            controller.dispatch_queued_warp_agent_prompt(id, Some(second_row_id), ctx);
        });

        QueuedQueryModel::handle(&app).read(&app, |queue, _| {
            assert_eq!(
                queue
                    .queue(id)
                    .iter()
                    .map(QueuedQuery::text)
                    .collect::<Vec<_>>(),
                vec!["prompt2"],
                "the targeted row (prompt3) should be dispatched and removed; prompt2 -- the \
                 head -- should be untouched"
            );
        });
        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                user_queries_in_order(history, id),
                vec!["prompt3".to_owned()],
                "the explicitly targeted row should have been sent"
            );
        });
    });
}

#[test]
fn dispatch_queued_warp_agent_prompt_respects_fifo_order() {
    // Regression test: the automatic (head-only) dispatch path dispatches the head row and leaves
    // the row behind it waiting.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        let terminal = add_window_with_terminal(&mut app, None);
        let controller = terminal.read(&app, |terminal, _| terminal.ai_controller().clone());
        let id = controller.update(&mut app, |controller, ctx| {
            let id = controller.bind_native_prompt_conversation(None, ctx);
            QueuedQueryModel::handle(ctx).update(ctx, |queue, ctx| {
                for text in ["local prompt", "second prompt"] {
                    queue.append(
                        id,
                        QueuedQuery::new(text.into(), QueuedQueryOrigin::QueueSlashCommand),
                        ctx,
                    );
                }
            });
            id
        });
        terminal.update(&mut app, |terminal, ctx| {
            terminal.enter_agent_view(None, Some(id), AgentViewEntryOrigin::Cli, ctx);
        });

        controller.update(&mut app, |controller, ctx| {
            controller.send_user_query_in_conversation("initial".into(), id, ctx);
            controller.dispatch_queued_warp_agent_prompt(id, None, ctx);
        });

        QueuedQueryModel::handle(&app).read(&app, |queue, _| {
            assert_eq!(
                queue
                    .queue(id)
                    .iter()
                    .map(QueuedQuery::text)
                    .collect::<Vec<_>>(),
                vec!["second prompt"],
                "the prompt at the head should have been dispatched, in FIFO order; the row \
                 behind it should still be waiting"
            );
        });
        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                user_queries_in_order(history, id),
                vec!["initial".to_owned(), "local prompt".to_owned()],
            );
        });
    });
}
