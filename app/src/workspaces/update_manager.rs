use std::sync::mpsc::SyncSender;

use anyhow::Context;
use warp_errors::report_if_error;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::user_workspaces::UserWorkspaces;
use super::workspace::WorkspaceUid;
use crate::persistence::ModelEvent;

/// Applies changes to the user's workspaces and saves them to the local database.
pub struct TeamUpdateManager {
    model_event_sender: Option<SyncSender<ModelEvent>>,
}

impl TeamUpdateManager {
    pub fn new(model_event_sender: Option<SyncSender<ModelEvent>>) -> Self {
        Self { model_event_sender }
    }

    #[cfg(test)]
    pub fn mock(_: &mut ModelContext<Self>) -> Self {
        Self::new(None)
    }

    pub fn set_current_workspace_uid(
        &mut self,
        workspace_uid: WorkspaceUid,
        ctx: &mut ModelContext<Self>,
    ) {
        UserWorkspaces::handle(ctx).update(ctx, |user_workspaces, ctx| {
            user_workspaces.set_current_workspace_uid(workspace_uid, ctx);
        });

        if let Some(sender) = &self.model_event_sender {
            report_if_error!(
                sender
                    .send(ModelEvent::SetCurrentWorkspace { workspace_uid })
                    .context("Unable to save the current workspace to sqlite")
            );
        }
    }
}

impl Entity for TeamUpdateManager {
    type Event = ();
}

impl SingletonEntity for TeamUpdateManager {}
