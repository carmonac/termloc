//! The headless `warp-tui` front-end's session bootstrap.
//!
//! [`run`] boots the real headless Warp app via [`warp::run_tui`]. Once shared
//! initialization is done, the mount built here starts the TUI driver and
//! creates the first terminal session.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use i18n::t;
use warp::settings::{TuiThemeSettings, TuiZeroStateSettings, TuiZeroStateSettingsChangedEvent};
#[cfg(feature = "voice_input")]
use warp::settings::{TuiVoiceSettings, TuiVoiceSettingsChangedEvent};
use warp::tui_export::{AIConversationAutoexecuteMode, Appearance, ServerConversationToken};
use warp_core::channel::ChannelState;
use warp_core::settings::Setting as _;
use warp_errors::report_error;
use warpui::SingletonEntity as _;
use warpui_core::platform::{TerminationMode, WindowStyle};
use warpui_core::runtime::{TuiDriverStartupError, TuiFocusPolicy, spawn_tui_driver};
use warpui_core::{AddWindowOptions, AppContext, ModelHandle, ViewHandle};

use crate::resume::TuiExitSummaryHandle;
use crate::root_view::RootTuiView;
use crate::session_registry::{TuiSessions, TuiSessionsEvent};
use crate::terminal_background::probe_and_select_theme;
use crate::terminal_session_view::{
    TuiConversationRestoreOrigin, TuiConversationRestoreTarget, tui_resume_shell_command,
};
#[cfg(feature = "voice_input")]
use crate::voice_input::requires_modifier_key_reporting;

/// Version string printed by `--version`. Release builds get `GIT_RELEASE_TAG`;
/// local cargo builds fall back to a numeric placeholder.
const CLI_VERSION: &str = match option_env!("GIT_RELEASE_TAG") {
    Some(version) => version,
    None => "v0.0.0.0.0.0",
};

#[derive(Debug, Parser)]
#[command(name = "warp", version = CLI_VERSION)]
struct TuiArgs {
    #[command(subcommand)]
    command: Option<TuiCommand>,

    /// Resume a local agent conversation by server token.
    #[arg(long)]
    resume: Option<String>,

    /// Enable auto-approve by default for new conversations.
    #[arg(long)]
    auto_approve: bool,
}

/// One-shot commands available on the standalone TUI binaries. The full app
/// binary implements the same `dump-settings-schema` contract (see
/// `warp_cli::Command::DumpSettingsSchema`); this local copy lets
/// `script/run-tui` seed a real settings schema from whichever TUI binary is
/// about to run, without building the full GUI app binary just for that.
#[derive(Debug, Subcommand)]
enum TuiCommand {
    /// Print the JSON schema for the current Warp channel's settings and exit.
    DumpSettingsSchema {
        /// Write the schema to this path instead of standard output.
        output_path: Option<PathBuf>,
    },
}

/// Validates and wraps a server conversation token from the command line.
fn parse_resume_token(token: String) -> Result<ServerConversationToken> {
    uuid::Uuid::parse_str(&token)
        .with_context(|| format!("invalid server conversation token: {token}"))?;
    Ok(ServerConversationToken::new(token))
}

/// Boots the headless Warp app and mounts the transcript-capable TUI session.
pub fn run() -> Result<()> {
    // If this process was re-exec'd as a Warp worker (e.g. the terminal
    // server), dispatch that instead of starting another TUI — otherwise the
    // worker re-exec would recursively launch TUIs.
    if let Some(result) = warp::run_tui_worker_if_requested() {
        return result;
    }
    let args = match TuiArgs::try_parse() {
        Ok(args) => args,
        // Match the zero-state version line: bare tag/version, no binary name prefix.
        Err(error) if error.kind() == ErrorKind::DisplayVersion => {
            println!("{CLI_VERSION}");
            return Ok(());
        }
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(anyhow::Error::new(error)),
    };
    if let Some(TuiCommand::DumpSettingsSchema { output_path }) = args.command {
        warp::features::init_feature_flags();
        return warp::settings::dump_settings_schema(output_path.as_deref());
    }
    let resume_token = args.resume.map(parse_resume_token).transpose()?;
    let default_autoexecute_mode = if args.auto_approve {
        AIConversationAutoexecuteMode::RunToCompletion
    } else {
        AIConversationAutoexecuteMode::RespectUserSettings
    };
    let exit_summary = TuiExitSummaryHandle::default();
    let exit_summary_for_app = exit_summary.clone();
    let result = warp::run_tui(Box::new(move |ctx| {
        init(
            resume_token,
            default_autoexecute_mode,
            exit_summary_for_app,
            ctx,
        )
    }));
    if result.is_ok()
        && let Some(token) = exit_summary.token()
    {
        let token = token.as_str();
        println!("{}", t!("To continue this conversation, run:"));
        let command = tui_resume_shell_command(ChannelState::channel(), token);
        println!("{command}");
    }
    result
}

