//! Team-scoped reads of workspace settings, plus the [`TeamScope`] types that name which team a
//! read is for.
//!

use std::rc::Rc;
use std::sync::OnceLock;

use regex::Regex;
#[cfg(not(target_family = "wasm"))]
use warp_cli::scope::{ObjectScope, TeamSelection};
use warpui::{AppContext, Entity, SingletonEntity, ViewContext, WeakViewHandle, WindowId};

#[cfg(not(target_family = "wasm"))]
use super::SoleTeamError;
use super::UserWorkspaces;
#[cfg(not(target_family = "wasm"))]
use crate::ai::ambient_agents::task::TaskScope;
#[cfg(any(test, feature = "test-util"))]
use crate::ai::llms::LLMInfo;
use crate::ai::llms::ModelsByFeature;
use crate::server::ids::ServerId;
use crate::settings::AgentModeCommandExecutionPredicate;
use crate::workspaces::gql_convert::ToAgentModeCommandExecutionPredicates;
use crate::workspaces::team::Team;
use crate::workspaces::workspace::{AdminEnablementSetting, AiAutonomySettings, Workspace};

mod sealed {
    pub trait Sealed {}
}

/// Reads a [`TeamContextForOperation`] or [`TeamContext`]'s team.
///
/// Application code obtains a [`TeamContext`] or [`TeamContextForOperation`] from a view-bound
/// context, handle, or window. Neither type can be copied or cloned. `TeamContext` is borrow-bound
/// to an immediate read, while `TeamContextForOperation` is owned so one operation can move it
/// across an asynchronous boundary without re-resolving against a different window team.
///
/// Sealed: only this module implements [`sealed::Sealed`], so a scope can never be minted
/// outside [`UserWorkspaces`].
#[allow(private_bounds)]
pub trait TeamScope: sealed::Sealed {
    fn team_uid(&self) -> Option<ServerId>;
}

/// The team selected when a view-scoped operation starts.
pub struct TeamContextForOperation {
    team_uid: Option<ServerId>,
}

impl sealed::Sealed for TeamContextForOperation {}

impl TeamScope for TeamContextForOperation {
    fn team_uid(&self) -> Option<ServerId> {
        self.team_uid
    }
}

#[cfg(test)]
impl TeamContextForOperation {
    pub(crate) fn new_for_test(team_uid: ServerId) -> Self {
        Self {
            team_uid: Some(team_uid),
        }
    }
}

/// The team a view renders as, borrowed for the duration of a single read.
///
/// It is resolved at the point of use so policy reads follow the view between windows.
pub struct TeamContext<'a> {
    team_uid: Option<&'a ServerId>,
}

impl sealed::Sealed for TeamContext<'_> {}

impl TeamScope for TeamContext<'_> {
    fn team_uid(&self) -> Option<ServerId> {
        self.team_uid.copied()
    }
}

/// The team a headless invocation acts as, resolved without a window.
///
/// It has two minting roots. [`UserWorkspaces::team_scope_for_cli`] resolves the command-line
/// selection against the user's memberships and rejects a team they are not on.
/// [`Self::from_task_scope`] takes the server's record of which team owns a task and performs no
/// membership check: a service-account worker resuming a run may belong to none of the task's
/// teams, and the server has already decided the task's ownership.
#[cfg(not(target_family = "wasm"))]
pub enum HeadlessTeamScope {
    Personal,
    Team(ServerId),
}

