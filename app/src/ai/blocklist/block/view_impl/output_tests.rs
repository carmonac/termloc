use std::time::{Duration, SystemTime};

use ai::agent::action::UseComputerRequest;
use ai::skills::{ParsedSkill, SkillProvider, SkillReference, SkillScope};
use computer_use::{Action, ScreenshotParams, Target, TargetedAction};
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::App;

use super::{
    RecordingCardText, read_skill_display_text, should_decorate_recorded_use_computer,
    start_recording_card_text, stop_recording_card_text, usage_pill_text,
};
use crate::ai::agent::{
    RecordingStarted, RecordingStopped, StartRecordingResult, StopRecordingResult,
};
use crate::settings::UsageDisplayUnit;
use crate::test_util::billing_unit::{set_charge_unit, set_usage_display_unit};
use crate::test_util::terminal::initialize_app_for_terminal_view;
use crate::workspaces::workspace::ChargeUnit;

/// The pill shows a cents-charged viewer who prefers dollars their figure in dollars whenever a
/// cents figure exists and falls back to credits otherwise; a credits-charged viewer sees credits
/// even when cents are present and the dollars preference is set.
#[test]
fn usage_pill_text_follows_the_display_unit() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        set_usage_display_unit(&mut app, UsageDisplayUnit::Dollars);

        app.read(|ctx| {
            assert_eq!(usage_pill_text(20.0, Some(36.0), ctx), "20 credits");
        });

        set_charge_unit(&mut app, ChargeUnit::Cents);
        app.read(|ctx| {
            assert_eq!(usage_pill_text(20.0, Some(36.0), ctx), "$0.36");
            assert_eq!(usage_pill_text(20.0, None, ctx), "20 credits");
        });
    });
}

#[test]
fn start_recording_card_text_uses_static_title_and_description_subtext() {
    let result = StartRecordingResult::Success(RecordingStarted {
        recording_id: "rec-1".to_string(),
        started_at: SystemTime::UNIX_EPOCH,
        width_px: 1280,
        height_px: 720,
    });

    let text = start_recording_card_text("Demo checkout flow", Some(&result));

    assert_eq!(
        text,
        RecordingCardText {
            primary: "Recording started".to_string(),
            subtext: Some("Demo checkout flow".to_string()),
        }
    );
}

#[test]
fn start_recording_card_text_includes_failure_copy() {
    let result = StartRecordingResult::Error("unsupported platform".to_string());

    let text = start_recording_card_text("Demo checkout flow", Some(&result));

    assert_eq!(
        text,
        RecordingCardText {
            primary: "Recording failed to start".to_string(),
            subtext: Some("unsupported platform".to_string()),
        }
    );
}

#[test]
fn stop_recording_card_text_includes_complete_duration() {
    let result = StopRecordingResult::Success(RecordingStopped {
        artifact_uid: "artifact-1".to_string(),
        duration: Duration::from_secs(2),
        width_px: 1280,
        height_px: 720,
        size_bytes: 42,
        completion_status: computer_use::RecordingCompletionStatus::Completed,
        termination_reason: "Stopped by agent".to_string(),
    });

    let text = stop_recording_card_text(Some(&result));

    assert_eq!(
        text,
        RecordingCardText {
            primary: "Recording saved".to_string(),
            subtext: Some("0:02".to_string()),
        }
    );
}

#[test]
fn stop_recording_card_text_includes_partial_duration_without_raw_reason() {
    let result = StopRecordingResult::Success(RecordingStopped {
        artifact_uid: "artifact-1".to_string(),
        duration: Duration::from_secs(12),
        width_px: 1280,
        height_px: 720,
        size_bytes: 42,
        completion_status: computer_use::RecordingCompletionStatus::StoppedEarly,
        termination_reason: "internal raw reason".to_string(),
    });

    let text = stop_recording_card_text(Some(&result));

    assert_eq!(
        text,
        RecordingCardText {
            primary: "Recording saved".to_string(),
            subtext: Some("Partial recording • 0:12".to_string()),
        }
    );
}

#[test]
fn use_computer_decoration_skips_screenshot_only_rows() {
    // Agents that only want a screenshot emit a zero-duration wait plus
    // screenshot params; a real wait is a captured interaction.
    let mut request = UseComputerRequest {
        action_summary: "Screenshot".to_string(),
        actions: vec![TargetedAction::screen(Action::Wait(Duration::ZERO))],
        screenshot_params: Some(ScreenshotParams {
            max_long_edge_px: None,
            max_total_px: None,
            region: None,
            target: Target::Screen,
        }),
    };
    assert!(!should_decorate_recorded_use_computer(&request));

    request.actions = vec![TargetedAction::screen(Action::Wait(Duration::from_secs(1)))];
    assert!(should_decorate_recorded_use_computer(&request));
}

fn make_skill(name: &str) -> ParsedSkill {
    ParsedSkill {
        name: name.to_string(),
        description: String::new(),
        path: LocalOrRemotePath::Local(
            std::path::PathBuf::from("/home/user/.agents/skills")
                .join(name)
                .join("SKILL.md"),
        ),
        content: String::new(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Home,
    }
}

#[test]
fn read_skill_display_text_shows_slash_command_when_skill_found() {
    let skill = make_skill("hello-world");
    let reference = SkillReference::Path(skill.path.clone());
    assert_eq!(
        read_skill_display_text(Some(&skill), &reference),
        "/hello-world"
    );
}

#[test]
fn read_skill_display_text_no_double_slash_when_skill_not_found_with_path_reference() {
    // When the skill is not in the manager the fallback is skill_reference.to_string(),
    // which for a path reference is an absolute path starting with '/'.  The display
    // text must NOT prepend an extra '/' — doing so would produce '//home/…'.
    let path = LocalOrRemotePath::Local(std::path::PathBuf::from(
        "/home/devbox/.termloc-local/skills/hello-world/SKILL.md",
    ));
    let reference = SkillReference::Path(path);
    let display = read_skill_display_text(None, &reference);
    assert!(
        !display.starts_with("//"),
        "display text must not start with '//': {display}"
    );
    assert!(
        display.starts_with('/'),
        "display text should start with '/': {display}"
    );
}

#[test]
fn read_skill_display_text_bundled_id_fallback_when_skill_not_found() {
    // The fallback uses the user-facing label (the bare id), not the canonical
    // `@warp-skill:<id>` reference form, so bundled-skill copy reads the same
    // way as path-based skill copy.
    let reference = SkillReference::BundledSkillId("create-pr".to_string());
    let display = read_skill_display_text(None, &reference);
    assert_eq!(display, "create-pr");
}
