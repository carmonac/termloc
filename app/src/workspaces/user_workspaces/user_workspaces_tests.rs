use std::cell::Cell;
use std::rc::Rc;

use regex::Regex;
use settings::{PrivatePreferences, PublicPreferences};
use warpui::elements::Empty;
use warpui::platform::WindowStyle;
use warpui::{AddSingletonModel, App, Element, TypedActionView, View, ViewHandle, WindowId};
use warpui_extras::user_preferences;

use super::*;
use crate::ai::execution_profiles::ActionPermission;
use crate::ai::llms::{AvailableLLMs, LLMInfo, MODELS_BY_FEATURE_CACHE_KEY};
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{CloudObject, CloudObjectGuest};
use crate::drive::sharing::{SharingAccessLevel, Subject, UserKind};
use crate::features::FeatureFlag;
use crate::network::NetworkStatus;
use crate::server::cloud_objects::update_manager::UpdateManager;
use crate::server::ids::ClientId;
use crate::server::server_api::ServerApiProvider;
use crate::server::sync_queue::SyncQueue;
use crate::settings::{
    AISettings, AgentModeCommandExecutionPredicate, CodeSettings, FocusedTerminalInfo,
};
use crate::system::SystemStats;
use crate::workflows::workflow::Workflow;
use crate::workflows::{CloudWorkflow, CloudWorkflowModel};
use crate::workspaces::team::{Team, TeamVisibility};
use crate::workspaces::team_tester::TeamTesterStatus;
use crate::workspaces::update_manager::TeamUpdateManager;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::{
    AdminEnablementSetting, EnforceableSetting, LinkSharingSettings, SandboxedAgentSettings,
    SplitListSetting, TeamLinkSharingSettings, Workspace,
};

#[derive(Default)]
struct CachedResources {
    workspaces: Vec<Workspace>,
}

fn initialize_app(app: &mut App, resources: CachedResources) {
    initialize_app_with_auth(app, resources, AuthStateProvider::new_for_test());
}

fn initialize_app_with_auth(
    app: &mut App,
    resources: CachedResources,
    auth_state_provider: AuthStateProvider,
) {
    // Add the necessary singleton models to the App
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
    app.add_singleton_model(TeamTesterStatus::new);
    app.add_singleton_model(SyncQueue::mock);
    app.add_singleton_model(CloudModel::mock);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(resources.workspaces, ctx));
    app.add_singleton_model(|_ctx| TeamUpdateManager::new(None));
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| auth_state_provider);
    app.add_singleton_model(|_| {
        PublicPreferences::new(Box::<user_preferences::in_memory::InMemoryPreferences>::default())
    });
    app.add_singleton_model(|_| {
        PrivatePreferences::new(Box::<user_preferences::in_memory::InMemoryPreferences>::default())
    });

    app.add_singleton_model(CodeSettings::new_with_defaults);
    app.add_singleton_model(AISettings::new_with_defaults);
    app.add_singleton_model(FocusedTerminalInfo::new);

    // The start of polling is normally triggered by authentication completion, but
    // we need to do it manually for tests.
    TeamTesterStatus::handle(app).update(app, |team_tester, ctx| {
        team_tester.initiate_data_pollers(false, ctx);
    });
}

fn initialize_window_team_test_app(app: &mut App, workspaces: Vec<Workspace>) {
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(workspaces, ctx));
}

#[test]
fn test_codebase_context_enabled_with_no_workspace() {
    App::test((), |mut app| async move {
        initialize_app(&mut app, CachedResources { workspaces: vec![] });

        app.read(|ctx| {
            let codebase_context_enabled =
                UserWorkspaces::as_ref(ctx).is_codebase_context_enabled(ctx);
            assert!(
                codebase_context_enabled,
                "codebase context should be on by default"
            );
        });
    })
}

fn team_for_test() -> Team {
    Team {
        uid: 123.into(),
        name: "test".to_string(),
        color: None,
        invite_link: None,
        members: vec![],
        pending_email_invites: vec![],
        invite_link_domain_restrictions: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        is_eligible_for_discovery: false,
        visibility: TeamVisibility::Open,
    }
}

fn workspace_for_test(team: &Team) -> Workspace {
    Workspace {
        uid: "workspace_uid123456789".to_string().into(),
        name: "test".to_string(),
        teams: vec![team.clone()],
        open_teams: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        invite_link_domain_restrictions: vec![],
        pending_email_invites: vec![],
        is_eligible_for_discovery: false,
        members: vec![],
    }
}

#[test]
fn test_window_team_assignment_is_immutable() {
    let first_team = team_for_test();
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    second_team.name = "second".to_string();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(second_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, second_team.uid, ctx);
            user_workspaces.set_team_for_window(window_id, first_team.uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(window_id),
                Some(second_team.uid)
            );
            assert_eq!(
                user_workspaces
                    .team_for_window(window_id)
                    .map(|team| team.uid),
                Some(second_team.uid)
            );
        });
    })
}

#[test]
fn test_window_team_assignment_inherits_from_source_or_default_team() {
    let first_team = team_for_test();
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(second_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let source_window_id = WindowId::new();
        let inherited_window_id = WindowId::new();
        let fallback_window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(source_window_id, second_team.uid, ctx);
            let inherited_team_uid =
                user_workspaces.inherited_or_default_team_uid(Some(source_window_id));
            let fallback_team_uid = user_workspaces.inherited_or_default_team_uid(None);
            user_workspaces.register_window(inherited_window_id, inherited_team_uid, ctx);
            user_workspaces.register_window(fallback_window_id, fallback_team_uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(inherited_window_id),
                Some(second_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(fallback_window_id),
                Some(first_team.uid)
            );
        });
    })
}