#[cfg(not(target_family = "wasm"))]
impl HeadlessTeamScope {
    pub(crate) fn from_task_scope(scope: &TaskScope) -> Self {
        if !scope.is_team() {
            return Self::Personal;
        }
        match ServerId::try_from(scope.uid.as_str()) {
            Ok(team_uid) => Self::Team(team_uid),
            Err(err) => {
                log::warn!(
                    "Task reported an invalid team scope uid '{}': {err}",
                    scope.uid
                );
                Self::Personal
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl sealed::Sealed for HeadlessTeamScope {}

#[cfg(not(target_family = "wasm"))]
impl TeamScope for HeadlessTeamScope {
    fn team_uid(&self) -> Option<ServerId> {
        match self {
            HeadlessTeamScope::Personal => None,
            HeadlessTeamScope::Team(team_uid) => Some(*team_uid),
        }
    }
}

pub struct ResolvedTeamScope(Option<ServerId>);

impl ResolvedTeamScope {
    pub fn from_scope(scope: &(impl TeamScope + ?Sized)) -> Self {
        Self(scope.team_uid())
    }

    #[cfg(feature = "agent_mode_evals")]
    pub(crate) fn teamless() -> Self {
        Self(None)
    }
}

impl sealed::Sealed for ResolvedTeamScope {}

impl TeamScope for ResolvedTeamScope {
    fn team_uid(&self) -> Option<ServerId> {
        self.0
    }
}

/// A teamless [`TeamScope`] for tests that pass a scope without standing up a window.
#[cfg(test)]
pub(crate) struct TeamlessScopeForTest;

#[cfg(test)]
impl sealed::Sealed for TeamlessScopeForTest {}

#[cfg(test)]
impl TeamScope for TeamlessScopeForTest {
    fn team_uid(&self) -> Option<ServerId> {
        None
    }
}

/// Resolves a [`TeamContext`] on demand from a view captured up front. See
/// [`UserWorkspaces::team_context_resolver`].
pub type TeamContextResolver = Rc<dyn for<'a> Fn(&'a AppContext) -> TeamContext<'a>>;
pub(crate) type TeamContextForOperationResolver =
    Rc<dyn Fn(&AppContext) -> TeamContextForOperation>;

#[cfg(not(target_family = "wasm"))]
#[derive(Debug, thiserror::Error)]
#[error("you are not on team {team_uid}")]
pub struct NotATeamMemberError {
    pub team_uid: ServerId,
}
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, thiserror::Error)]
pub(crate) enum TeamScopeForCliError {
    #[error("Invalid --team '{team_uid}': {message}")]
    InvalidTeamUid { team_uid: String, message: String },
    #[error(transparent)]
    NoSoleTeam(#[from] SoleTeamError),
    #[error(transparent)]
    NotAMember(#[from] NotATeamMemberError),
}

impl UserWorkspaces {
    /// Captures the team selected in `ctx`'s window as an operation's
    /// [`TeamContextForOperation`]. Always succeeds -- a window with no team selected still yields
    /// a scope whose `team_uid()` is `None`.
    pub fn team_context_for_operation<T: Entity>(
        &self,
        ctx: &ViewContext<T>,
    ) -> TeamContextForOperation {
        self.team_context_for_window_operation(ctx.window_id())
    }
    /// Captures the team selected in a headless frontend's window.
    pub fn team_context_for_window_operation(
        &self,
        window_id: WindowId,
    ) -> TeamContextForOperation {
        TeamContextForOperation {
            team_uid: self.team_uid_for_window(window_id),
        }
    }

    pub(crate) fn team_context<'a, T: Entity>(
        &'a self,
        view: &WeakViewHandle<T>,
        app: &AppContext,
    ) -> TeamContext<'a> {
        let team_uid = self.team_for_view_handle(view, app).map(|team| &team.uid);
        TeamContext { team_uid }
    }

