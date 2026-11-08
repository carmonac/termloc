use std::io;

use clap::Parser;
use warp::tui_export::register_tui_session_view_test_singletons;
use warpui::platform::WindowStyle;
use warpui::{AddWindowOptions, SingletonEntity};
use warpui_core::App;
use warpui_core::runtime::TuiDriverStartupError;

use super::{
    TuiArgs, ensure_terminal_session, handle_tui_driver_startup_error, parse_resume_token,
};
use crate::root_view::RootTuiView;
use crate::session_registry::TuiSessions;
use crate::test_fixtures::{add_test_semantic_selection, add_test_terminal_session};

#[test]
fn parses_resume_server_token() {
    let token = uuid::Uuid::new_v4().to_string();
    let args = TuiArgs::try_parse_from(["warp", "--resume", token.as_str(), "--auto-approve"])
        .expect("TUI launch arguments should parse together");

    assert_eq!(args.resume.as_deref(), Some(token.as_str()));
    assert!(args.auto_approve);
    assert_eq!(
        parse_resume_token(token.clone())
            .expect("UUID token should validate")
            .as_str(),
        token
    );
}

#[test]
fn terminal_bootstrap_is_idempotent_after_background_terminal_exists() {
    App::test((), |mut app| async move {
        register_tui_session_view_test_singletons(&mut app);
        add_test_semantic_selection(&mut app);
        let (window_id, root) = app.update(|ctx| {
            ctx.add_tui_window(
                AddWindowOptions {
                    window_style: WindowStyle::NotStealFocus,
                    ..Default::default()
                },
                RootTuiView::new,
            )
        });
        let sessions = app.add_singleton_model(|_| TuiSessions::new_for_test());
        let (surface, manager) = add_test_terminal_session(&mut app, window_id);
        app.update(|ctx| {
            TuiSessions::register_session(&sessions, surface, manager, true, ctx);
            ensure_terminal_session(&sessions, &root, ctx);
            ensure_terminal_session(&sessions, &root, ctx);
        });

        app.read(|ctx| assert_eq!(TuiSessions::as_ref(ctx).len(), 1));
    });
}
#[test]
fn terminal_disconnect_during_driver_startup_exits_without_error() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            handle_tui_driver_startup_error(
                TuiDriverStartupError::TerminalDisconnected(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "terminal disconnected",
                )),
                ctx,
            );
        });

        assert!(app.termination_result().is_none());
    });
}

#[test]
fn rejects_malformed_resume_server_token() {
    let error = parse_resume_token("not-a-token".to_owned())
        .expect_err("non-UUID token should be rejected");

    assert!(
        error
            .to_string()
            .contains("invalid server conversation token")
    );
}

#[test]
fn accepts_startup_without_resume() {
    let args = TuiArgs::try_parse_from(["warp"]).expect("empty arguments should parse");

    assert_eq!(args.resume, None);
    assert!(!args.auto_approve);
}

#[test]
fn version_flag_prints_cli_version() {
    let error = TuiArgs::try_parse_from(["warp", "--version"])
        .expect_err("--version should short-circuit clap parsing");

    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    // `run()` prints only CLI_VERSION (no binary-name precursor). Clap's
    // DisplayVersion payload still contains the configured version string.
    assert!(
        error.to_string().contains(super::CLI_VERSION),
        "--version should be backed by the configured CLI version"
    );
}
