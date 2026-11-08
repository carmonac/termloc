use std::collections::{HashMap, HashSet};

use anyhow::Result;
use warp_core::features::FeatureFlag;
use warp_core::settings::{ChangeEventReason, Setting};
use warp_core::user_preferences::GetUserPreferences;
use warpui::{
    AppContext, Entity, ModelContext, SingletonEntity, Tracked, ViewContext, WeakViewHandle,
    WindowId,
};

use super::team::{DiscoverableTeam, Team};
#[cfg(test)]
use super::team::{MembershipRole, TeamVisibility};
use super::workspace::{AdminEnablementSetting, EnterpriseSecretRegex, Workspace, WorkspaceUid};
use crate::ai::llms::{AvailableLLMs, MODELS_BY_FEATURE_CACHE_KEY, ModelsByFeature};
use crate::auth::AuthStateProvider;
#[cfg(test)]
use crate::auth::UserUid;
use crate::channel::ChannelState;
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{Owner, Space};
use crate::server::ids::ServerId;
use crate::settings::{
    AISettings, AISettingsChangedEvent, CodeSettings, CodeSettingsChangedEvent, PrivacySettings,
};
#[cfg(test)]
use crate::workspaces::workspace::{
    AiAutonomySettings, SplitListSetting, WorkspaceMember, WorkspaceSettings,
};
pub(crate) mod team_workspace_settings;
#[cfg(any(test, feature = "tui"))]
pub use team_workspace_settings::TeamContextForOperation;
#[cfg(test)]
pub(crate) use team_workspace_settings::TeamlessScopeForTest;
pub use team_workspace_settings::{ResolvedTeamScope, TeamContext, TeamContextResolver, TeamScope};

#[derive(Debug)]
pub enum UserWorkspacesEvent {
    FetchDiscoverableTeamsSuccess(Vec<DiscoverableTeam>),
    /// Fired whenever the set of teams the user is on changes.
    TeamsChanged,
    /// Fired when the selected workspace actually changes to a different one.
    CurrentWorkspaceChanged,
    /// Fired when a single window's team assignment changes. Windows are independent, so
    /// subscribers that hold per-window state must only react to their own window.
    WindowTeamChanged {
        window_id: WindowId,
    },
    CodebaseContextEnablementChanged,
}

/// UserWorkspaces is a singleton model that holds workspace metadata (name, members, etc).
/// It should be used for getting information about the workspaces, teams, current teams,
/// and all other things related to operating on workspace and team data.
/// TODO: move other server_api calls to update_manager to correctly update sqlite.
pub struct UserWorkspaces {
    current_workspace_uid: Tracked<Option<WorkspaceUid>>,
    workspaces: Tracked<Vec<Workspace>>,
    window_team_uids: HashMap<WindowId, Option<ServerId>>,
    joinable_teams: Vec<DiscoverableTeam>,
    /// The user-level plan terms from the latest workspaces-metadata response; the fallback for
    /// users whose only workspace is the server's placeholder filtered out of `workspaces`.
    /// The model catalog to fall back to when no current workspace exists: before login, or
    /// for a logged-in user whose only workspace is the server's placeholder, which is
    /// filtered out of `workspaces`.
    workspaceless_models_by_feature: Option<ModelsByFeature>,
}

#[derive(Debug, thiserror::Error)]
pub enum SoleTeamError {
    #[error("you are not on a team")]
    NoTeam,
    #[error("you are on {} teams, so no single team applies", .team_uids.len())]
    MoreThanOneTeam { team_uids: Vec<ServerId> },
}

impl UserWorkspaces {
    #[cfg(any(test, all(feature = "tui", feature = "test-util")))]
    pub fn mock(cached_workspaces: Vec<Workspace>, _ctx: &mut ModelContext<Self>) -> Self {
        Self {
            current_workspace_uid: cached_workspaces.first().map(|w| w.uid).into(),
            workspaces: cached_workspaces.into(),
            window_team_uids: Default::default(),
            joinable_teams: Default::default(),
            workspaceless_models_by_feature: None,
        }
    }