#[test]
fn warp_agent_cli_upgrade_link_is_channel_aware_and_user_bound() {
    let user_uid = UserUid::new("user-123");

    assert_eq!(
        UserWorkspaces::warp_agent_cli_upgrade_link(Some(user_uid)),
        format!(
            "{}{STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX}/user/{user_uid}?source=warp-agent-cli",
            ChannelState::server_root_url(),
        )
    );
}

#[test]
fn warp_agent_cli_upgrade_link_uses_channel_aware_fallback_without_a_user() {
    assert_eq!(
        UserWorkspaces::warp_agent_cli_upgrade_link(None),
        format!(
            "{}{STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX}?source=warp-agent-cli",
            ChannelState::server_root_url().trim_end_matches('/'),
        )
    );
}

#[test]
fn test_every_departed_team_window_falls_back_when_team_is_removed() {
    let first_team = team_for_test();
    let mut removed_team = team_for_test();
    removed_team.uid = 456.into();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(removed_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace.clone()]);
        let first_removed_team_window = WindowId::new();
        let second_removed_team_window = WindowId::new();
        let existing_fallback_team_window = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(first_removed_team_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(second_removed_team_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(existing_fallback_team_window, first_team.uid, ctx);
            workspace.teams.retain(|team| team.uid != removed_team.uid);
            user_workspaces.update_workspaces(vec![workspace], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(first_removed_team_window),
                Some(first_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(second_removed_team_window),
                Some(first_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(existing_fallback_team_window),
                Some(first_team.uid)
            );
        });
    })
}

#[test]
fn test_every_departed_team_window_becomes_teamless_without_remaining_memberships() {
    let removed_team = team_for_test();
    let mut workspace = workspace_for_test(&removed_team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace.clone()]);

        let first_window = WindowId::new();
        let second_window = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(first_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(second_window, removed_team.uid, ctx);
            workspace.teams.clear();
            user_workspaces.update_workspaces(vec![workspace], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(user_workspaces.team_uid_for_window(first_window), None);
            assert_eq!(user_workspaces.team_uid_for_window(second_window), None);
        });
    })
}

#[test]
fn test_window_team_assignment_reconciles_when_current_workspace_changes() {
    let first_team = team_for_test();
    let first_workspace = workspace_for_test(&first_team);
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    let mut second_workspace = workspace_for_test(&second_team);
    second_workspace.uid = "workspace_uid987654321".to_string().into();
    let second_workspace_uid = second_workspace.uid;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![first_workspace, second_workspace]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
            user_workspaces.set_current_workspace_uid(second_workspace_uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(second_team.uid)
            );
        });
    })
}

#[derive(Default)]
struct TeamContextTestView;

impl Entity for TeamContextTestView {
    type Event = ();
}

impl View for TeamContextTestView {
    fn ui_name() -> &'static str {
        "TeamContextTestView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

impl TypedActionView for TeamContextTestView {
    type Action = ();
}

fn create_test_window(app: &mut App) -> (WindowId, ViewHandle<TeamContextTestView>) {
    app.add_window(WindowStyle::NotStealFocus, |_| TeamContextTestView)
}

fn two_teams() -> (Team, Team) {
    let team_a = team_for_test();
    let mut team_b = team_for_test();
    team_b.uid = 456.into();
    team_b.name = "team-b".to_string();
    (team_a, team_b)
}

#[test]
fn test_team_context_for_operation_resolves_each_windows_own_team() {
    let (team_a, team_b) = two_teams();
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, view_a) = create_test_window(&mut app);
        let (window_b, view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        let context_a = view_a.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        let context_b = view_b.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        assert_eq!(
            context_a.team_uid(),
            Some(team_a.uid),
            "the view in window A should mint a context resolving to team A"
        );
        assert_eq!(
            context_b.team_uid(),
            Some(team_b.uid),
            "the view in window B should mint a context resolving to team B"
        );
    })
}

/// A window only changes teams by reconciling away from a team that left the workspace, so
/// that is also the only way to observe a captured context and a live render diverging.
#[test]
fn test_window_team_reconciliation_moves_rendering_but_not_a_captured_context() {
    let (team_a, team_b) = two_teams();
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        let weak_view = view.downgrade();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team_a.uid, ctx);
        });

        let context_a = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .team_context(&weak_view, ctx)
                    .team_uid(),
                Some(team_a.uid),
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace_for_test(&team_b)], ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .team_context(&weak_view, ctx)
                    .team_uid(),
                Some(team_b.uid),
                "a freshly resolved render context should follow the window to team B"
            );
        });
        assert_eq!(
            context_a.team_uid(),
            Some(team_a.uid),
            "a context captured for team A should keep pointing at team A rather than follow \
             the window onto team B"
        );
    })
}

fn set_team_remote_session_policy(team: &mut Team, allow_ai: bool, patterns: &[&str]) {
    team.settings
        .ai_permissions
        .allow_ai_in_remote_sessions
        .value = allow_ai;
    team.settings.ai_permissions.remote_session_regex_list = patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("test pattern should compile"))
        .collect();
}

fn set_workspace_remote_session_policy(
    workspace: &mut Workspace,
    allow_ai: bool,
    patterns: &[&str],
) {
    workspace
        .settings
        .ai_permissions_settings
        .allow_ai_in_remote_sessions = allow_ai;
    workspace
        .settings
        .ai_permissions_settings
        .remote_session_regex_list = patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("test pattern should compile"))
        .collect();
}

