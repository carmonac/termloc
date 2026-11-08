//! [`RootTuiView`]: the root view of the `warp-tui` front-end.
use warp::tui_export::{ServerId, UserWorkspaces};
use warp_core::user_preferences::GetUserPreferences as _;
use warpui::SingletonEntity as _;
use warpui_core::elements::tui::{TuiChildView, TuiElement};
use warpui_core::keymap::FixedBinding;
use warpui_core::keymap::macros::*;
use warpui_core::platform::TerminationMode;
use warpui_core::{
    AppContext, Entity, EntityId, FocusContext, TuiView, TypedActionView, ViewContext, ViewHandle,
    WindowId, keymap,
};

use crate::keybindings::TUI_BINDING_GROUP;
use crate::session_registry::TuiSessions;
use crate::terminal_session_view::TuiTerminalSessionView;
use crate::ui::terminal_starting;
const LAST_TEAM_STORAGE_KEY: &str = "TuiLastTeamUid";

/// Typed actions handled by [`RootTuiView`].
#[derive(Debug, Clone)]
pub enum RootTuiAction {
    /// Exits the app while no terminal session is focused.
    ExitApp,
}

/// The app-level TUI shell, projecting only the focused full session view.
pub struct RootTuiView;

/// Registers the root view's keybindings.
pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([FixedBinding::new(
        "ctrl-c",
        RootTuiAction::ExitApp,
        id!(RootTuiView::ui_name()),
    )
    .with_group(TUI_BINDING_GROUP)]);
}

impl RootTuiView {
    /// Creates the root view.
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let window_id = ctx.window_id();
        let team_uid = Self::restore_last_team_uid(ctx)
            .or_else(|| UserWorkspaces::as_ref(ctx).inherited_or_default_team_uid(None));
        UserWorkspaces::handle(ctx).update(ctx, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, team_uid, ctx);
        });
        Self
    }

    pub(crate) fn switch_window_to_team(
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut AppContext,
    ) {
        UserWorkspaces::handle(ctx).update(ctx, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, team_uid, ctx);
        });
        Self::store_last_team_uid(team_uid, ctx);
    }

    fn restore_last_team_uid(ctx: &AppContext) -> Option<ServerId> {
        ctx.private_user_preferences()
            .read_value(LAST_TEAM_STORAGE_KEY)
            .ok()
            .flatten()
            .and_then(|stored| serde_json::from_str::<ServerId>(&stored).ok())
    }

    fn store_last_team_uid(team_uid: ServerId, ctx: &AppContext) {
        let Ok(serialized) = serde_json::to_string(&team_uid) else {
            return;
        };
        let _ = ctx
            .private_user_preferences()
            .write_value(LAST_TEAM_STORAGE_KEY, serialized);
    }

    fn focused_session_view(&self, ctx: &AppContext) -> Option<ViewHandle<TuiTerminalSessionView>> {
        if !ctx.has_singleton_model::<TuiSessions>() {
            return None;
        }

        TuiSessions::as_ref(ctx)
            .focused_session()
            .map(|session| session.view().clone())
    }
}

impl Entity for RootTuiView {
    type Event = ();
}

impl TuiView for RootTuiView {
    fn ui_name() -> &'static str {
        "RootTuiView"
    }

    fn child_view_ids(&self, ctx: &AppContext) -> Vec<EntityId> {
        self.focused_session_view(ctx)
            .map(|view| vec![view.id()])
            .unwrap_or_default()
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused()
            && let Some(view) = self.focused_session_view(ctx)
        {
            view.update(ctx, |view, ctx| view.activate(ctx));
        }
    }
    fn render(&self, ctx: &AppContext) -> Box<dyn TuiElement> {
        self.focused_session_view(ctx)
            .map(|view| TuiChildView::new(&view).finish())
            .unwrap_or_else(terminal_starting)
    }

    fn keymap_context(&self, _ctx: &AppContext) -> keymap::Context {
        let mut context = keymap::Context::default();
        context.set.insert("RootTuiView");
        context
    }
}

impl TypedActionView for RootTuiView {
    type Action = RootTuiAction;

    fn handle_action(&mut self, action: &RootTuiAction, ctx: &mut ViewContext<Self>) {
        match action {
            RootTuiAction::ExitApp => {
                ctx.terminate_app(TerminationMode::ForceTerminate, None);
            }
        }
    }
}

#[cfg(test)]
#[path = "root_view_tests.rs"]
mod tests;