    #[cfg(test)]
    pub fn default_mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::mock(vec![], ctx)
    }

    pub fn new(
        cached_workspaces: Vec<Workspace>,
        current_workspace_uid: Option<WorkspaceUid>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(
            &CodeSettings::handle(ctx),
            |_, _, code_settings_event, ctx| match code_settings_event {
                CodeSettingsChangedEvent::CodebaseContextEnabled { .. }
                | CodeSettingsChangedEvent::AutoIndexingEnabled { .. } => {
                    ctx.emit(UserWorkspacesEvent::CodebaseContextEnablementChanged);
                }
                _ => {}
            },
        );

        ctx.subscribe_to_model(&AISettings::handle(ctx), |_, _, ai_settings_event, ctx| {
            if let AISettingsChangedEvent::IsAnyAIEnabled { .. } = ai_settings_event {
                ctx.emit(UserWorkspacesEvent::CodebaseContextEnablementChanged);
            }
        });

        let mut me = Self {
            current_workspace_uid: current_workspace_uid.into(),
            workspaces: cached_workspaces.into(),
            window_team_uids: Default::default(),
            joinable_teams: Default::default(),
            workspaceless_models_by_feature: None,
        };

        // One-release migration: moving feature_model_choices off of `LLMPreferences` to `Workspace`.
        // This means that on the first time the user opens a version of warp without a
        // Workspace.feature_model_choice saved in their sqlite db, we can fall back to reading feature
        // model choices from the old LLMPreferences cache.
        // TODO: delete once it's safe to assume every client has fetched at least once since
        // this migration shipped.
        if me
            .current_workspace()
            .is_some_and(|workspace| workspace.feature_model_choice == ModelsByFeature::default())
            && let Some(legacy_catalog) = migrate_legacy_feature_model_choices_cache(ctx)
            && let Some(workspace) = me.current_workspace_mut()
        {
            workspace.feature_model_choice = legacy_catalog;
        }

        me
    }

    pub(crate) fn set_workspaceless_models_by_feature(&mut self, models: ModelsByFeature) {
        self.workspaceless_models_by_feature = Some(models);
    }

    pub fn team_from_uid(&self, team_uid: ServerId) -> Option<&Team> {
        self.current_workspace()
            .and_then(|w| w.teams.iter().find(|t| t.uid == team_uid))
    }

    pub fn is_member_of_team(&self, team_uid: ServerId) -> bool {
        self.team_from_uid(team_uid).is_some()
    }

    pub fn register_window(
        &mut self,
        window_id: WindowId,
        team_uid: Option<ServerId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let previous_team_uid = self.team_uid_for_window(window_id);
        self.window_team_uids.entry(window_id).or_insert(team_uid);
        if self.team_uid_for_window(window_id) != previous_team_uid {
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        }
        ctx.notify();
    }

    pub fn inherited_or_default_team_uid(
        &self,
        source_window_id: Option<WindowId>,
    ) -> Option<ServerId> {
        source_window_id
            .and_then(|source_window_id| self.team_uid_for_window(source_window_id))
            .or_else(|| {
                self.current_workspace()
                    .and_then(|workspace| workspace.teams.first())
                    .map(|team| team.uid)
            })
    }

    pub fn set_team_for_window(
        &mut self,
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut ModelContext<Self>,
    ) {
        let window_team_uid = self.window_team_uids.entry(window_id).or_default();
        if window_team_uid.is_none() {
            *window_team_uid = Some(team_uid);
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
            ctx.notify();
        }
    }

    pub fn team_uid_for_window(&self, window_id: WindowId) -> Option<ServerId> {
        self.window_team_uids.get(&window_id).copied().flatten()
    }

    pub fn switch_window_to_team(
        &mut self,
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_window_team_uid(window_id, Some(team_uid), ctx);
    }

    pub(crate) fn set_window_team_uid(
        &mut self,
        window_id: WindowId,
        team_uid: Option<ServerId>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.team_uid_for_window(window_id) == team_uid {
            return;
        }
        self.window_team_uids.insert(window_id, team_uid);
        ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        ctx.notify();
    }

    /// Returns `true` when the user belongs to more than one team in the current
    /// workspace, meaning the team-switcher pill and dropdown should be shown.
    /// Single-team and no-workspace users return `false` so their UI is unchanged.
    pub fn can_switch_teams(&self) -> bool {
        self.current_workspace()
            .map(|ws| ws.teams.len() > 1)
            .unwrap_or(false)
    }
    pub fn team_for_window(&self, window_id: WindowId) -> Option<&Team> {
        self.team_uid_for_window(window_id)
            .and_then(|team_uid| self.team_from_uid(team_uid))
    }
    pub fn team_for_view<T: Entity>(&self, ctx: &ViewContext<T>) -> Option<&Team> {
        self.team_for_window(ctx.window_id())
    }

    pub fn team_for_view_handle<T: Entity>(
        &self,
        view_handle: &WeakViewHandle<T>,
        ctx: &AppContext,
    ) -> Option<&Team> {
        view_handle
            .window_id(ctx)
            .and_then(|window_id| self.team_for_window(window_id))
    }

    /// Returns the windows whose team assignment changed.
    #[must_use]
    fn reconcile_window_team_assignments(&mut self) -> Vec<WindowId> {
        let team_uids = self
            .current_workspace()
            .map(|workspace| {
                workspace
                    .teams
                    .iter()
                    .map(|team| team.uid)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let fallback_team_uid = team_uids.first().copied();

        let mut reassigned_windows = Vec::new();
        for (window_id, window_team_uid) in self.window_team_uids.iter_mut() {
            if window_team_uid.is_none_or(|team_uid| !team_uids.contains(&team_uid))
                && *window_team_uid != fallback_team_uid
            {
                *window_team_uid = fallback_team_uid;
                reassigned_windows.push(*window_id);
            }
        }
        reassigned_windows
    }

    fn emit_window_team_changed(windows: Vec<WindowId>, ctx: &mut ModelContext<Self>) {
        for window_id in windows {
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        }
    }

    pub fn team_from_uid_across_all_workspaces(&self, team_uid: ServerId) -> Option<&Team> {
        self.workspaces
            .iter()
            .flat_map(|w| w.teams.iter())
            .find(|t| t.uid == team_uid)
    }

    /// The teams [`Self::owner_to_space`] recognizes. An owner naming a team outside this set
    /// resolves to the shared space instead of that team's space, so a change here remaps
    /// objects between spaces without any of them changing.
    pub fn team_uids_across_all_workspaces(&self) -> HashSet<ServerId> {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.teams.iter())
            .map(|team| team.uid)
            .collect()
    }

    pub fn workspace_from_uid(&self, workspace_uid: WorkspaceUid) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.uid == workspace_uid)
    }

    pub fn workspace_from_uid_mut(
        &mut self,
        workspace_uid: WorkspaceUid,
    ) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.uid == workspace_uid)
    }

    /// The user's single team in the current workspace.
    ///
    /// Having no workspace is reported as [`SoleTeamError::NoTeam`]: a user with no workspace
    /// is on no team, and no caller can act differently on the distinction.
    pub fn sole_team(&self) -> Result<&Team, SoleTeamError> {
        let teams = self
            .current_workspace()
            .map(|workspace| workspace.teams.as_slice())
            .unwrap_or_default();
        match teams {
            [] => Err(SoleTeamError::NoTeam),
            [team] => Ok(team),
            _ => Err(SoleTeamError::MoreThanOneTeam {
                team_uids: teams.iter().map(|team| team.uid).collect(),
            }),
        }
    }

    pub fn sole_team_uid(&self) -> Result<ServerId, SoleTeamError> {
        self.sole_team().map(|team| team.uid)
    }

    /// Note that the workspace is populated with dummy data until the initial fetch
    /// completes (only workspace name/ID and workspace team's name/ID are cached in
    /// sqlite locally).
    /// Consider whether you need to wait for the results of the fetch before checking the
    /// values of other fields.
    pub fn current_workspace(&self) -> Option<&Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid(workspace_uid))
    }

    pub fn current_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid_mut(workspace_uid))
    }

    pub fn workspaces(&self) -> &Vec<Workspace> {
        &self.workspaces
    }

    pub fn set_current_workspace_uid(
        &mut self,
        workspace_uid: WorkspaceUid,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = *self.current_workspace_uid != Some(workspace_uid);
        *self.current_workspace_uid = Some(workspace_uid);
        let reassigned_windows = self.reconcile_window_team_assignments();
        self.notify_and_emit_teams_changed(ctx);
        Self::emit_window_team_changed(reassigned_windows, ctx);
        if changed {
            ctx.emit(UserWorkspacesEvent::CurrentWorkspaceChanged);
        }
    }

    // Returns a Vec of the user's active spaces, based on their
    // team membership.
    pub fn team_spaces(&self) -> Vec<Space> {
        if let Some(workspace) = self.current_workspace() {
            workspace
                .teams
                .iter()
                .map(|team| Space::Team { team_uid: team.uid })
                .collect()
        } else {
            // If the user has no workspace, they have no team spaces.
            vec![]
        }
    }

    pub fn total_teammates_in_joinable_teams(&self) -> i64 {
        self.joinable_teams
            .iter()
            .map(|team| team.num_members)
            .sum()
    }

    pub fn num_joinable_teams(&self) -> usize {
        self.joinable_teams.len()
    }

    pub fn spaces_for_window(&self, window_id: WindowId, ctx: &AppContext) -> Vec<Space> {
        if AuthStateProvider::as_ref(ctx)
            .get()
            .is_user_web_anonymous_user()
            .unwrap_or_default()
        {
            return vec![Space::Shared];
        }
        let mut spaces = vec![];
        if let Some(team) = self.team_for_window(window_id) {
            spaces.push(Space::Team { team_uid: team.uid });
        }

        if FeatureFlag::SharedWithMe.is_enabled()
            && CloudModel::as_ref(ctx).has_directly_shared_objects(self, ctx)
        {
            spaces.push(Space::Shared);
        }
        spaces.push(Space::Personal);

        spaces
    }

    // Returns the [`Owner`] for the user's personal drive. If the user is not authenticated, this
    // returns `None`.
    pub fn personal_drive(&self, ctx: &AppContext) -> Option<Owner> {
        AuthStateProvider::as_ref(ctx)
            .get()
            .user_id()
            .map(|user_uid| Owner::User { user_uid })
    }

    // Maps a [`Space`] into an [`Owner`], based on the user's team memberships. If the space
    // does not directly identify an owner (it's the space for shared objects), returns `None`.
    pub fn space_to_owner(&self, space: Space, ctx: &AppContext) -> Option<Owner> {
        match space {
            Space::Team { team_uid } => Some(Owner::Team { team_uid }),
            Space::Personal => self.personal_drive(ctx),
            Space::Shared => None,
        }
    }

    // Maps an [`Owner`] into a [`Space`], based on the user's team memberships.
    // This is always possible, as unknown owners imply the shared space.
    pub fn owner_to_space(&self, owner: Owner, ctx: &AppContext) -> Space {
        match owner {
            Owner::User { user_uid } => {
                if !FeatureFlag::SharedWithMe.is_enabled() {
                    return Space::Personal;
                }

                let current_user = AuthStateProvider::as_ref(ctx).get().user_id();
                if Some(user_uid) == current_user {
                    Space::Personal
                } else {
                    Space::Shared
                }
            }
            Owner::Team { team_uid } => {
                if !FeatureFlag::SharedWithMe.is_enabled()
                    || self.team_from_uid_across_all_workspaces(team_uid).is_some()
                {
                    Space::Team { team_uid }
                } else {
                    Space::Shared
                }
            }
        }
    }

    pub fn has_teams(&self) -> bool {
        if let Some(workspace) = self.current_workspace() {
            !workspace.teams.is_empty()
        } else {
            false
        }
    }

    pub fn has_workspaces(&self) -> bool {
        !self.workspaces.is_empty()
    }

    pub fn update_workspaces(&mut self, workspaces: Vec<Workspace>, ctx: &mut ModelContext<Self>) {
        *self.workspaces = workspaces;
        let reassigned_windows = self.reconcile_window_team_assignments();
        self.notify_and_emit_teams_changed(ctx);
        Self::emit_window_team_changed(reassigned_windows, ctx);
    }

    fn notify_and_emit_teams_changed(&self, ctx: &mut ModelContext<Self>) {
        // PrivacySettings can't observe UserWorkspaces for updates, as it's initialized too early in
        // the app initialization flow. So, we update it manually whenever teams data changes.
        PrivacySettings::handle(ctx).update(ctx, |settings, ctx| {
            settings.set_enterprise_secret_redaction_settings(
                self.is_enterprise_secret_redaction_enabled(),
                self.get_enterprise_secret_redaction_regex_list(),
                ChangeEventReason::CloudSync,
                ctx,
            );
        });

        ctx.emit(UserWorkspacesEvent::TeamsChanged);
        ctx.emit(UserWorkspacesEvent::CodebaseContextEnablementChanged);
        ctx.notify();
    }

    pub fn update_joinable_teams(
        &mut self,
        joinable_teams: Vec<DiscoverableTeam>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.joinable_teams.clone_from(&joinable_teams);
        ctx.emit(UserWorkspacesEvent::FetchDiscoverableTeamsSuccess(
            joinable_teams,
        ));
        ctx.notify();
    }

    pub fn is_enterprise_secret_redaction_enabled(&self) -> bool {
        self.current_workspace()
            .map(|workspace| workspace.settings.secret_redaction_settings.enabled)
            .unwrap_or(false)
    }

    pub fn get_enterprise_secret_redaction_regex_list(&self) -> Vec<EnterpriseSecretRegex> {
        self.current_workspace()
            .map(|workspace| workspace.settings.secret_redaction_settings.regexes.clone())
            .unwrap_or_default()
    }

    pub fn get_cloud_conversation_storage_enablement_setting(&self) -> AdminEnablementSetting {
        self.current_workspace()
            .map(|workspace| {
                workspace
                    .settings
                    .cloud_conversation_storage_settings
                    .setting
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Whether invite links are enabled for the current workspace. This is a
    /// workspace-level setting; the teams-settings page reads it from here rather
    /// than from the `Team` struct.
    pub fn is_invite_link_enabled(&self) -> bool {
        self.current_workspace()
            .map(|workspace| workspace.settings.is_invite_link_enabled)
            .unwrap_or(false)
    }

    /// Whether the current workspace's team is discoverable. This is a
    /// workspace-level setting; the teams-settings page reads it from here rather
    /// than from the `Team` struct.
    pub fn is_discoverable(&self) -> bool {
        self.current_workspace()
            .map(|workspace| workspace.settings.is_discoverable)
            .unwrap_or(false)
    }

    /// Returns whether codebase context is enabled across all of the user's teams.
    pub fn is_codebase_context_enabled(&self, app: &AppContext) -> bool {
        if ChannelState::is_local_only()
            && AISettings::as_ref(app)
                .local_embedding_model
                .value()
                .is_empty()
        {
            return false;
        }
        let ai_globally_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        match self.teams_allow_codebase_context() {
            AdminEnablementSetting::Enable => ai_globally_enabled,
            AdminEnablementSetting::Disable => false,
            AdminEnablementSetting::RespectUserSetting => {
                ai_globally_enabled && *CodeSettings::as_ref(app).codebase_context_enabled.value()
            }
        }
    }

    pub fn teams_allow_codebase_context(&self) -> AdminEnablementSetting {
        let mut team_settings = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.teams.iter())
            .map(|team| &team.settings.codebase_context.value)
            .peekable();

        if team_settings.peek().is_none() {
            return self
                .current_workspace()
                .map(|workspace| workspace.settings.codebase_context_settings.setting.clone())
                .unwrap_or_default();
        }

        // TODO(isaiah): Enforce codebase-indexing policy per team and window.
        let mut respects_user_setting = false;
        for setting in team_settings {
            match setting {
                AdminEnablementSetting::Enable => {}
                AdminEnablementSetting::Disable => return AdminEnablementSetting::Disable,
                AdminEnablementSetting::RespectUserSetting => respects_user_setting = true,
            }
        }

        if respects_user_setting {
            AdminEnablementSetting::RespectUserSetting
        } else {
            AdminEnablementSetting::Enable
        }
    }

    pub fn team_disabling_codebase_context(&self) -> Option<&Team> {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.teams.iter())
            .find(|team| team.settings.codebase_context.value == AdminEnablementSetting::Disable)
    }
}