fn remote_session_patterns_for_surface(
    view: &ViewHandle<TeamContextTestView>,
    app: &AppContext,
) -> HashSet<String> {
    let user_workspaces = UserWorkspaces::as_ref(app);
    let scope = user_workspaces.team_context(&view.downgrade(), app);
    user_workspaces
        .get_remote_session_regex_list(&scope)
        .iter()
        .map(|regex| regex.as_str().to_string())
        .collect()
}

fn remote_session_ai_allowed_for_surface(
    view: &ViewHandle<TeamContextTestView>,
    app: &AppContext,
) -> bool {
    let user_workspaces = UserWorkspaces::as_ref(app);
    let scope = user_workspaces.team_context(&view.downgrade(), app);
    user_workspaces.is_ai_allowed_in_remote_sessions(&scope)
}

/// Each surface is judged by the rules of the team whose window it is in, so two terminals on
/// opposing teams get opposing answers at the same moment.
#[test]
fn remote_session_policy_follows_each_surfaces_own_team() {
    let (mut team_a, mut team_b) = two_teams();
    set_team_remote_session_policy(&mut team_a, true, &["^kubectl"]);
    set_team_remote_session_policy(&mut team_b, false, &["^ssh"]);
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams = vec![team_a.clone(), team_b.clone()];

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, view_a) = create_test_window(&mut app);
        let (window_b, view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        app.read(|ctx| {
            assert!(
                remote_session_ai_allowed_for_surface(&view_a, ctx),
                "the surface in team A's window is governed by team A, which permits it"
            );
            assert!(
                !remote_session_ai_allowed_for_surface(&view_b, ctx),
                "the surface in team B's window is governed by team B, which forbids it"
            );
            assert_eq!(
                remote_session_patterns_for_surface(&view_a, ctx),
                HashSet::from(["^kubectl".to_string()])
            );
            assert_eq!(
                remote_session_patterns_for_surface(&view_b, ctx),
                HashSet::from(["^ssh".to_string()])
            );
        });
    })
}

#[test]
fn remote_session_policy_falls_back_to_the_workspace_for_a_user_with_no_teams() {
    let team = team_for_test();
    let mut workspace = workspace_for_test(&team);
    workspace.teams.clear();
    set_workspace_remote_session_policy(&mut workspace, false, &["^workspace-only"]);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        app.read(|ctx| {
            assert!(
                !remote_session_ai_allowed_for_surface(&view, ctx),
                "the workspace value is genuinely team-neutral for a user with no teams, so it \
                 is the one to read"
            );
            assert_eq!(
                remote_session_patterns_for_surface(&view, ctx),
                HashSet::from(["^workspace-only".to_string()])
            );
        });
    })
}

/// Guards the shape of the getter rather than a reachable user scenario: a scope that names an
/// unresolvable team must fail closed, not fall through to the no-team branch. Mirrors
/// `member_byo_policy_denies_a_scope_naming_an_unresolvable_team`.
#[test]
fn remote_session_ai_permission_denies_a_scope_naming_an_unresolvable_team() {
    let mut team = team_for_test();
    set_team_remote_session_policy(&mut team, true, &["^kubectl"]);
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let unresolvable_team_scope = TeamContextForOperation::new_for_test(9999.into());
        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert!(
                !user_workspaces.is_ai_allowed_in_remote_sessions(&unresolvable_team_scope),
                "a team whose policy cannot be read must not inherit another team's"
            );
        });
    })
}

/// `current_workspace()` is `None` only when logged out or before the first metadata fetch, not
/// for a teamless user (who has a personal workspace). With no workspace there is no admin
/// policy to consult, so this permits -- preserving the pre-refactor default that the
/// unresolvable-team deny would otherwise have swallowed.
#[test]
fn remote_session_ai_permission_is_allowed_for_a_user_with_no_workspace_at_all() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        app.read(|ctx| {
            assert!(remote_session_ai_allowed_for_surface(&view, ctx));
            assert!(remote_session_patterns_for_surface(&view, ctx).is_empty());
        });
    })
}

fn team_named(uid: i64, name: &str) -> Team {
    let mut team = team_for_test();
    team.uid = uid.into();
    team.name = name.to_owned();
    team
}

/// Two teams in workspace order, so a test can tell the default apart from a chosen team.
fn platform_and_security() -> (Team, Team, Workspace) {
    let platform = team_named(123, "Platform");
    let security = team_named(456, "Security");
    let mut workspace = workspace_for_test(&platform);
    workspace.teams.push(security.clone());
    (platform, security, workspace)
}

#[test]
fn switching_a_window_to_a_team_overwrites_and_announces_it() {
    let (platform, security, workspace) = platform_and_security();

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let window_id = WindowId::new();
        let changes = Rc::new(Cell::new(0));
        let changes_for_subscription = changes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, event, _| {
                if matches!(event, UserWorkspacesEvent::WindowTeamChanged { .. }) {
                    changes_for_subscription.set(changes_for_subscription.get() + 1);
                }
            });
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, Some(platform.uid), ctx);
        });
        let changes_after_register = changes.get();

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, security.uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(security.uid),
                "switching must overwrite, unlike the insert-only registration paths"
            );
        });
        assert_eq!(
            changes.get(),
            changes_after_register + 1,
            "the switch must announce itself so scoped consumers resync"
        );
    })
}

#[test]
fn switching_a_window_to_its_current_team_announces_nothing() {
    let team = team_for_test();
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, Some(team.uid), ctx);
        });

        let changes = Rc::new(Cell::new(0));
        let changes_for_subscription = changes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, event, _| {
                if matches!(event, UserWorkspacesEvent::WindowTeamChanged { .. }) {
                    changes_for_subscription.set(changes_for_subscription.get() + 1);
                }
            });
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, team.uid, ctx);
        });

        assert_eq!(changes.get(), 0);
    })
}