    /// The scope a headless CLI invocation reads team policy through.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn team_scope_for_cli(
        &self,
        team_selection: &TeamSelection,
    ) -> Result<HeadlessTeamScope, TeamScopeForCliError> {
        let team_uid = match &team_selection.team {
            None => match self.sole_team_uid() {
                Ok(team_uid) => Some(team_uid),
                Err(SoleTeamError::NoTeam) => None,
                Err(error @ SoleTeamError::MoreThanOneTeam { .. }) => {
                    return Err(error.into());
                }
            },
            Some(None) => Some(self.sole_team_uid()?),
            Some(Some(team_uid)) => Some(ServerId::try_from(team_uid.as_str()).map_err(|err| {
                TeamScopeForCliError::InvalidTeamUid {
                    team_uid: team_uid.to_string(),
                    message: err.to_string(),
                }
            })?),
        };
        if let Some(team_uid) = team_uid
            && !self.is_member_of_team(team_uid)
        {
            return Err(NotATeamMemberError { team_uid }.into());
        }
        Ok(match team_uid {
            Some(team_uid) => HeadlessTeamScope::Team(team_uid),
            None => HeadlessTeamScope::Personal,
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn team_scope_for_cli_object(
        &self,
        object_scope: &ObjectScope,
    ) -> Result<HeadlessTeamScope, TeamScopeForCliError> {
        if object_scope.personal {
            Ok(HeadlessTeamScope::Personal)
        } else {
            self.team_scope_for_cli(&object_scope.team_selection)
        }
    }

    pub(crate) fn team_context_for_view<T: Entity>(&self, ctx: &ViewContext<T>) -> TeamContext<'_> {
        self.team_context_for_window_id(ctx.window_id())
    }

    /// Captures `view` as a reusable source of [`TeamContext`], for consumers that cannot name
    /// a view at the boundaries where they need one.
    pub fn team_context_resolver<T: Entity>(view: WeakViewHandle<T>) -> TeamContextResolver {
        Rc::new(move |app| Self::as_ref(app).team_context(&view, app))
    }

    pub(crate) fn team_context_for_operation_resolver(
        resolver: TeamContextResolver,
    ) -> TeamContextForOperationResolver {
        Rc::new(move |app| TeamContextForOperation {
            team_uid: resolver(app).team_uid(),
        })
    }

    /// A resolver for tests that build a model without a window to resolve against.
    #[cfg(any(test, feature = "test-util"))]
    pub fn teamless_context_resolver_for_test() -> TeamContextResolver {
        Rc::new(|_| TeamContext { team_uid: None })
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn teamless_context_for_operation_for_test() -> TeamContextForOperation {
        TeamContextForOperation { team_uid: None }
    }

    fn team_context_for_window_id(&self, window_id: WindowId) -> TeamContext<'_> {
        TeamContext {
            team_uid: self
                .team_uid_for_window(window_id)
                .and_then(|team_uid| self.team_from_uid(team_uid))
                .map(|team| &team.uid),
        }
    }

    pub fn team_context_for_window(&self, window_id: WindowId) -> TeamContext<'_> {
        self.team_context_for_window_id(window_id)
    }

    /// [`Self::team_context_for_view`] for tests, which build scopes for bare windows rather
    /// than standing up a view for each one. Production exchanges a view or a [`ViewContext`]
    /// for a scope; this is `#[cfg(test)]` precisely so that contract holds.
    #[cfg(test)]
    pub(crate) fn team_context_for_window_for_test(&self, window_id: WindowId) -> TeamContext<'_> {
        self.team_context_for_window_id(window_id)
    }

    /// The team a scope names, when it names one that is still in the current workspace.
    ///
    /// Deliberately private. Callers get a resolved *setting* from a getter that takes their
    /// scope, never a `&Team` they could carry somewhere the scope never reached. Wanting a
    /// `&Team` at a call site means the read belongs behind a new getter here instead.
    fn team_from_scope<S: TeamScope + ?Sized>(&self, scope: &S) -> Option<&Team> {
        scope
            .team_uid()
            .and_then(|team_uid| self.team_from_uid(team_uid))
    }