#[cfg(test)]
fn split_test_list(values: Option<Vec<String>>) -> SplitListSetting<String> {
    match values {
        Some(values) => SplitListSetting {
            team_entries: values.clone(),
            values,
            ..Default::default()
        },
        None => Default::default(),
    }
}

#[cfg(test)]
impl UserWorkspaces {
    /// Creates a test workspace with a team and sets it as the current workspace.
    /// Returns the workspace UID and admin UID for use in tests.
    pub fn setup_test_workspace(&mut self, ctx: &mut ModelContext<Self>) {
        let workspace_uid = WorkspaceUid::from(ServerId::from(1));
        let owner_uid = UserUid::new("test_owner");

        let workspace_settings = WorkspaceSettings::default();

        let workspace = Workspace {
            uid: workspace_uid,
            name: "Test Workspace".to_string(),
            teams: vec![Team {
                uid: ServerId::from(2),
                name: "Test Team".to_string(),
                settings: Default::default(),
                color: None,
                members: vec![],
                invite_link: None,
                pending_email_invites: vec![],
                invite_link_domain_restrictions: vec![],
                feature_model_choice: Default::default(),
                is_eligible_for_discovery: false,
                visibility: TeamVisibility::Open,
            }],
            open_teams: vec![],
            members: vec![WorkspaceMember {
                uid: owner_uid,
                email: "test@example.com".to_string(),
                role: MembershipRole::Owner,
                is_disabled: false,
            }],
            settings: workspace_settings,
            feature_model_choice: Default::default(),
            invite_link_domain_restrictions: vec![],
            pending_email_invites: vec![],
            is_eligible_for_discovery: false,
        };

        self.update_workspaces(vec![workspace], ctx);
        self.set_current_workspace_uid(workspace_uid, ctx);
    }