/// Creates the login-gated root and starts the headless draw and input driver.
fn init(
    resume_token: Option<ServerConversationToken>,
    default_autoexecute_mode: AIConversationAutoexecuteMode,
    exit_summary: TuiExitSummaryHandle,
    ctx: &mut AppContext,
) {
    // Register the TUI views' keybindings (and, in debug builds, the
    // cross-surface binding validators) before any input can be dispatched.
    crate::keybindings::init(ctx);

    crate::zero_state_animation::ZeroStateAnimationConfig::register(ctx);

    // Honor an explicit TUI theme or match the host terminal automatically.
    // Keep this scoped to the TUI process by overriding the already-initialized
    // Appearance theme at mount time, without changing normal GUI theme
    // selection or font settings.
    let selected_theme = TuiThemeSettings::as_ref(ctx).selected_theme();
    let theme = probe_and_select_theme(selected_theme);
    Appearance::handle(ctx).update(ctx, |appearance, ctx| {
        appearance.set_theme(theme, ctx);
    });

    let (window_id, root) = ctx.add_tui_window(
        AddWindowOptions {
            window_style: WindowStyle::NotStealFocus,
            ..Default::default()
        },
        RootTuiView::new,
    );
    #[cfg(feature = "voice_input")]
    let modifier_key_lifecycle_enabled = requires_modifier_key_reporting(ctx);
    #[cfg(not(feature = "voice_input"))]
    let modifier_key_lifecycle_enabled = false;
    let freeze_repaints_when_unfocused = *TuiZeroStateSettings::as_ref(ctx)
        .freeze_animation_when_unfocused
        .value();
    match spawn_tui_driver(
        ctx,
        window_id,
        root.clone(),
        TuiFocusPolicy::PresentedTree,
        modifier_key_lifecycle_enabled,
        freeze_repaints_when_unfocused,
    ) {
        Ok(driver) => {
            let sessions = ctx.add_singleton_model(|_| {
                TuiSessions::new(driver, exit_summary, resume_token, default_autoexecute_mode)
            });
            let sessions_for_zero_state_settings = sessions.clone();
            ctx.subscribe_to_model(
                &TuiZeroStateSettings::handle(ctx),
                move |settings, event, ctx| {
                    let TuiZeroStateSettingsChangedEvent::TuiZeroStateFreezeAnimationWhenUnfocusedSetting {
                        ..
                    } = event
                    else {
                        return;
                    };
                    let freeze =
                        *settings.as_ref(ctx).freeze_animation_when_unfocused.value();
                    sessions_for_zero_state_settings.update(ctx, |sessions, _| {
                        sessions.set_freeze_repaints_when_unfocused(freeze);
                    });
                    ctx.invalidate_all_views();
                },
            );
            #[cfg(feature = "voice_input")]
            let sessions_for_voice_settings = sessions.clone();
            #[cfg(feature = "voice_input")]
            ctx.subscribe_to_model(&TuiVoiceSettings::handle(ctx), move |_, event, ctx| {
                let TuiVoiceSettingsChangedEvent::TuiVoiceInputHoldKeySetting { .. } = event;
                let enabled = requires_modifier_key_reporting(ctx);
                let result = sessions_for_voice_settings.update(ctx, |sessions, ctx| {
                    sessions.set_modifier_key_lifecycle_enabled(enabled, ctx)
                });
                if let Err(error) = result {
                    report_error!(
                        anyhow::Error::new(error)
                            .context("failed to update TUI modifier key reporting")
                    );
                }
            });
            root.update(ctx, |_, ctx| {
                ctx.subscribe_to_model(&sessions, |_, _, event, ctx| match event {
                    TuiSessionsEvent::FocusChanged(_) => ctx.notify(),
                });
            });
            ensure_terminal_session(&sessions, &root, ctx);
        }
        Err(error) => handle_tui_driver_startup_error(error, ctx),
    }
}

fn handle_tui_driver_startup_error(error: TuiDriverStartupError, ctx: &mut AppContext) {
    match error {
        TuiDriverStartupError::TerminalDisconnected(error) => {
            log::error!("failed to start the TUI driver: {error}");
            ctx.terminate_app(TerminationMode::ForceTerminate, None);
        }
        TuiDriverStartupError::Unexpected(error) => {
            let error = anyhow::Error::new(error);
            report_error!(&error);
            ctx.terminate_app(TerminationMode::ForceTerminate, Some(Err(error)));
        }
    }
}

/// Creates the focused bootstrap session and restores the requested conversation.
fn ensure_terminal_session(
    sessions: &ModelHandle<TuiSessions>,
    root: &ViewHandle<RootTuiView>,
    ctx: &mut AppContext,
) {
    if sessions.read(ctx, |sessions, _| !sessions.is_empty()) {
        return;
    }

    let resume_token = sessions.update(ctx, |sessions, _| sessions.take_resume_token());
    let window_id = root.window_id(ctx);
    let handles_first_run_onboarding = resume_token.is_none();
    let (_, surface) = TuiSessions::create_local_terminal_session(
        sessions,
        window_id,
        true,
        handles_first_run_onboarding,
        std::env::current_dir().ok(),
        ctx,
    );
    surface.update(ctx, |view, ctx| {
        view.enable_cli_agent_osc_event_publishing(ctx);
    });
    if let Some(token) = resume_token {
        surface.update(ctx, |view, ctx| {
            view.restore_conversation(
                TuiConversationRestoreTarget::Server(token),
                TuiConversationRestoreOrigin::Startup,
                ctx,
            );
        });
    }
    root.update(ctx, |_, ctx| ctx.notify());
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
