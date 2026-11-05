//! [`TuiSessions`]: registry and foreground selection for live TUI sessions.
//!
//! Sessions retain a terminal view with its manager. The container owns session
//! lifetime and focus; the root view renders and routes input only to the focused session.
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use pathfinder_geometry::vector::Vector2F;
use warp::tui_export::{
    AIConversationAutoexecuteMode, BannerState, GlobalResourceHandlesProvider,
    IsSharedSessionCreator, LocalTtyTerminalManager, PersistenceWriter, ServerConversationToken,
    TerminalManagerTrait, TerminalSurfaceResult,
};
use warpui::SingletonEntity;
use warpui_core::runtime::TuiDriverHandle;
use warpui_core::{AppContext, Entity, EntityId, ModelContext, ModelHandle, ViewHandle, WindowId};

use crate::resume::TuiExitSummaryHandle;
use crate::terminal_session_view::TuiTerminalSessionView;
use crate::transcript_view::TRANSCRIPT_BLOCK_SPACING;

/// Identifies a TUI terminal session.
///
/// A session and its eagerly-created view have the same lifetime, so the
/// view's entity id is also the terminal surface id used by shared AI models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TuiSessionId(EntityId);

impl TuiSessionId {
    /// The raw entity id used at shared-model boundaries.
    pub(crate) fn surface_id(self) -> EntityId {
        self.0
    }
}

/// A live TUI session and any resources required to retain it.
pub(crate) struct TuiSession {
    id: TuiSessionId,
    view: ViewHandle<TuiTerminalSessionView>,
    /// Keeps the session's PTY and event loop alive.
    _manager: ModelHandle<Box<dyn TerminalManagerTrait>>,
}

impl TuiSession {
    pub(crate) fn view(&self) -> &ViewHandle<TuiTerminalSessionView> {
        &self.view
    }
}

/// Events emitted as the session set or focus changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiSessionsEvent {
    /// A session was removed from the container.
    SessionRemoved(TuiSessionId),
    /// The focused session changed to this id.
    FocusChanged(TuiSessionId),
}

/// Owns all live TUI sessions and the focused-session selection.
pub(crate) struct TuiSessions {
    /// TUI-specific process driver. Its handle restores terminal mode on
    /// drop, so the app-lifetime session singleton must retain it.
    driver: Option<TuiDriverHandle>,
    keyboard_enhancement_supported: bool,
    exit_summary: TuiExitSummaryHandle,
    sessions: Vec<TuiSession>,
    focused_session_id: Option<TuiSessionId>,
    resume_token: Option<ServerConversationToken>,
    default_autoexecute_mode: AIConversationAutoexecuteMode,
}

impl Entity for TuiSessions {
    type Event = TuiSessionsEvent;
}

impl SingletonEntity for TuiSessions {}

impl TuiSessions {
    /// Creates and registers a full local terminal session.
    pub(crate) fn create_local_terminal_session(
        sessions: &ModelHandle<Self>,
        window_id: WindowId,
        focus: bool,
        handles_first_run_onboarding: bool,
        startup_directory: Option<PathBuf>,
        ctx: &mut AppContext,
    ) -> (TuiSessionId, ViewHandle<TuiTerminalSessionView>) {
        let (
            exit_summary,
            keyboard_enhancement_supported,
            default_autoexecute_mode,
            is_first_session,
        ) = sessions.read(ctx, |sessions, _| {
            (
                sessions.exit_summary.clone(),
                sessions.keyboard_enhancement_supported,
                sessions.default_autoexecute_mode,
                sessions.is_empty(),
            )
        });
        let initial_settings_file_error = is_first_session
            .then(|| {
                GlobalResourceHandlesProvider::as_ref(ctx)
                    .get()
                    .settings_file_error
                    .clone()
            })
            .flatten();
        // The manager uses this internal model for unsupported-shell state; the
        // TUI does not render a separate banner surface.
        let banner = ctx.add_model(|_| BannerState::default());
        let model_event_sender = PersistenceWriter::as_ref(ctx).sender();
        let manager = LocalTtyTerminalManager::<TuiTerminalSessionView>::create_tui_model(
            startup_directory,
            HashMap::<OsString, OsString>::from_iter(std::env::vars_os()),
            IsSharedSessionCreator::No,
            None,
            banner.clone(),
            Vector2F::new(120., 24.),
            model_event_sender,
            None,
            TRANSCRIPT_BLOCK_SPACING,
            ctx,
            move |surface_init, ctx| {
                let surface = ctx.add_typed_action_tui_view(window_id, |ctx| {
                    TuiTerminalSessionView::new(
                        surface_init,
                        exit_summary,
                        keyboard_enhancement_supported,
                        default_autoexecute_mode,
                        handles_first_run_onboarding,
                        initial_settings_file_error,
                        ctx,
                    )
                });
                TerminalSurfaceResult {
                    surface,
                    post_wire: move |_manager: &mut LocalTtyTerminalManager<
                        TuiTerminalSessionView,
                    >,
                                     _surface: &ViewHandle<TuiTerminalSessionView>,
                                     _ctx: &mut AppContext| {},
                }
            },
        );

        let surface = manager.surface.clone();
        let session_id =
            Self::register_session(sessions, manager.surface, manager.manager, focus, ctx);
        (session_id, surface)
    }