    /// Updates the current workspace by applying a mutation function.
    pub fn update_current_workspace<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Workspace),
    {
        if let Some(workspace) = self.current_workspace() {
            if workspace.teams.is_empty() {
                panic!("No team found in current workspace. Did you call setup_test_workspace()?");
            }

            let mut new_workspace = workspace.clone();
            f(&mut new_workspace);

            self.update_workspaces(vec![new_workspace], ctx);
        } else {
            panic!("No workspace found. Did you call setup_test_workspace()?");
        }
    }

    /// Sets the sandboxed-agent command denylist on [`Self::setup_test_workspace`]'s team, the
    /// team [`Self::sandboxed_agent_execute_commands_denylist_for_scope`] reads for a scope on
    /// it.
    pub fn update_team_sandboxed_agent_denylist<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut SplitListSetting<String>),
    {
        self.update_current_workspace(
            |workspace| {
                f(&mut workspace
                    .teams
                    .first_mut()
                    .expect("test workspace should have a team")
                    .settings
                    .sandboxed_agent
                    .execute_commands_denylist);
            },
            ctx,
        );
    }

    pub fn update_ai_autonomy_settings<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut AiAutonomySettings),
    {
        self.update_current_workspace(
            |workspace| {
                f(&mut workspace.settings.ai_autonomy_settings);
                let settings = &workspace.settings.ai_autonomy_settings;
                let team_settings = &mut workspace
                    .teams
                    .first_mut()
                    .expect("test workspace should have a team")
                    .settings
                    .ai_autonomy;
                team_settings.apply_code_diffs.value = settings.apply_code_diffs_setting;
                team_settings.read_files.value = settings.read_files_setting;
                team_settings.execute_commands.value = settings.execute_commands_setting;
                team_settings.write_to_pty.value = settings.write_to_pty_setting;
                team_settings.computer_use.value = settings.computer_use_setting;
                team_settings.read_files_allowlist =
                    split_test_list(settings.read_files_allowlist.as_ref().map(|items| {
                        items
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect()
                    }));
                team_settings.execute_commands_allowlist = split_test_list(
                    settings
                        .execute_commands_allowlist
                        .as_ref()
                        .map(|items| items.iter().map(ToString::to_string).collect()),
                );
                team_settings.execute_commands_denylist = split_test_list(
                    settings
                        .execute_commands_denylist
                        .as_ref()
                        .map(|items| items.iter().map(ToString::to_string).collect()),
                );
            },
            ctx,
        );
    }
}

