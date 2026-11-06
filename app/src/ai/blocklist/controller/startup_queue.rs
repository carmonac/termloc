use warpui::{ModelContext, SingletonEntity};

use super::BlocklistAIController;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::{
    BlocklistAIHistoryModel, QueuedPromptDeliveryMode, QueuedQueryId, QueuedQueryModel,
};

impl BlocklistAIController {
    /// Binds this controller to a native conversation for the rest of an externally driven run.
    /// Idempotent: returns the existing binding if one is already in place.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub(crate) fn bind_native_prompt_conversation(
        &mut self,
        restored_conversation_id: Option<AIConversationId>,
        ctx: &mut ModelContext<Self>,
    ) -> AIConversationId {
        if let Some(id) = self.native_prompt_conversation_id {
            return id;
        }
        let id = restored_conversation_id.unwrap_or_else(|| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.start_new_conversation(self.terminal_surface_id, false, false, ctx)
            })
        });
        self.native_prompt_conversation_id = Some(id);
        log::info!(
            "event=native_queue_initialized terminal_id={:?} conversation_id={id} resumed={}",
            self.terminal_surface_id,
            restored_conversation_id.is_some(),
        );
        QueuedQueryModel::handle(ctx).update(ctx, |queue, ctx| {
            queue.begin_native_setup(id, ctx);
            queue.set_delivery_mode(id, QueuedPromptDeliveryMode::Steering);
        });
        id
    }

    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub(crate) fn native_prompt_conversation_id(&self) -> Option<AIConversationId> {
        self.native_prompt_conversation_id
    }

    /// Sends a prepared queued prompt. An explicit ID may interrupt; automatic sends preserve FIFO.
    pub(crate) fn dispatch_queued_warp_agent_prompt(
        &mut self,
        conversation_id: AIConversationId,
        query_id: Option<QueuedQueryId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let has_active_subagent = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|conversation| conversation.has_active_subagent());
        if has_active_subagent {
            log::info!(
                "event=native_queue_drain_deferred conversation_id={conversation_id} reason=active_subagent",
            );
            return;
        }
        let queue = QueuedQueryModel::as_ref(ctx);
        let row = match query_id {
            Some(query_id) => queue.ready_query(conversation_id, query_id),
            None => queue.ready_head(conversation_id),
        }
        .filter(|row| !row.is_command())
        .cloned();
        let Some(row) = row else {
            return;
        };
        let row_id = row.id();
        log::info!(
            "event=native_queue_row_dispatched terminal_id={:?} conversation_id={conversation_id} query_id={row_id:?}",
            self.terminal_surface_id,
        );
        // The send path resolves this row's attachments by id, so it must still be in the
        // queue when this is called; remove it only afterward.
        self.send_queued_user_query_in_conversation(
            row.text().to_owned(),
            conversation_id,
            row_id,
            ctx,
        );
        QueuedQueryModel::handle(ctx).update(ctx, |queue, ctx| {
            queue.remove_fired_row(conversation_id, row_id, ctx);
        });
    }
}

#[cfg(test)]
#[path = "startup_queue_tests.rs"]
mod tests;