/// The switcher's own visibility rule, which the TUI indicator reuses rather than
/// reimplementing so the two front-ends cannot disagree about who counts as multi-team.
#[test]
fn can_switch_teams_only_with_more_than_one_team() {
    let (_, _, two_team_workspace) = platform_and_security();
    let one_team_workspace = workspace_for_test(&team_for_test());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![one_team_workspace]);
        app.read(|ctx| assert!(!UserWorkspaces::as_ref(ctx).can_switch_teams()));

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![two_team_workspace], ctx);
        });
        app.read(|ctx| assert!(UserWorkspaces::as_ref(ctx).can_switch_teams()));
    })
}

#[test]
fn test_team_contexts_represent_a_registered_teamless_window() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        let context = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        assert_eq!(context.team_uid(), None);

        let weak_view = view.downgrade();
        app.read(|ctx| {
            let context = UserWorkspaces::as_ref(ctx).team_context(&weak_view, ctx);
            assert_eq!(context.team_uid(), None);
        });
    })
}

fn autonomy_setting(value: ActionPermission) -> EnforceableSetting<Option<ActionPermission>> {
    EnforceableSetting {
        value: Some(value),
        is_enforced_by_workspace: false,
    }
}

/// A team whose admins enforce `execute_commands`, so the team is distinguishable from one
/// that enforces nothing and from a team enforcing something else.
fn team_enforcing_execute_commands(team: &Team, value: ActionPermission) -> Team {
    let mut team = team.clone();
    team.settings.ai_autonomy.execute_commands = autonomy_setting(value);
    team
}

#[test]
fn test_ai_autonomy_settings_resolve_each_windows_own_team() {
    let (team_a, team_b) = two_teams();
    let team_a = team_enforcing_execute_commands(&team_a, ActionPermission::AlwaysAsk);
    let team_b = team_enforcing_execute_commands(&team_b, ActionPermission::AlwaysAllow);
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, view_a) = create_test_window(&mut app);
        let (window_b, view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        let scope_a = view_a.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        let scope_b = view_b.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces
                    .ai_autonomy_settings(&scope_a)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAsk),
                "the window on team A should read team A's policy"
            );
            assert_eq!(
                user_workspaces
                    .ai_autonomy_settings(&scope_b)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAllow),
                "the window on team B should read team B's policy"
            );
        });
    })
}

#[test]
fn test_ai_autonomy_settings_for_a_teamless_window_fall_back_to_the_workspace() {
    let team = team_enforcing_execute_commands(&team_for_test(), ActionPermission::AlwaysAsk);
    let mut workspace = workspace_for_test(&team);
    workspace
        .settings
        .ai_autonomy_settings
        .execute_commands_setting = Some(ActionPermission::AlwaysAllow);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        assert_eq!(scope.team_uid(), None);

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .ai_autonomy_settings(&scope)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAllow),
                "a window on no team falls back to the workspace policy"
            );
        });
    })
}

#[test]
fn test_ai_autonomy_settings_fall_back_to_the_workspace_for_a_user_with_no_teams() {
    let mut workspace = workspace_for_test(&team_for_test());
    workspace.teams.clear();
    workspace
        .settings
        .ai_autonomy_settings
        .execute_commands_setting = Some(ActionPermission::AlwaysAllow);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .ai_autonomy_settings(&scope)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAllow),
                "with no teams at all the workspace layer is genuinely team-neutral, so it is \
                 the right fallback"
            );
        });
    })
}

/// A window only changes teams by reconciling away from a team that left the workspace, so
/// that is also the only way to observe a captured scope and a freshly resolved one
/// disagreeing about which policy applies.
#[test]
fn test_a_captured_autonomy_scope_does_not_follow_its_window_to_another_team() {
    let (team_a, team_b) = two_teams();
    let team_a = team_enforcing_execute_commands(&team_a, ActionPermission::AlwaysAsk);
    let team_b = team_enforcing_execute_commands(&team_b, ActionPermission::AlwaysAllow);
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team_a.uid, ctx);
        });

        let captured = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .ai_autonomy_settings(&captured)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAsk),
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace_for_test(&team_b)], ctx);
        });

        let weak_view = view.downgrade();
        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces
                    .ai_autonomy_settings(&captured)
                    .execute_commands_setting,
                None,
                "the captured scope's team is gone, so it imposes no policy rather than \
                 silently adopting team B's"
            );

            let resolved = user_workspaces.team_context(&weak_view, ctx);
            assert_eq!(
                user_workspaces
                    .ai_autonomy_settings(&resolved)
                    .execute_commands_setting,
                Some(ActionPermission::AlwaysAllow),
                "a freshly resolved scope follows the window onto team B"
            );
        });
    })
}

/// A list no admin layer contributed to is not an override. This is also the case the
/// client cannot yet tell apart from a layer explicitly configuring an empty list, which
/// needs `StringListSettingInfo.isConfigured` from the server.
#[test]
fn test_an_unconfigured_team_list_setting_is_not_an_override() {
    let mut team = team_for_test();
    team.settings.ai_autonomy.execute_commands_allowlist = SplitListSetting::default();
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team.uid, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            let settings = UserWorkspaces::as_ref(ctx).ai_autonomy_settings(&scope);
            assert!(
                !settings.has_override_for_execute_commands_allowlist(),
                "an admin who has configured no allowlist entries is not enforcing an empty \
                 allowlist"
            );
        });
    })
}