    /// Registers a session view with the manager that retains it.
    pub(crate) fn register_session(
        sessions: &ModelHandle<Self>,
        view: ViewHandle<TuiTerminalSessionView>,
        manager: ModelHandle<Box<dyn TerminalManagerTrait>>,
        focus: bool,
        ctx: &mut AppContext,
    ) -> TuiSessionId {
        let id = TuiSessionId(view.id());
        sessions.update(ctx, |sessions, ctx| {
            debug_assert!(
                sessions.session(id).is_none(),
                "a session must not be registered twice"
            );
            sessions.sessions.push(TuiSession {
                id,
                view,
                _manager: manager,
            });
            if focus {
                sessions.focus_session(id, ctx);
            }
            ctx.notify();
            id
        })
    }

    /// Creates the app's session container.
    pub(crate) fn new(
        driver: TuiDriverHandle,
        exit_summary: TuiExitSummaryHandle,
        resume_token: Option<ServerConversationToken>,
        default_autoexecute_mode: AIConversationAutoexecuteMode,
    ) -> Self {
        let keyboard_enhancement_supported = driver.keyboard_enhancement_supported();
        Self {
            driver: Some(driver),
            keyboard_enhancement_supported,
            exit_summary,
            sessions: Vec::new(),
            focused_session_id: None,
            resume_token,
            default_autoexecute_mode,
        }
    }

    /// Creates a driverless container for unit tests.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self {
            driver: None,
            keyboard_enhancement_supported: false,
            exit_summary: TuiExitSummaryHandle::default(),
            sessions: Vec::new(),
            focused_session_id: None,
            resume_token: None,
            default_autoexecute_mode: AIConversationAutoexecuteMode::RespectUserSettings,
        }
    }

    pub(crate) fn set_freeze_repaints_when_unfocused(&mut self, freeze: bool) {
        if let Some(driver) = self.driver.as_mut() {
            driver.set_freeze_repaints_when_unfocused(freeze);
        }
    }

    #[cfg(feature = "voice_input")]
    pub(crate) fn set_modifier_key_lifecycle_enabled(
        &mut self,
        enabled: bool,
        ctx: &mut ModelContext<Self>,
    ) -> std::io::Result<()> {
        let terminal_views = self
            .sessions
            .iter()
            .map(|session| session.view.clone())
            .collect::<Vec<_>>();
        // Reconfigure the terminal before the sessions react, so a failed write
        // leaves them consistent with the reporting that is still in effect.
        if let Some(driver) = self.driver.as_mut() {
            driver.set_modifier_key_lifecycle_enabled(enabled)?;
        }
        for view in terminal_views {
            view.update(ctx, |view, ctx| {
                view.handle_voice_hold_key_setting_changed(enabled, ctx);
            });
        }
        Ok(())
    }

    /// Removes every retained session without focusing an intermediate fallback.
    pub(crate) fn clear(&mut self, ctx: &mut ModelContext<Self>) {
        let removed_ids = self
            .sessions
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        self.focused_session_id = None;
        self.sessions.clear();
        for id in removed_ids {
            ctx.emit(TuiSessionsEvent::SessionRemoved(id));
        }
        ctx.notify();
    }
    /// Focuses a registered session. Returns whether focus changed.
    pub(crate) fn focus_session(&mut self, id: TuiSessionId, ctx: &mut ModelContext<Self>) -> bool {
        if self.focused_session_id == Some(id) || self.session(id).is_none() {
            return false;
        }
        self.focused_session_id = Some(id);
        let view = self
            .session(id)
            .expect("focused session was validated above")
            .view
            .clone();
        view.update(ctx, |view, ctx| view.activate(ctx));
        ctx.emit(TuiSessionsEvent::FocusChanged(id));
        ctx.notify();
        true
    }

    /// The focused session's id.
    pub(crate) fn focused_session_id(&self) -> Option<TuiSessionId> {
        self.focused_session_id
    }

    /// The focused session.
    pub(crate) fn focused_session(&self) -> Option<&TuiSession> {
        self.focused_session_id.and_then(|id| self.session(id))
    }

    /// Looks up a registered session.
    pub(crate) fn session(&self, id: TuiSessionId) -> Option<&TuiSession> {
        self.sessions.iter().find(|session| session.id == id)
    }
    /// Whether no session has been registered.
    pub(crate) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Consumes the startup resume token.
    pub(crate) fn take_resume_token(&mut self) -> Option<ServerConversationToken> {
        self.resume_token.take()
    }
}

#[cfg(test)]
#[path = "session_registry_tests.rs"]
mod tests;