/// Reads the legacy, pre-team-keyed model catalog cache (`MODELS_BY_FEATURE_CACHE_KEY`), for
/// the one-release migration in [`UserWorkspaces::new`]. Understands both real shapes an older
/// client could have written: the (more recent) single `ModelsByFeature`, and (older still) a
/// bare `AvailableLLMs`, which becomes the `agent_mode` field.
fn migrate_legacy_feature_model_choices_cache(app: &mut AppContext) -> Option<ModelsByFeature> {
    let value = app
        .private_user_preferences()
        .read_value(MODELS_BY_FEATURE_CACHE_KEY)
        .ok()
        .flatten()?;

    match serde_json::from_str::<ModelsByFeature>(&value) {
        Ok(models) => Some(models),
        Err(e1) => match serde_json::from_str::<AvailableLLMs>(&value) {
            Ok(agent_mode) => Some(ModelsByFeature {
                agent_mode,
                ..Default::default()
            }),
            Err(e2) => {
                log::warn!("Failed to deserialize legacy cached LLMs: {e1}\n{e2}");
                None
            }
        },
    }
}

impl Entity for UserWorkspaces {
    type Event = UserWorkspacesEvent;
}

/// Mark UserWorkspaces as global application state.
impl SingletonEntity for UserWorkspaces {}

#[cfg(test)]
#[path = "user_workspaces_tests.rs"]
mod user_workspaces_tests;