/// The merged `values` is the whole policy: a list contributed to by both admin layers reads
/// as one override of exactly the merged `values`, not the layers concatenated.
///
/// The layers deliberately overlap on `ls`, so `values` and `workspace_entries ++
/// team_entries` differ in length. With disjoint layers the two are the same list and this
/// test would pass against an implementation that concatenated.
#[test]
fn test_a_team_list_setting_overrides_with_its_merged_values() {
    let mut team = team_for_test();
    team.settings.ai_autonomy.execute_commands_allowlist = SplitListSetting {
        values: vec!["ls".to_string(), "git status".to_string()],
        workspace_entries: vec!["ls".to_string()],
        team_entries: vec!["ls".to_string(), "git status".to_string()],
    };
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team.uid, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            let settings = UserWorkspaces::as_ref(ctx).ai_autonomy_settings(&scope);
            assert!(settings.has_override_for_execute_commands_allowlist());
            let allowlist = settings
                .execute_commands_allowlist
                .expect("a non-empty list is an override");
            assert_eq!(
                allowlist.len(),
                2,
                "the merged values are the policy, so entries are not counted once per layer"
            );
            assert!(allowlist.iter().any(|predicate| predicate.matches("ls")));
            assert!(
                allowlist
                    .iter()
                    .any(|predicate| predicate.matches("git status"))
            );
        });
    })
}

/// Allowlists merge by intersection, so two admin layers that share no entries produce an
/// empty `values` while both layers plainly configured one. That is an override permitting
/// nothing, and reading it as "no override" would hand the decision back to the user's own
/// profile — the permissive answer to a deny-by-default setting.
#[test]
fn test_disjoint_admin_allowlists_are_an_override_that_permits_nothing() {
    let mut team = team_for_test();
    team.settings.ai_autonomy.execute_commands_allowlist = SplitListSetting {
        values: vec![],
        workspace_entries: vec!["ls".to_string()],
        team_entries: vec!["git status".to_string()],
    };
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team.uid, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            let settings = UserWorkspaces::as_ref(ctx).ai_autonomy_settings(&scope);
            assert!(
                settings.has_override_for_execute_commands_allowlist(),
                "both layers configured an allowlist, so the intersection being empty is an \
                 override rather than an absence"
            );
            assert_eq!(
                settings
                    .execute_commands_allowlist
                    .expect("a configured list is an override")
                    .len(),
                0,
                "the layers agree on nothing, so nothing is allowlisted"
            );
        });
    })
}

fn team_with_sandboxed_denylist(team: &Team, patterns: &[&str]) -> Team {
    let mut team = team.clone();
    team.settings.sandboxed_agent.execute_commands_denylist = SplitListSetting {
        values: patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
        ..Default::default()
    };
    team
}

fn denylist_patterns(denylist: Vec<AgentModeCommandExecutionPredicate>) -> Vec<String> {
    denylist.iter().map(ToString::to_string).collect()
}

#[test]
fn test_sandboxed_agent_denylist_resolves_the_scopes_own_team() {
    let (team_a, team_b) = two_teams();
    let team_a = team_with_sandboxed_denylist(&team_a, &["rm .*"]);
    let team_b = team_with_sandboxed_denylist(&team_b, &["curl .*"]);
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, view_a) = create_test_window(&mut app);
        let (window_b, view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        let scope_a = view_a.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        let scope_b = view_b.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let denylist_a = user_workspaces
                .sandboxed_agent_execute_commands_denylist_for_scope(&scope_a)
                .expect("team A configured a denylist");
            assert_eq!(
                denylist_patterns(denylist_a),
                ["rm .*"],
                "the window on team A should read team A's denylist"
            );

            let denylist_b = user_workspaces
                .sandboxed_agent_execute_commands_denylist_for_scope(&scope_b)
                .expect("team B configured a denylist");
            assert_eq!(
                denylist_patterns(denylist_b),
                ["curl .*"],
                "the window on team B should read team B's denylist"
            );
        });
    })
}

/// Mirrors [`member_byo_policy_denies_a_scope_naming_an_unresolvable_team`] for the sandboxed
/// denylist: a scope naming a team that cannot be resolved must not fall through to another
/// team's (or the workspace's) denylist.
#[test]
fn test_sandboxed_agent_denylist_denies_a_scope_naming_an_unresolvable_team() {
    let (team_a, _team_b) = two_teams();
    let team_a = team_with_sandboxed_denylist(&team_a, &["rm .*"]);
    let workspace = workspace_for_test(&team_a);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let unresolvable_team_scope = TeamContextForOperation::new_for_test(9999.into());
        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .sandboxed_agent_execute_commands_denylist_for_scope(&unresolvable_team_scope),
                None,
                "a team whose denylist cannot be read must not inherit another team's"
            );
        });
    })
}

/// A list no admin layer contributed to is not an override, the same distinction
/// [`test_an_unconfigured_team_list_setting_is_not_an_override`] locks in for the AI autonomy
/// allowlists. Getting this wrong here would silently drop a team's denylist protection.
#[test]
fn test_sandboxed_agent_denylist_is_none_when_the_team_has_not_configured_one() {
    let team = team_for_test();
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team.uid, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .sandboxed_agent_execute_commands_denylist_for_scope(&scope),
                None,
                "an unconfigured team denylist must not read as an override with an empty list"
            );
        });
    })
}

