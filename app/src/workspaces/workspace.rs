use std::cmp::Ordering;
use std::path::PathBuf;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::conversions::{ToAgentModeCommandExecutionPredicates, ToPathBufs};
use super::team::{DiscoverableTeam, MembershipRole, Team};
use crate::ai::execution_profiles::{
    ActionPermission, ComputerUsePermission, WriteToPtyPermission,
};
use crate::ai::llms::{LLMModelHost, LLMProvider, ModelsByFeature};
use crate::auth::UserUid;
use crate::server::ids::ServerId;
use crate::settings::AgentModeCommandExecutionPredicate;

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
pub struct WorkspaceUid(ServerId);
impl From<String> for WorkspaceUid {
    fn from(uid: String) -> Self {
        WorkspaceUid(ServerId::from_string_lossy(uid))
    }
}
impl From<WorkspaceUid> for String {
    fn from(workspace_uid: WorkspaceUid) -> String {
        workspace_uid.0.to_string()
    }
}
impl From<ServerId> for WorkspaceUid {
    fn from(uid: ServerId) -> Self {
        WorkspaceUid(uid)
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub uid: WorkspaceUid,
    pub name: String,
    pub teams: Vec<Team>,
    pub open_teams: Vec<DiscoverableTeam>,
    pub settings: WorkspaceSettings,
    /// The resolved-teamless model catalog -- fallback to this when teams[x].feature_model_choice isn't available
    pub feature_model_choice: ModelsByFeature,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub pending_email_invites: Vec<EmailInvite>,
    // If the team is eligible for discovery, then show toggle for setting discoverability to the team's admin
    pub is_eligible_for_discovery: bool,
    pub members: Vec<WorkspaceMember>,
}

impl Workspace {
    pub fn from_local_cache(
        uid: WorkspaceUid,
        name: String,
        teams: Option<Vec<Team>>,
        feature_model_choice: Option<ModelsByFeature>,
    ) -> Self {
        Self {
            uid,
            name,
            teams: teams.unwrap_or_default(),
            open_teams: Default::default(),
            settings: Default::default(), // TODO: persistence wrapper instead of default
            feature_model_choice: feature_model_choice.unwrap_or_default(),
            invite_link_domain_restrictions: Default::default(),
            pending_email_invites: Default::default(),
            is_eligible_for_discovery: false,
            members: Default::default(),
        }
    }

    fn get_member_by_email(&self, email: &str) -> Option<&WorkspaceMember> {
        self.members.iter().find(|member| member.email == email)
    }

    pub fn is_workspace_admin(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email)
            .is_some_and(|member| member.role.is_admin_or_owner())
    }

    pub fn joinable_teams(&self) -> impl Iterator<Item = &DiscoverableTeam> {
        self.open_teams.iter().filter(|open_team| {
            let open_team_uid = ServerId::from_string_lossy(&open_team.team_uid);
            self.teams.iter().all(|team| team.uid != open_team_uid)
        })
    }