    /// Resolves a per-team setting for `scope`: the scope's own team when it names one, otherwise
    /// `current_workspace().settings`.
    ///
    /// A scope naming an unresolvable team yields `absent`, never another team's value. The
    /// no-team branch reads `current_workspace().settings` unconditionally; for a member on teams
    /// that is the server's arbitrarily-elected stand-in (see [`TeamScope`]), a deliberate
    /// simplification because a windowed terminal is never expected to present a teamless scope,
    /// so in practice only a genuinely teamless user reaches it, whose workspace settings the
    /// server computes from tier defaults.
    fn scoped_or_workspace_setting<'a, S: TeamScope + ?Sized, T>(
        &'a self,
        scope: &S,
        from_team: impl FnOnce(&'a Team) -> T,
        from_workspace: impl FnOnce(&'a Workspace) -> T,
        absent: T,
    ) -> T {
        match scope.team_uid() {
            Some(_) => self.team_from_scope(scope).map_or(absent, from_team),
            None => self.current_workspace().map_or(absent, from_workspace),
        }
    }

    /// The self-hosted worker host slug configured as the default for `scope`'s team. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback.
    pub(crate) fn default_host_slug<S: TeamScope + ?Sized>(&self, scope: &S) -> Option<&str> {
        self.scoped_or_workspace_setting(
            scope,
            |team| team.settings.default_host_slug.as_deref(),
            |workspace| workspace.settings.default_host_slug.as_deref(),
            None,
        )
    }

    /// The agent attribution policy for `scope`'s team: `Enable` and `Disable` lock the user's
    /// attribution toggle, `RespectUserSetting` leaves it editable. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback.
    pub(crate) fn get_agent_attribution_setting<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> AdminEnablementSetting {
        self.scoped_or_workspace_setting(
            scope,
            |team| team.settings.enable_warp_attribution.clone(),
            |workspace| workspace.settings.enable_warp_attribution.clone(),
            AdminEnablementSetting::default(),
        )
    }

    pub(crate) fn is_anyone_with_link_sharing_enabled<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> bool {
        self.scoped_or_workspace_setting(
            scope,
            |team| {
                team.settings
                    .link_sharing
                    .anyone_with_link_sharing_enabled
                    .value
            },
            |workspace| {
                workspace
                    .settings
                    .link_sharing_settings
                    .anyone_with_link_sharing_enabled
            },
            true,
        )
    }

    pub(crate) fn is_direct_link_sharing_enabled<S: TeamScope + ?Sized>(&self, scope: &S) -> bool {
        self.scoped_or_workspace_setting(
            scope,
            |team| team.settings.link_sharing.direct_link_sharing_enabled.value,
            |workspace| {
                workspace
                    .settings
                    .link_sharing_settings
                    .direct_link_sharing_enabled
            },
            true,
        )
    }

    /// The AI autonomy policy that applies to `scope`'s team. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback.
    pub(crate) fn ai_autonomy_settings<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> AiAutonomySettings {
        self.scoped_or_workspace_setting(
            scope,
            |team| AiAutonomySettings::from(&team.settings.ai_autonomy),
            |workspace| workspace.settings.ai_autonomy_settings.clone(),
            AiAutonomySettings::default(),
        )
    }

    pub(crate) fn feature_model_choice_for_scope<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> &ModelsByFeature {
        static DEFAULT: OnceLock<ModelsByFeature> = OnceLock::new();
        self.scoped_or_workspace_setting(
            scope,
            |team| &team.feature_model_choice,
            |workspace| &workspace.feature_model_choice,
            self.workspaceless_models_by_feature
                .as_ref()
                .unwrap_or_else(|| DEFAULT.get_or_init(ModelsByFeature::default)),
        )
    }

    pub(crate) fn feature_model_choice_for_team_uid(
        &self,
        team_uid: Option<ServerId>,
    ) -> &ModelsByFeature {
        self.feature_model_choice_for_scope(&TeamContextForOperation { team_uid })
    }

    pub(crate) fn set_feature_model_choice_for_team_uid(
        &mut self,
        team_uid: Option<ServerId>,
        models: ModelsByFeature,
    ) {
        match team_uid {
            Some(team_uid) => {
                if let Some(workspace) = self.current_workspace_mut()
                    && let Some(team) = workspace.teams.iter_mut().find(|t| t.uid == team_uid)
                {
                    team.feature_model_choice = models;
                }
            }
            None => match self.current_workspace_mut() {
                Some(workspace) => workspace.feature_model_choice = models,
                None => self.set_workspaceless_models_by_feature(models),
            },
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn add_agent_mode_model_for_test_for_team_uid(
        &mut self,
        team_uid: Option<ServerId>,
        llm: LLMInfo,
    ) {
        match team_uid {
            Some(team_uid) => {
                if let Some(workspace) = self.current_workspace_mut()
                    && let Some(team) = workspace.teams.iter_mut().find(|t| t.uid == team_uid)
                {
                    team.feature_model_choice
                        .agent_mode
                        .push_choice_for_test(llm);
                }
            }
            None => match self.current_workspace_mut() {
                Some(workspace) => workspace
                    .feature_model_choice
                    .agent_mode
                    .push_choice_for_test(llm),
                None => self
                    .workspaceless_models_by_feature
                    .get_or_insert_with(ModelsByFeature::default)
                    .agent_mode
                    .push_choice_for_test(llm),
            },
        }
    }

    /// The organization-managed command denylist a sandboxed agent must obey, for `scope`'s
    /// team. An empty or unconfigured list is no constraint (a denylist blocks only what it
    /// lists), so both lower to `None`. See [`Self::scoped_or_workspace_setting`] for the
    /// no-team fallback.
    pub(crate) fn sandboxed_agent_execute_commands_denylist_for_scope<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> Option<Vec<AgentModeCommandExecutionPredicate>> {
        self.scoped_or_workspace_setting(
            scope,
            |team| {
                // A denylist blocks only what it lists, so an empty list is no constraint.
                let values = &team
                    .settings
                    .sandboxed_agent
                    .execute_commands_denylist
                    .values;
                (!values.is_empty()).then(|| values.clone().to_predicates())
            },
            |workspace| {
                workspace
                    .settings
                    .sandboxed_agent_settings
                    .clone()
                    .and_then(|settings| settings.execute_commands_denylist)
            },
            None,
        )
    }

    /// Returns true iff AI autonomy features are allowed for `scope`'s team.
    /// TODO: This should be deleted soon. AI autonomy settings have been moved into organization
    /// settings (see `ai_autonomy_settings` above), but there could be an interim time where we
    /// have not set up the org settings yet for an enterprise that previously had the entire
    /// feature set disabled. To capture that case, we'll see if all the settings are `None`;
    /// if so, we'll fall back to their billing metadata's value. Once we've migrated everyone
    /// into org settings, we should remove `is_enabled` from the policy and delete this function.
    pub fn is_ai_autonomy_allowed<S: TeamScope + ?Sized>(&self, scope: &S) -> bool {
        let settings = self.ai_autonomy_settings(scope);
        let all_settings_none = settings.apply_code_diffs_setting.is_none()
            && settings.read_files_setting.is_none()
            && settings.read_files_allowlist.is_none()
            && settings.execute_commands_setting.is_none()
            && settings.execute_commands_allowlist.is_none()
            && settings.execute_commands_denylist.is_none();

        if !all_settings_none {
            return true;
        }

        self.current_workspace().is_none_or(|workspace| {
            workspace
                .billing_metadata
                .tier
                .ai_autonomy_policy
                .is_some_and(|policy| policy.is_enabled)
        })
    }

    /// Whether AI is allowed in remote sessions under `scope`'s team. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback. An unresolvable named
    /// team denies rather than guessing, for a control gating AI in an environment the user
    /// may not control.
    ///
    /// `current_workspace()` is only `None` when logged out or before the first metadata fetch
    /// (an authenticated user, teamless or not, always has at least a personal workspace). There
    /// is no admin policy to read in that state, so it permits, preserving the pre-refactor
    /// default. The helper's single `absent` value cannot express both that and the
    /// unresolvable-team deny, so the no-workspace case is handled explicitly first.
    pub(crate) fn is_ai_allowed_in_remote_sessions<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> bool {
        if scope.team_uid().is_none() && self.current_workspace().is_none() {
            return true;
        }
        self.scoped_or_workspace_setting(
            scope,
            |team| {
                team.settings
                    .ai_permissions
                    .allow_ai_in_remote_sessions
                    .value
            },
            |workspace| {
                workspace
                    .settings
                    .ai_permissions_settings
                    .allow_ai_in_remote_sessions
            },
            false,
        )
    }

    /// The remote-session command patterns configured by `scope`'s team. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback.
    pub(crate) fn get_remote_session_regex_list<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> &[Regex] {
        self.scoped_or_workspace_setting(
            scope,
            |team| {
                team.settings
                    .ai_permissions
                    .remote_session_regex_list
                    .as_slice()
            },
            |workspace| {
                workspace
                    .settings
                    .ai_permissions_settings
                    .remote_session_regex_list
                    .as_slice()
            },
            &[],
        )
    }
}