#[test]
fn test_sandboxed_agent_denylist_falls_back_to_the_workspace_for_a_user_with_no_teams() {
    let mut workspace = workspace_for_test(&team_for_test());
    workspace.teams.clear();
    workspace.settings.sandboxed_agent_settings = Some(SandboxedAgentSettings {
        execute_commands_denylist: Some(vec![
            AgentModeCommandExecutionPredicate::new_regex("git .*").unwrap(),
        ]),
    });

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        let scope = view.update(&mut app, |_, ctx| {
            UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx)
        });
        assert_eq!(scope.team_uid(), None);

        app.read(|ctx| {
            let denylist = UserWorkspaces::as_ref(ctx)
                .sandboxed_agent_execute_commands_denylist_for_scope(&scope)
                .expect("with no teams at all the workspace's denylist is genuinely team-neutral");
            assert_eq!(denylist_patterns(denylist), ["git .*"]);
        });
    })
}

fn two_teams_with_opposing_link_sharing_policy() -> (Team, Team) {
    fn link_sharing_settings(permitted: bool) -> TeamLinkSharingSettings {
        let setting = EnforceableSetting {
            value: permitted,
            is_enforced_by_workspace: false,
        };
        TeamLinkSharingSettings {
            anyone_with_link_sharing_enabled: setting.clone(),
            direct_link_sharing_enabled: setting,
        }
    }

    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.link_sharing = link_sharing_settings(true);
    team_b.settings.link_sharing = link_sharing_settings(false);
    (team_a, team_b)
}

#[test]
fn link_sharing_follows_each_scopes_team() {
    let (team_a, team_b) = two_teams_with_opposing_link_sharing_policy();
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope_a = TeamContextForOperation::new_for_test(team_a.uid);
            let scope_b = TeamContextForOperation::new_for_test(team_b.uid);

            assert!(user_workspaces.is_anyone_with_link_sharing_enabled(&scope_a));
            assert!(user_workspaces.is_direct_link_sharing_enabled(&scope_a));
            assert!(!user_workspaces.is_anyone_with_link_sharing_enabled(&scope_b));
            assert!(!user_workspaces.is_direct_link_sharing_enabled(&scope_b));
        });
    })
}

fn assert_teamless_scope_reads_workspace_link_sharing_policy(permitted: bool) {
    let (_team_a, team_b) = two_teams_with_opposing_link_sharing_policy();
    let mut workspace = workspace_for_test(&team_b);
    workspace.teams.clear();
    workspace.settings.link_sharing_settings = LinkSharingSettings {
        anyone_with_link_sharing_enabled: permitted,
        direct_link_sharing_enabled: permitted,
    };

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope = TeamlessScopeForTest;
            assert_eq!(
                user_workspaces.is_anyone_with_link_sharing_enabled(&scope),
                permitted
            );
            assert_eq!(
                user_workspaces.is_direct_link_sharing_enabled(&scope),
                permitted
            );
        });
    })
}

#[test]
fn link_sharing_for_a_teamless_scope_follows_a_permissive_workspace() {
    assert_teamless_scope_reads_workspace_link_sharing_policy(true);
}

#[test]
fn link_sharing_for_a_teamless_scope_follows_a_restrictive_workspace() {
    assert_teamless_scope_reads_workspace_link_sharing_policy(false);
}

#[test]
fn link_sharing_fails_open_for_an_unresolvable_team() {
    let (_team_a, team_b) = two_teams_with_opposing_link_sharing_policy();
    let mut workspace = workspace_for_test(&team_b);
    workspace.settings.link_sharing_settings = LinkSharingSettings {
        anyone_with_link_sharing_enabled: false,
        direct_link_sharing_enabled: false,
    };

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let scope = TeamContextForOperation::new_for_test(9999.into());
        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert!(user_workspaces.is_anyone_with_link_sharing_enabled(&scope));
            assert!(user_workspaces.is_direct_link_sharing_enabled(&scope));
        });
    })
}

#[test]
fn link_sharing_fails_open_without_a_workspace() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope = TeamlessScopeForTest;
            assert!(user_workspaces.is_anyone_with_link_sharing_enabled(&scope));
            assert!(user_workspaces.is_direct_link_sharing_enabled(&scope));
        });
    })
}

#[test]
fn test_spaces_for_window_orders_selected_team_shared_and_personal() {
    let _flag = FeatureFlag::SharedWithMe.override_enabled(true);
    let first_team = team_for_test();
    let mut selected_team = team_for_test();
    selected_team.uid = 456.into();
    selected_team.name = "selected".to_string();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(selected_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, selected_team.uid, ctx);
        });

        let current_user_uid = app.read(|ctx| {
            AuthStateProvider::as_ref(ctx)
                .get()
                .user_id()
                .expect("test user should be authenticated")
        });
        let mut shared_object = CloudWorkflow::new_local(
            CloudWorkflowModel {
                data: Workflow::new("shared workflow", "echo shared"),
            },
            Owner::User {
                user_uid: UserUid::new("other-user"),
            },
            None,
            ClientId::default(),
        );
        shared_object
            .permissions_mut()
            .guests
            .push(CloudObjectGuest {
                subject: Subject::User(UserKind::Account(current_user_uid)),
                access_level: SharingAccessLevel::View,
                source: None,
            });
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(shared_object.id, shared_object);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).spaces_for_window(window_id, ctx),
                vec![
                    Space::Team {
                        team_uid: selected_team.uid
                    },
                    Space::Shared,
                    Space::Personal,
                ]
            );
        });
    })
}
#[test]
fn test_unassigned_window_is_initialized_after_workspace_metadata_loads() {
    let team = team_for_test();
    let workspace = workspace_for_test(&team);
    let workspace_uid = workspace.uid;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });
        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                None
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace], ctx);
            user_workspaces.set_current_workspace_uid(workspace_uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(team.uid)
            );
        });
    })
}