    pub fn can_be_deleted(&self, current_user_email: &str) -> bool {
        // Current user needs to be an admin and be the only user remaining
        self.is_workspace_admin(current_user_email)
            && self.members.len() == 1
            && self
                .members
                .first()
                .is_some_and(|m| m.email == current_user_email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct WorkspaceMember {
    pub uid: UserUid,
    pub email: String,
    pub role: MembershipRole,
    pub is_disabled: bool,
}

impl PartialOrd for WorkspaceMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkspaceMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.email.cmp(&other.email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct EmailInvite {
    pub invitee_email: String,
    pub expired: bool,
    pub team_uid: Option<ServerId>,
}

impl PartialOrd for EmailInvite {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EmailInvite {
    fn cmp(&self, other: &Self) -> Ordering {
        self.invitee_email.cmp(&other.invitee_email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct InviteLinkDomainRestriction {
    pub uid: ServerId,
    pub domain: String,
}

impl PartialOrd for InviteLinkDomainRestriction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InviteLinkDomainRestriction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.domain.cmp(&other.domain)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum HostEnablementSetting {
    Enforce,
    #[default]
    RespectUserSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmHostSettings {
    pub enabled: bool,
    pub enablement_setting: HostEnablementSetting,
    /// Full resource name of the GCP workload identity provider that Gemini Enterprise
    /// (GEAP) credential minting exchanges Warp OIDC JWTs against. Only populated on the
    /// `GeminiEnterprise` host entry; `None` for other hosts and for workspace caches
    /// written before this field existed.
    #[serde(default)]
    pub gcp_audience: Option<String>,
    /// Email of the GCP service account that Gemini Enterprise credential minting
    /// impersonates after the STS exchange. `None` (or empty) means the federated token
    /// is used directly.
    #[serde(default)]
    pub gcp_sa_email: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmSettings {
    pub enabled: bool,
    #[serde(default)]
    pub host_configs: std::collections::HashMap<LLMModelHost, LlmHostSettings>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TelemetrySettings {
    pub force_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum UgcCollectionEnablementSetting {
    Disable,
    Enable,
    #[default]
    RespectUserSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UgcCollectionSettings {
    pub setting: UgcCollectionEnablementSetting,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum AdminEnablementSetting {
    Disable,
    Enable,
    #[default]
    RespectUserSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CloudConversationStorageSettings {
    pub setting: AdminEnablementSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiPermissionsSettings {
    pub allow_ai_in_remote_sessions: bool,
    #[serde(with = "serde_regex")]
    pub remote_session_regex_list: Vec<Regex>,
}

/// The AI autonomy policy an admin has imposed, in the shape the enforcement paths
/// consume: `None` on a field means no admin override, so the user's execution profile
/// decides.
///
/// Both admin layers lower into this one type. The workspace layer stores it directly on
/// [`WorkspaceSettings`]; a team's effective policy arrives as [`TeamAiAutonomySettings`]
/// and converts via its `From` impl. The team shape's extra structure —
/// [`EnforceableSetting`]'s `is_enforced_by_workspace` and [`SplitListSetting`]'s
/// per-layer entries — records which admin layer contributed a value, which is an admin-UI
/// concern rather than part of the policy, so it does not survive the conversion.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiAutonomySettings {
    pub apply_code_diffs_setting: Option<ActionPermission>,
    pub read_files_setting: Option<ActionPermission>,
    pub read_files_allowlist: Option<Vec<PathBuf>>,
    pub execute_commands_setting: Option<ActionPermission>,
    pub execute_commands_allowlist: Option<Vec<AgentModeCommandExecutionPredicate>>,
    pub execute_commands_denylist: Option<Vec<AgentModeCommandExecutionPredicate>>,
    pub write_to_pty_setting: Option<WriteToPtyPermission>,
    pub computer_use_setting: Option<ComputerUsePermission>,
}

impl AiAutonomySettings {
    pub fn has_any_overrides(&self) -> bool {
        self.apply_code_diffs_setting.is_some()
            || self.read_files_setting.is_some()
            || self.read_files_allowlist.is_some()
            || self.execute_commands_setting.is_some()
            || self.execute_commands_allowlist.is_some()
            || self.execute_commands_denylist.is_some()
            || self.write_to_pty_setting.is_some()
            || self.computer_use_setting.is_some()
    }

    pub fn has_override_for_code_diffs(&self) -> bool {
        self.apply_code_diffs_setting.is_some()
    }

    pub fn has_override_for_read_files(&self) -> bool {
        self.read_files_setting.is_some()
    }

    pub fn has_override_for_read_files_allowlist(&self) -> bool {
        self.read_files_allowlist.is_some()
    }

    pub fn has_override_for_execute_commands(&self) -> bool {
        self.execute_commands_setting.is_some()
    }

    pub fn has_override_for_execute_commands_allowlist(&self) -> bool {
        self.execute_commands_allowlist.is_some()
    }

    pub fn has_override_for_execute_commands_denylist(&self) -> bool {
        self.execute_commands_denylist.is_some()
    }

    pub fn has_override_for_write_to_pty(&self) -> bool {
        self.write_to_pty_setting.is_some()
    }

    pub fn has_override_for_computer_use(&self) -> bool {
        self.computer_use_setting.is_some()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkSharingSettings {
    pub anyone_with_link_sharing_enabled: bool,
    pub direct_link_sharing_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnterpriseSecretRegex {
    pub pattern: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecretRedactionSettings {
    pub enabled: bool,
    pub regexes: Vec<EnterpriseSecretRegex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CodebaseContextSettings {
    pub setting: AdminEnablementSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SandboxedAgentSettings {
    pub execute_commands_denylist: Option<Vec<AgentModeCommandExecutionPredicate>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub llm_settings: LlmSettings,
    pub team_byo: Option<TeamByoSettings>,
    pub telemetry_settings: TelemetrySettings,
    pub ugc_collection_settings: UgcCollectionSettings,
    pub cloud_conversation_storage_settings: CloudConversationStorageSettings,
    pub link_sharing_settings: LinkSharingSettings,
    pub secret_redaction_settings: SecretRedactionSettings,
    pub ai_permissions_settings: AiPermissionsSettings,
    pub ai_autonomy_settings: AiAutonomySettings,
    pub is_invite_link_enabled: bool,
    pub is_discoverable: bool,
    pub codebase_context_settings: CodebaseContextSettings,
    pub sandboxed_agent_settings: Option<SandboxedAgentSettings>,
    /// The team-level agent attribution setting. When `Enable` or `Disable`, the
    /// user toggle is locked. When `RespectUserSetting` (or absent), the user can choose.
    #[serde(default)]
    pub enable_warp_attribution: AdminEnablementSetting,
}

/// A workspace-governable setting carried on [`TeamSettings`]: the effective
/// `value` plus whether the workspace layer enforces it (mirrors the server's
/// `*SettingInfo` wrappers). The enforcement bit is preserved so future admin UI
/// can distinguish workspace-enforced values from team-owned ones.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnforceableSetting<T> {
    pub value: T,
    #[serde(default)]
    pub is_enforced_by_workspace: bool,
}

/// A list setting split by the layer that contributed each entry (mirrors the
/// server's `StringListSettingInfo` / `SecretRedactionRegexListInfo`). `values`
/// is the authoritative merged result; `workspace_entries` / `team_entries` are
/// preserved so future admin UI can present the layers separately.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SplitListSetting<T> {
    pub values: Vec<T>,
    #[serde(default)]
    pub workspace_entries: Vec<T>,
    #[serde(default)]
    pub team_entries: Vec<T>,
}

impl<T> SplitListSetting<T> {
    /// Whether an admin layer configured this list.
    ///
    /// Empty `values` does not answer that: the two allowlists merge by intersection
    /// server-side, so layers configuring `["ls"]` and `["git status"]` yield empty `values`
    /// with both entry lists populated. Reading `values` alone would call that unconfigured
    /// and fall back to the user's own profile allowlist, granting exemptions neither admin
    /// granted. The denylists and regex lists merge by union, where empty `values` already
    /// implies empty entries, so the entry checks are inert for them.
    ///
    /// Clearing a list stores it as unset rather than empty -- that is how an admin reverts
    /// to no constraint -- so "configured but empty" does not arise. If that changes,
    /// `StringListSettingInfo.isConfigured` answers this directly, once the vendored schema
    /// copy is re-pulled to include it.
    pub fn is_configured(&self) -> bool {
        !(self.values.is_empty()
            && self.workspace_entries.is_empty()
            && self.team_entries.is_empty())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamAiPermissionsSettings {
    pub allow_ai_in_remote_sessions: EnforceableSetting<bool>,
    #[serde(with = "serde_regex")]
    pub remote_session_regex_list: Vec<Regex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSecretRedactionSettings {
    pub enabled: EnforceableSetting<bool>,
    pub regexes: SplitListSetting<EnterpriseSecretRegex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamAiAutonomySettings {
    pub apply_code_diffs: EnforceableSetting<Option<ActionPermission>>,
    pub read_files: EnforceableSetting<Option<ActionPermission>>,
    pub create_plans: EnforceableSetting<Option<ActionPermission>>,
    pub execute_commands: EnforceableSetting<Option<ActionPermission>>,
    pub write_to_pty: EnforceableSetting<Option<WriteToPtyPermission>>,
    pub computer_use: EnforceableSetting<Option<ComputerUsePermission>>,
    pub read_files_allowlist: SplitListSetting<String>,
    pub execute_commands_allowlist: SplitListSetting<String>,
    pub execute_commands_denylist: SplitListSetting<String>,
}

impl From<&TeamAiAutonomySettings> for AiAutonomySettings {
    /// Lowers a team's effective autonomy policy into the shape enforcement reads.
    ///
    /// A list counts as an override when any admin layer configured it, which is not the
    /// same question as whether the merged result is empty — see
    /// [`SplitListSetting::is_configured`].
    ///
    /// `create_plans` is dropped. It exists only on the team side; `AIExecutionProfile`
    /// carries no create-plans permission for it to override, so there is nothing to
    /// lower it into.
    fn from(team: &TeamAiAutonomySettings) -> Self {
        fn override_list<T>(
            list: &SplitListSetting<String>,
            convert: impl FnOnce(Vec<String>) -> Vec<T>,
        ) -> Option<Vec<T>> {
            if list.is_configured() {
                Some(convert(list.values.clone()))
            } else {
                None
            }
        }

        Self {
            apply_code_diffs_setting: team.apply_code_diffs.value,
            read_files_setting: team.read_files.value,
            read_files_allowlist: override_list(
                &team.read_files_allowlist,
                ToPathBufs::to_path_bufs,
            ),
            execute_commands_setting: team.execute_commands.value,
            execute_commands_allowlist: override_list(
                &team.execute_commands_allowlist,
                ToAgentModeCommandExecutionPredicates::to_predicates,
            ),
            execute_commands_denylist: override_list(
                &team.execute_commands_denylist,
                ToAgentModeCommandExecutionPredicates::to_predicates,
            ),
            write_to_pty_setting: team.write_to_pty.value,
            computer_use_setting: team.computer_use.value,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamLinkSharingSettings {
    pub anyone_with_link_sharing_enabled: EnforceableSetting<bool>,
    pub direct_link_sharing_enabled: EnforceableSetting<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSandboxedAgentSettings {
    pub execute_commands_denylist: SplitListSetting<String>,
}

/// The effective settings that apply to a team, combining the workspace layer
/// with the team's own configuration.
///
/// This is intentionally a distinct type from [`WorkspaceSettings`] rather than
/// an alias: it is sourced from the server's effective `Team.settings`. Each
/// workspace-governable group keeps both its effective value **and** the
/// `is_enforced_by_workspace` / workspace-vs-team split metadata (via
/// [`EnforceableSetting`] / [`SplitListSetting`]) so future admin UI can recover
/// those details. Unlike `WorkspaceSettings`, it does not carry the
/// workspace-scoped `is_invite_link_enabled` / `is_discoverable` flags (those
/// remain on [`WorkspaceSettings`] and are read from the current workspace).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSettings {
    pub ugc_collection: EnforceableSetting<UgcCollectionEnablementSetting>,
    pub cloud_conversation_storage: EnforceableSetting<AdminEnablementSetting>,
    pub codebase_context: EnforceableSetting<AdminEnablementSetting>,
    pub ai_permissions: TeamAiPermissionsSettings,
    pub secret_redaction: TeamSecretRedactionSettings,
    pub ai_autonomy: TeamAiAutonomySettings,
    pub link_sharing: TeamLinkSharingSettings,
    pub sandboxed_agent: TeamSandboxedAgentSettings,
    pub llm_settings: LlmSettings,
    pub telemetry_settings: TelemetrySettings,
    /// The team-level agent attribution setting. When `Enable` or `Disable`, the
    /// user toggle is locked. When `RespectUserSetting` (or absent), the user can choose.
    #[serde(default)]
    pub enable_warp_attribution: AdminEnablementSetting,
    pub team_byo: Option<TeamByoSettings>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamByoSettings {
    pub first_party_enabled: bool,
    pub endpoints_enabled: bool,
    pub allow_user_keys: bool,
    pub allow_user_endpoints: bool,
    pub first_party_keys: Vec<ByoFirstPartyKey>,
    pub endpoints: Vec<ByoEndpointMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByoFirstPartyKey {
    pub provider: LLMProvider,
    pub credential_uid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByoEndpointMetadata {
    pub uid: String,
    pub name: String,
    pub enabled: bool,
    pub credential_uid: String,
    pub models: Vec<ByoEndpointModelMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByoEndpointModelMetadata {
    pub config_key: String,
    pub slug: String,
    pub alias: Option<String>,
    pub display_name: String,
    pub enabled: bool,
}