#[test]
fn test_codebase_context_enabled_when_all_teams_enable_it() {
    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.codebase_context.value = AdminEnablementSetting::Enable;
    team_b.settings.codebase_context.value = AdminEnablementSetting::Enable;
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b);

    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            CachedResources {
                workspaces: vec![workspace],
            },
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.teams_allow_codebase_context(),
                AdminEnablementSetting::Enable
            );
            assert!(user_workspaces.is_codebase_context_enabled(ctx));
        });
    })
}

#[test]
fn test_codebase_context_disabled_when_any_team_disables_it() {
    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.codebase_context.value = AdminEnablementSetting::Enable;
    team_b.settings.codebase_context.value = AdminEnablementSetting::Disable;
    let disabled_team_uid = team_b.uid;
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b);

    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            CachedResources {
                workspaces: vec![workspace],
            },
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.teams_allow_codebase_context(),
                AdminEnablementSetting::Disable
            );
            assert_eq!(
                user_workspaces
                    .team_disabling_codebase_context()
                    .map(|team| team.uid),
                Some(disabled_team_uid)
            );
            assert!(!user_workspaces.is_codebase_context_enabled(ctx));
        });
    })
}

#[test]
fn test_codebase_context_respects_user_setting_when_any_team_does() {
    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.codebase_context.value = AdminEnablementSetting::Enable;
    team_b.settings.codebase_context.value = AdminEnablementSetting::RespectUserSetting;
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b);

    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            CachedResources {
                workspaces: vec![workspace],
            },
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.teams_allow_codebase_context(),
                AdminEnablementSetting::RespectUserSetting
            );
            assert!(user_workspaces.is_codebase_context_enabled(ctx));
        });
    })
}

#[test]
fn test_codebase_context_uses_workspace_setting_without_teams() {
    let team = team_for_test();
    let mut workspace = workspace_for_test(&team);
    workspace.teams.clear();
    workspace.settings.codebase_context_settings.setting = AdminEnablementSetting::Disable;

    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            CachedResources {
                workspaces: vec![workspace],
            },
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.teams_allow_codebase_context(),
                AdminEnablementSetting::Disable
            );
            assert!(!user_workspaces.is_codebase_context_enabled(ctx));
        });
    })
}

#[test]
fn test_joining_team_moves_objects() {
    let _flag = FeatureFlag::SharedWithMe.override_enabled(true);

    let team = Team {
        uid: 123.into(),
        name: "test".to_string(),
        color: None,
        invite_link: None,
        members: vec![],
        pending_email_invites: vec![],
        invite_link_domain_restrictions: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        is_eligible_for_discovery: false,
        visibility: TeamVisibility::Open,
    };
    let team_uid = team.uid;
    let workspace = Workspace {
        uid: "workspace_uid123456789".to_string().into(),
        name: "test".to_string(),
        teams: vec![team.clone()],
        open_teams: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        invite_link_domain_restrictions: vec![],
        pending_email_invites: vec![],
        is_eligible_for_discovery: false,
        members: vec![],
    };

    let shared_object = CloudWorkflow::new_local(
        CloudWorkflowModel {
            data: Workflow::new("shared workflow", "echo shared"),
        },
        Owner::Team { team_uid },
        None,
        ClientId::default(),
    );
    let object_id = shared_object.id;

    App::test((), |mut app| async move {
        initialize_app(&mut app, CachedResources { workspaces: vec![] });
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(object_id, shared_object);
        });

        // At first, the object is shared.
        app.read(|ctx| {
            assert!(!UserWorkspaces::as_ref(ctx).has_teams());

            let space = CloudModel::as_ref(ctx)
                .get_by_uid(&object_id.uid())
                .unwrap()
                .space(ctx);
            assert_eq!(space, Space::Shared);
        });

        // Now, the user joins the owning team.
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace], ctx);
        });

        // This migrates the object into the team drive.
        app.read(|ctx: &AppContext| {
            let space = CloudModel::as_ref(ctx)
                .get_by_uid(&object_id.uid())
                .unwrap()
                .space(ctx);
            assert_eq!(space, Space::Team { team_uid });
        });
    })
}

#[test]
fn test_agent_attribution_default_with_no_workspace() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope = user_workspaces.team_context_for_window_for_test(window_id);
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(&scope),
                AdminEnablementSetting::RespectUserSetting,
                "attribution should default to RespectUserSetting when there is no workspace"
            );
        });
    })
}

/// Two windows on teams with opposing attribution policies each see their own team's.
#[test]
fn test_agent_attribution_resolves_each_windows_own_team() {
    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.enable_warp_attribution = AdminEnablementSetting::Enable;
    team_b.settings.enable_warp_attribution = AdminEnablementSetting::Disable;
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, _view_a) = create_test_window(&mut app);
        let (window_b, _view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope_a = user_workspaces.team_context_for_window_for_test(window_a);
            let scope_b = user_workspaces.team_context_for_window_for_test(window_b);
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(&scope_a),
                AdminEnablementSetting::Enable,
                "the window on team A should see team A's forced-on attribution"
            );
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(&scope_b),
                AdminEnablementSetting::Disable,
                "the window on team B should see team B's forced-off attribution"
            );
        });
    })
}

/// Reconciliation can move a window onto a different team, and the policy read has to move
/// with it rather than keep answering for the team the window was on before -- mirrors
/// `member_byo_policy_follows_a_window_reconciled_onto_another_team`.
#[test]
fn test_agent_attribution_follows_a_window_reconciled_onto_another_team() {
    let (mut team_a, mut team_b) = two_teams();
    team_a.settings.enable_warp_attribution = AdminEnablementSetting::Disable;
    team_b.settings.enable_warp_attribution = AdminEnablementSetting::Enable;
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, _view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team_a.uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(
                    &user_workspaces.team_context_for_window_for_test(window_id)
                ),
                AdminEnablementSetting::Disable
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace_for_test(&team_b)], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(
                    &user_workspaces.team_context_for_window_for_test(window_id)
                ),
                AdminEnablementSetting::Enable,
                "the window reconciled onto team B, so its policy applies now"
            );
        });
    })
}

/// Workspace settings are only trustworthy for a user with no teams at all; that user still
/// has to get the policy their server-side tier defaults produced.
#[test]
fn test_agent_attribution_reads_workspace_settings_for_a_teamless_user() {
    let mut workspace = workspace_for_test(&team_for_test());
    workspace.teams.clear();
    workspace.settings.enable_warp_attribution = AdminEnablementSetting::Disable;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, _view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope = user_workspaces.team_context_for_window_for_test(window_id);
            assert_eq!(
                user_workspaces.get_agent_attribution_setting(&scope),
                AdminEnablementSetting::Disable,
                "a genuinely teamless user reads the workspace's own setting"
            );
        });
    })
}

#[test]
fn test_team_switcher_hidden_with_zero_teams() {
    // When the user is in no workspace / no teams, `can_switch_teams` must return
    // false so the pill does not render.
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);
        app.read(|ctx| {
            assert!(
                !UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "0 teams: switcher should be hidden"
            );
        });
    })
}

#[test]
fn test_team_switcher_hidden_with_single_team() {
    // With exactly 1 team, `can_switch_teams` must return false.
    let team = team_for_test();
    let workspace = workspace_for_test(&team);
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        app.read(|ctx| {
            assert!(
                !UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "1 team: switcher should be hidden"
            );
        });
    })
}

#[test]
fn test_team_switcher_visible_with_multiple_teams() {
    // With 2+ teams, `can_switch_teams` must return true so the pill is shown.
    let team1 = team_for_test();
    let mut team2 = team_for_test();
    team2.uid = 456.into();
    team2.name = "Second Team".to_string();
    let mut workspace = workspace_for_test(&team1);
    workspace.teams.push(team2);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        app.read(|ctx| {
            assert!(
                UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "2 teams: switcher should be visible"
            );
        });
    })
}

#[test]
fn test_leaving_team_moves_objects() {
    let _flag = FeatureFlag::SharedWithMe.override_enabled(true);

    let team = Team {
        uid: 123.into(),
        name: "test".to_string(),
        color: None,
        invite_link: None,
        members: vec![],
        pending_email_invites: vec![],
        invite_link_domain_restrictions: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        is_eligible_for_discovery: false,
        visibility: TeamVisibility::Open,
    };
    let team_uid = team.uid;
    let workspace = Workspace {
        uid: "workspace_uid123456789".to_string().into(),
        name: "test".to_string(),
        teams: vec![team.clone()],
        open_teams: vec![],
        settings: Default::default(),
        feature_model_choice: Default::default(),
        invite_link_domain_restrictions: vec![],
        pending_email_invites: vec![],
        is_eligible_for_discovery: false,
        members: vec![],
    };

    let shared_object = CloudWorkflow::new_local(
        CloudWorkflowModel {
            data: Workflow::new("shared workflow", "echo shared"),
        },
        Owner::Team { team_uid },
        None,
        ClientId::default(),
    );
    let object_id = shared_object.id;

    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            CachedResources {
                workspaces: vec![workspace],
            },
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(object_id, shared_object);
        });

        // At first, the object is in the team drive.
        app.read(|ctx| {
            let space = CloudModel::as_ref(ctx)
                .get_by_uid(&object_id.uid())
                .unwrap()
                .space(ctx);
            assert_eq!(space, Space::Team { team_uid });
        });

        // Now, the user leaves the owning team. However, the object is still shared with them.
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![], ctx);
        });

        // This migrates the object into the shared space.
        app.read(|ctx| {
            let space = CloudModel::as_ref(ctx)
                .get_by_uid(&object_id.uid())
                .unwrap()
                .space(ctx);
            assert_eq!(space, Space::Shared);
        });
    })
}

#[test]
fn legacy_cache_migration_yields_a_usable_teamless_catalog() {
    // The older, pre-`ModelsByFeature` cache shape was a bare `AvailableLLMs`, written when
    // all available LLMs were solely for Agent Mode. Migrating it must still produce a
    // catalog whose `agent_mode` bucket resolves the cached model -- a "usable teamless
    // catalog" -- not an empty or default one.
    warpui::App::test((), |mut app| async move {
        app.update(|ctx| {
            ctx.add_singleton_model(|_| {
                PrivatePreferences::new(
                    Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
                )
            });
        });

        let legacy_agent_mode = AvailableLLMs::new(
            "legacy-model".into(),
            vec![LLMInfo::new_for_test("legacy-model")],
            None,
        )
        .expect("legacy AvailableLLMs fixture should be valid");

        app.update(|ctx| {
            ctx.private_user_preferences()
                .write_value(
                    MODELS_BY_FEATURE_CACHE_KEY,
                    serde_json::to_string(&legacy_agent_mode)
                        .expect("legacy fixture should serialize"),
                )
                .expect("legacy cache should be writable");
        });

        app.update(|ctx| {
            let migrated = migrate_legacy_feature_model_choices_cache(ctx)
                .expect("a legacy bare-AvailableLLMs cache should still migrate");
            assert!(
                migrated
                    .agent_mode
                    .info_for_id(&"legacy-model".into())
                    .is_some(),
                "the older bare-AvailableLLMs cache shape should become a usable teamless \
                 agent_mode catalog"
            );
        });
    });
}
