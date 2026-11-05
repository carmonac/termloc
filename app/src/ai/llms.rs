use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

pub use ai::{LLMId, LLMProvider};
use parking_lot::FairMutex;
use serde::{Deserialize, Serialize, de};
use warp_core::ui::Icon;
use warp_errors::report_error;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use super::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::server::ids::ServerId;
#[cfg(feature = "agent_mode_evals")]
use crate::workspaces::user_workspaces::ResolvedTeamScope;
use crate::workspaces::user_workspaces::{TeamScope, UserWorkspaces};

/// The leading icon shown next to a model in the model picker and model menus.
pub fn model_leading_icon(llm: &LLMInfo) -> Icon {
    llm.provider.icon().unwrap_or(Icon::Agent)
}

/// Key for cached LLM metadata in user preferences.
///
/// Note: this key used to store a single [`AvailableLLMs`]
/// but was migrated to store a full [`ModelsByFeature`].
pub const MODELS_BY_FEATURE_CACHE_KEY: &str = "AvailableLLMs";
const CLOUD_FALLBACK_OZ_MODEL_ID: &str = "auto";
/// Label for usage rows of custom endpoints recorded in existing conversations.
pub const CUSTOM_ENDPOINT_USAGE_LABEL: &str = "Custom endpoint";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LLMUsageMetadata {
    pub request_multiplier: usize,
    pub credit_multiplier: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisableReason {
    AdminDisabled,
    OutOfRequests,
    ProviderOutage,
    RequiresUpgrade,
    Unavailable,
}

impl DisableReason {
    /// Returns a user-facing tooltip explaining why the model is disabled.
    pub fn tooltip_text(&self) -> &'static str {
        match self {
            DisableReason::AdminDisabled => "This model has been disabled by your team admin.",
            DisableReason::OutOfRequests => "Please upgrade your plan to make more requests.",
            DisableReason::ProviderOutage => {
                "This model is temporarily unavailable due to a provider outage."
            }
            DisableReason::RequiresUpgrade => "Please upgrade your plan to access this model.",
            DisableReason::Unavailable => "This model is unavailable.",
        }
    }

    /// Returns `true` when this disable reason means the user cannot use the model
    /// and we should clear their stored preference.
    ///
    /// `OutOfRequests` and `ProviderOutage` are transient and expected to
    /// resolve without user action, so we preserve the selection.
    fn should_clear_preference(&self) -> bool {
        match self {
            DisableReason::AdminDisabled
            | DisableReason::RequiresUpgrade
            | DisableReason::Unavailable => true,
            DisableReason::OutOfRequests | DisableReason::ProviderOutage => false,
        }
    }
}

/// Returns `true` when the model is usable for the current user: not disabled,
/// or disabled for a reason that doesn't block requests (see
/// [`DisableReason::should_clear_preference`]).
fn is_usable_llm(info: &LLMInfo) -> bool {
    info.disable_reason
        .as_ref()
        .is_none_or(|reason| !reason.should_clear_preference())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LLMSpec {
    pub cost: f32,
    pub quality: f32,
    pub speed: f32,
}

/// The host where an LLM can be routed to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LLMModelHost {
    DirectApi,
    #[serde(other)]
    Unknown,
}

/// Configuration for routing an LLM to a specific host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingHostConfig {
    pub enabled: bool,
    pub model_routing_host: LLMModelHost,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMContextWindow {
    #[serde(default)]
    pub is_configurable: bool,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: u32,
    #[serde(default)]
    pub default_max: u32,
}

/// Metadata about an LLM.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LLMInfo {
    pub display_name: String,
    pub base_model_name: String,
    pub id: LLMId,
    pub reasoning_level: Option<String>,
    pub usage_metadata: LLMUsageMetadata,
    pub description: Option<String>,
    pub disable_reason: Option<DisableReason>,
    pub vision_supported: bool,
    pub spec: Option<LLMSpec>,
    pub provider: LLMProvider,
    pub host_configs: HashMap<LLMModelHost, RoutingHostConfig>,
    pub discount_percentage: Option<f32>,
    pub context_window: LLMContextWindow,
}

impl<'de> Deserialize<'de> for LLMInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        /// Helper type that can deserialize host_configs from either:
        /// - A Vec (wire format from server)
        /// - A HashMap (cached format after commit a8a82421c3)
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HostConfigsWire {
            Vec(Vec<RoutingHostConfig>),
            Map(HashMap<LLMModelHost, RoutingHostConfig>),
        }

        impl Default for HostConfigsWire {
            fn default() -> Self {
                HostConfigsWire::Vec(Vec::new())
            }
        }

        #[derive(Deserialize)]
        struct WireLLMInfo {
            display_name: String,
            #[serde(default)]
            base_model_name: Option<String>,
            id: LLMId,
            #[serde(default)]
            reasoning_level: Option<String>,
            usage_metadata: LLMUsageMetadata,
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            disable_reason: Option<DisableReason>,
            #[serde(default)]
            vision_supported: bool,
            #[serde(default)]
            spec: Option<LLMSpec>,
            provider: LLMProvider,
            #[serde(default)]
            host_configs: HostConfigsWire,
            #[serde(default)]
            discount_percentage: Option<f32>,
            #[serde(default)]
            context_window: LLMContextWindow,
        }

        let wire = WireLLMInfo::deserialize(deserializer)?;
        let host_configs = match wire.host_configs {
            HostConfigsWire::Map(map) => map,
            HostConfigsWire::Vec(vec) => {
                let mut map = HashMap::new();
                for config in vec {
                    let host = config.model_routing_host.clone();
                    if map.insert(host.clone(), config).is_some() {
                        log::warn!(
                            "Duplicate LLMModelHost entry for {:?}, using latest value",
                            host
                        );
                    }
                }
                map
            }
        };
        Ok(Self {
            base_model_name: wire
                .base_model_name
                .unwrap_or_else(|| wire.display_name.clone()),
            vision_supported: wire.vision_supported,
            provider: wire.provider,
            display_name: wire.display_name,
            id: wire.id,
            reasoning_level: wire.reasoning_level,
            usage_metadata: wire.usage_metadata,
            description: wire.description,
            disable_reason: wire.disable_reason,
            spec: wire.spec,
            host_configs,
            discount_percentage: wire.discount_percentage,
            context_window: wire.context_window,
        })
    }
}

/// Deduplicates a list of LLMInfo choices by base_model_name and returns an alphabetically sorted
/// list of display names.
pub fn dedupe_model_display_names<'a>(
    choices: impl IntoIterator<Item = &'a LLMInfo>,
) -> Vec<String> {
    let names: HashSet<String> = choices
        .into_iter()
        .map(|choice| choice.base_model_name.clone())
        .collect();
    let mut sorted: Vec<String> = names.into_iter().collect();
    sorted.sort();
    sorted
}

impl LLMInfo {
    /// Returns the display name for the LLM, to be used in the LLM selector menu.
    pub fn menu_display_name(&self) -> String {
        // Base label includes optional description in parentheses
        match &self.description {
            // This is a temporary implementation that won't scale well for longer
            // descriptions. We should implement a better approach for displaying
            // model descriptions, maybe through subtext.
            Some(desc) => format!("{} ({})", self.display_name, desc),
            None => self.display_name.clone(),
        }
    }

    /// Returns the given model's base name.
    /// For non-reasoning models, this is the same as the display name.
    /// E.g. gpt-5.1 (low reasoning) -> gpt-5.1
    pub fn base_model_name(&self) -> &str {
        &self.base_model_name
    }

    /// Returns true if this model has a reasoning level configured.
    pub fn has_reasoning_level(&self) -> bool {
        self.reasoning_level.is_some()
    }

    /// Returns the reasoning level label formatted for display.
    pub fn reasoning_level(&self) -> Option<String> {
        self.reasoning_level.clone()
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub(crate) fn new_for_test(llm_name: &str) -> Self {
        Self {
            display_name: llm_name.to_string(),
            base_model_name: llm_name.to_string(),
            id: llm_name.into(),
            reasoning_level: None,
            usage_metadata: LLMUsageMetadata {
                request_multiplier: 1,
                credit_multiplier: None,
            },
            description: None,
            disable_reason: None,
            vision_supported: false, // Default to false for tests
            spec: None,
            provider: LLMProvider::Unknown,
            host_configs: HashMap::new(),
            discount_percentage: None,
            context_window: LLMContextWindow::default(),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn local_llm_info(model: &local_agent::LocalModel) -> LLMInfo {
    LLMInfo {
        display_name: model.name.clone(),
        base_model_name: model.name.clone(),
        id: model.name.clone().into(),
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 1,
            credit_multiplier: None,
        },
        description: None,
        disable_reason: None,
        vision_supported: model.supports_vision,
        spec: None,
        provider: LLMProvider::Ollama,
        host_configs: HashMap::new(),
        discount_percentage: None,
        context_window: LLMContextWindow::default(),
    }
}

/// The set of LLMs available for a feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvailableLLMs {
    /// The Warp "default" LLM.
    default_id: LLMId,
    choices: Vec<LLMInfo>,

    #[serde(default)]
    preferred_codex_model_id: Option<LLMId>,
}

impl AvailableLLMs {
    /// Constructs an `AvailableLLMs` instance from the given default ID and choices.
    ///
    /// If choices is empty, returns an error.
    ///
    /// If default_id is not a valid ID present in `choices`, takes the first choice in `choices
    /// and uses it as the default.
    pub fn new<T: Into<LLMInfo>>(
        mut default_id: LLMId,
        choices: impl IntoIterator<Item = T>,
        preferred_codex_model_id: Option<LLMId>,
    ) -> Result<Self, anyhow::Error> {
        let choices: Vec<LLMInfo> = choices.into_iter().map(Into::into).collect();
        if choices.is_empty() {
            return Err(anyhow::anyhow!(
                "Tried to create AvailableLLMs with empty`choices`.",
            ));
        } else if !choices.iter().any(|info| info.id == default_id) {
            let fallback_default = choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("Choices should not be empty"))?;
            report_error!(
                "Default LLM ID not present in choices, falling back to first choice",
                extra: {
                    "default_id" => %default_id,
                    "fallback_choice" => %fallback_default.display_name
                }
            );
            default_id = fallback_default.id.clone();
        }

        Ok(Self {
            default_id,
            choices: choices.into_iter().collect(),
            preferred_codex_model_id,
        })
    }

    pub(crate) fn info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.choices.iter().find(|info| info.id == *id)
    }

    /// Returns the info for the given id only if the model is usable (present
    /// and not effectively disabled for the current user).
    fn usable_info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.info_for_id(id).filter(|info| is_usable_llm(info))
    }

    /// Disable-aware default: the server default when usable, otherwise the
    /// first usable choice. `None` when no server-provided choice is usable
    /// (e.g. an admin disabled every hosted model).
    fn usable_default_llm_info(&self) -> Option<&LLMInfo> {
        self.usable_info_for_id(&self.default_id)
            .or_else(|| self.choices.iter().find(|info| is_usable_llm(info)))
    }

    fn default_llm_info(&self) -> &LLMInfo {
        if let Some(info) = self.info_for_id(&self.default_id) {
            return info;
        }

        // `new()` enforces that `default_id` is one of `choices`, but
        // deserialization bypasses `new()`, so a stale persisted cache or a
        // server payload can produce an `AvailableLLMs` whose `default_id` is
        // absent from `choices`. Rather than panic, mirror `new()` and fall
        // back to the first choice.
        let fallback = self
            .choices
            .first()
            .expect("AvailableLLMs must have at least one choice");
        report_error!(
            "Default LLM ID not present in choices, falling back to first choice",
            extra: {
                "default_id" => %self.default_id,
                "fallback_choice" => %fallback.display_name
            }
        );
        fallback
    }

    #[cfg(feature = "integration_tests")]
    pub fn new_for_test(llm_name: &str) -> Self {
        Self {
            default_id: llm_name.into(),
            choices: vec![LLMInfo::new_for_test(llm_name)],
            preferred_codex_model_id: None,
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn push_choice_for_test(&mut self, llm: LLMInfo) {
        self.choices.push(llm);
    }
}

/// The set of models available to the client, grouped by the feature they support.
/// This is fetched from the server and cached.
///
/// Currently, if a model is available for multiple features,
/// it will appear denormalized in each of the feature's
/// [`AvailableLLMs`]. While this denormalization doesn't add much value today,
/// it eventually lets us add feature-specific properties to an [`LLMInfo`].
///
/// NOTE: This used to include a `planning` field; this was removed after planning via subagent was
/// deprecated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelsByFeature {
    pub agent_mode: AvailableLLMs,
    pub coding: AvailableLLMs,
    /// The set of LLMs available for CLI agent.
    /// This field is optional during deserialization, as older clients might not have this field.
    #[serde(default)]
    pub cli_agent: Option<AvailableLLMs>,
    /// The set of LLMs available for computer use agent.
    /// This field is optional during deserialization, as older clients might not have this field.
    #[serde(default)]
    pub computer_use: Option<AvailableLLMs>,
}

impl ModelsByFeature {
    /// Returns the info about the LLM identified by `id`, if we have it.
    ///
    /// For models that are available across multiple features,
    /// any one of the metadata will be returned.
    pub(crate) fn info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.agent_mode.info_for_id(id)
    }
}

/// Returns the default AvailableLLMs for computer use.
/// Used both in `ModelsByFeature::default()` and as a fallback in `get_computer_use_available()`.
fn default_computer_use_llms() -> AvailableLLMs {
    AvailableLLMs {
        default_id: "computer-use-agent-auto".to_owned().into(),
        choices: vec![LLMInfo {
            display_name: "auto".to_owned(),
            base_model_name: "auto".to_owned(),
            id: "computer-use-agent-auto".to_owned().into(),
            reasoning_level: None,
            usage_metadata: LLMUsageMetadata {
                request_multiplier: 1,
                credit_multiplier: None,
            },
            description: None,
            disable_reason: None,
            vision_supported: true,
            spec: None,
            provider: LLMProvider::Unknown,
            host_configs: HashMap::new(),
            discount_percentage: None,
            context_window: LLMContextWindow::default(),
        }],
        preferred_codex_model_id: None,
    }
}

impl Default for ModelsByFeature {
    fn default() -> Self {
        Self {
            agent_mode: AvailableLLMs {
                default_id: "auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto (cost-efficient)".to_owned(),
                    base_model_name: "auto (cost-efficient)".to_owned(),
                    id: "auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: true,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    discount_percentage: None,
                    context_window: LLMContextWindow::default(),
                }],
                preferred_codex_model_id: None,
            },
            coding: AvailableLLMs {
                default_id: "auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto (responsive)".to_owned(),
                    base_model_name: "auto (responsive)".to_owned(),
                    id: "auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: true,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    discount_percentage: None,
                    context_window: LLMContextWindow::default(),
                }],
                preferred_codex_model_id: None,
            },
            cli_agent: Some(AvailableLLMs {
                default_id: "cli-agent-auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto".to_owned(),
                    base_model_name: "auto".to_owned(),
                    id: "cli-agent-auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: false,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    discount_percentage: None,
                    context_window: LLMContextWindow::default(),
                }],
                preferred_codex_model_id: None,
            }),
            computer_use: Some(default_computer_use_llms()),
        }
    }
}

enum UpdatePopupVisibilityState {
    WaitingToBeShown,
    Visible(EntityId),
    Hidden,
}

struct AvailableLLMsUpdate {
    new_choices: Vec<LLMInfo>,
    popup_visibility_state: Arc<FairMutex<UpdatePopupVisibilityState>>,
}

/// Singleton model holding user/workspace LLM preferences, including the set of LLMs available for
/// use as well as the user's preferred LLM for Agent Mode.
pub struct LLMPreferences {
    /// Whether the most recent authed agent-mode model-list fetch failed.
    agent_mode_models_unavailable: HashMap<Option<ServerId>, bool>,
    last_update: Option<AvailableLLMsUpdate>,
    // Stores model overrides for a given terminal view. User selections are
    // normalized against the GUI profile default, while explicit child-run
    // selections remain pinned even when they currently equal the fallback.
    base_llm_for_terminal_view: HashMap<EntityId, LLMId>,
    computer_use_llm_for_terminal_view: HashMap<EntityId, Option<LLMId>>,
}

impl LLMPreferences {
    pub fn new(
        #[cfg_attr(not(feature = "agent_mode_evals"), allow(unused_variables))]
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        #[cfg_attr(not(feature = "agent_mode_evals"), allow(unused_mut))]
        let mut me = Self {
            agent_mode_models_unavailable: HashMap::new(),
            last_update: None,
            base_llm_for_terminal_view: HashMap::new(),
            computer_use_llm_for_terminal_view: HashMap::new(),
        };

        // In agent mode eval builds, eagerly kick off a fetch of the model list from the server
        // so that it's available by the time test steps like `set_preferred_agent_mode_llm` run.
        // In production, this is handled reactively (on auth complete, network online, etc.)
        // to avoid duplicate requests at startup.
        #[cfg(feature = "agent_mode_evals")]
        me.refresh_available_models(&ResolvedTeamScope::teamless(), ctx);

        me
    }

    /// Returns the `LLMInfo` for the base LLM to be used for an Agent Mode request.
    pub fn get_active_base_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_preferred_base_model(scope, app, terminal_view_id)
    }

    pub fn get_active_base_model_for_team_uid<'a>(
        &'a self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let models_by_feature =
            UserWorkspaces::as_ref(app).feature_model_choice_for_team_uid(team_uid);
        if let Some(terminal_view_id) = terminal_view_id {
            let raw_override = self.base_llm_for_terminal_view.get(&terminal_view_id);
            if let Some(llm_id) = raw_override
                && let Some(llm_info) = models_by_feature.agent_mode.usable_info_for_id(llm_id)
            {
                return llm_info;
            }
        }
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);
        profile
            .data()
            .base_model
            .clone()
            .and_then(|id| models_by_feature.agent_mode.usable_info_for_id(&id))
            .unwrap_or_else(|| Self::fallback_llm_info(&models_by_feature.agent_mode))
    }

    /// Returns `LLMInfo` for the currently selected LLM to be used for Agent Mode.
    fn get_preferred_base_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let models_by_feature = UserWorkspaces::as_ref(app).feature_model_choice_for_scope(scope);
        if let Some(terminal_view_id) = terminal_view_id {
            let raw_override = self.base_llm_for_terminal_view.get(&terminal_view_id);
            if let Some(llm_id) = raw_override
                && let Some(llm_info) = models_by_feature.agent_mode.usable_info_for_id(llm_id)
            {
                return llm_info;
            }
        }

        self.get_active_profile_base_model(scope, app, terminal_view_id)
    }

    /// Returns the active execution profile's effective base model without applying a
    /// terminal-view override.
    pub fn get_active_profile_base_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_active_profile_base_model_for_team_uid(scope.team_uid(), app, terminal_view_id)
    }

    pub fn get_active_profile_base_model_for_team_uid<'a>(
        &'a self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);
        let models_by_feature =
            UserWorkspaces::as_ref(app).feature_model_choice_for_team_uid(team_uid);

        profile
            .data()
            .base_model
            .clone()
            .and_then(|id| models_by_feature.agent_mode.usable_info_for_id(&id))
            .unwrap_or_else(|| Self::fallback_llm_info(&models_by_feature.agent_mode))
    }

    /// Disable-aware fallback for when the user has no explicit (usable)
    /// selection: the feature default when usable, else the first usable
    /// choice, else the (possibly disabled) default as a last resort.
    fn fallback_llm_info(available: &AvailableLLMs) -> &LLMInfo {
        available
            .usable_default_llm_info()
            .unwrap_or_else(|| available.default_llm_info())
    }

    pub fn get_active_coding_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_preferred_coding_model(scope, app, terminal_view_id)
    }

    /// Returns `LLMInfo` for user's preferred coding model.
    fn get_preferred_coding_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);
        let models_by_feature = UserWorkspaces::as_ref(app).feature_model_choice_for_scope(scope);

        profile
            .data()
            .coding_model
            .clone()
            .and_then(|id| models_by_feature.coding.info_for_id(&id))
            .unwrap_or_else(|| Self::fallback_llm_info(&models_by_feature.coding))
    }

    /// Returns the set of LLMs available for Agent Mode use.
    pub fn get_base_llm_choices_for_agent_mode<'a, S: TeamScope + ?Sized>(
        &'a self,
        scope: &S,
        app: &'a AppContext,
    ) -> impl Iterator<Item = &'a LLMInfo> + use<'a, S> {
        self.get_base_llm_choices_for_agent_mode_for_team_uid(scope.team_uid(), app)
    }

    pub fn get_base_llm_choices_for_agent_mode_for_team_uid<'a>(
        &'a self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
    ) -> impl Iterator<Item = &'a LLMInfo> + use<'a> {
        UserWorkspaces::as_ref(app)
            .feature_model_choice_for_team_uid(team_uid)
            .agent_mode
            .choices
            .iter()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn add_agent_mode_model_for_test(
        &mut self,
        scope: &(impl TeamScope + ?Sized),
        llm: LLMInfo,
        ctx: &mut ModelContext<Self>,
    ) {
        let team_uid = scope.team_uid();
        UserWorkspaces::handle(ctx).update(ctx, |workspaces, _| {
            workspaces.add_agent_mode_model_for_test_for_team_uid(team_uid, llm);
        });
    }

    /// Returns the set of LLMs available for coding.
    pub fn get_coding_llm_choices<'a, S: TeamScope + ?Sized>(
        &'a self,
        scope: &S,
        app: &'a AppContext,
    ) -> impl Iterator<Item = &'a LLMInfo> + use<'a, S> {
        // Don't show admin-disabled models in the dropdown
        UserWorkspaces::as_ref(app)
            .feature_model_choice_for_scope(scope)
            .coding
            .choices
            .iter()
            .filter(|llm| !matches!(llm.disable_reason, Some(DisableReason::AdminDisabled)))
    }

    /// Returns the set of LLMs available for CLI agent.
    pub fn get_cli_agent_llm_choices<'a, S: TeamScope + ?Sized>(
        &'a self,
        scope: &S,
        app: &'a AppContext,
    ) -> impl Iterator<Item = &'a LLMInfo> + use<'a, S> {
        // Don't show admin-disabled models in the dropdown
        self.get_cli_agent_available(scope.team_uid(), app)
            .choices
            .iter()
            .filter(|llm| !matches!(llm.disable_reason, Some(DisableReason::AdminDisabled)))
    }

    /// Returns the `LLMInfo` for the CLI agent model.
    pub fn get_active_cli_agent_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_active_cli_agent_model_for_team_uid(scope.team_uid(), app, terminal_view_id)
    }

    pub fn get_active_cli_agent_model_for_team_uid<'a>(
        &'a self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        let available = self.get_cli_agent_available(team_uid, app);
        profile
            .data()
            .cli_agent_model
            .clone()
            .and_then(|id| available.info_for_id(&id))
            .unwrap_or_else(|| Self::fallback_llm_info(available))
    }

    /// Returns the effective default CLI agent model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_cli_agent_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> &'a LLMInfo {
        Self::fallback_llm_info(self.get_cli_agent_available(scope.team_uid(), app))
    }

    /// Helper to get the AvailableLLMs for cli_agent, falling back to agent_mode.
    fn get_cli_agent_available<'a>(
        &self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
    ) -> &'a AvailableLLMs {
        let models_by_feature =
            UserWorkspaces::as_ref(app).feature_model_choice_for_team_uid(team_uid);
        models_by_feature
            .cli_agent
            .as_ref()
            .unwrap_or(&models_by_feature.agent_mode)
    }

    /// Returns the set of LLMs available for computer use agent.
    pub fn get_computer_use_llm_choices<'a>(
        &self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> impl Iterator<Item = &'a LLMInfo> {
        self.get_computer_use_available(scope.team_uid(), app)
            .choices
            .iter()
    }

    /// Returns the `LLMInfo` for the computer use agent model.
    pub fn get_active_computer_use_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        let available = self.get_computer_use_available(scope.team_uid(), app);
        let selected_model = terminal_view_id
            .and_then(|id| self.computer_use_llm_for_terminal_view.get(&id))
            .map(Option::as_ref)
            .unwrap_or_else(|| profile.data().computer_use_model.as_ref());
        selected_model
            .and_then(|id| available.info_for_id(id))
            .unwrap_or_else(|| self.get_default_computer_use_model(scope, app))
    }

    /// Returns the effective default computer use model as a fallback: the
    /// server default when usable, else the first usable choice, else the
    /// (possibly disabled) server default.
    pub fn get_default_computer_use_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> &'a LLMInfo {
        let available = self.get_computer_use_available(scope.team_uid(), app);
        available
            .usable_default_llm_info()
            .unwrap_or_else(|| available.default_llm_info())
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn set_computer_use_llm_override(
        &mut self,
        terminal_view_id: EntityId,
        model_id: Option<LLMId>,
    ) {
        self.computer_use_llm_for_terminal_view
            .insert(terminal_view_id, model_id);
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn clear_computer_use_llm_override(&mut self, terminal_view_id: EntityId) {
        self.computer_use_llm_for_terminal_view
            .remove(&terminal_view_id);
    }

    /// Helper to get the AvailableLLMs for computer_use.
    /// Falls back to a computer-use-specific default if None.
    fn get_computer_use_available<'a>(
        &self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
    ) -> &'a AvailableLLMs {
        static DEFAULT: OnceLock<AvailableLLMs> = OnceLock::new();
        UserWorkspaces::as_ref(app)
            .feature_model_choice_for_team_uid(team_uid)
            .computer_use
            .as_ref()
            .unwrap_or_else(|| DEFAULT.get_or_init(default_computer_use_llms))
    }

    /// Returns metadata about an LLM, if the client knows about it.
    pub fn get_llm_info<'a>(&'a self, id: &LLMId, app: &'a AppContext) -> Option<&'a LLMInfo> {
        let workspaces = UserWorkspaces::as_ref(app);
        workspaces
            .current_workspace()
            .map(|workspace| workspace.teams.iter())
            .into_iter()
            .flatten()
            .find_map(|team| team.feature_model_choice.info_for_id(id))
            .or_else(|| {
                workspaces
                    .feature_model_choice_for_team_uid(None)
                    .info_for_id(id)
            })
    }

    /// Returns `true` when `id` can run in a Warp cloud (Oz) agent, and is therefore safe to
    /// forward as a cloud `config.model_id`. Ollama models live on this machine, so a cloud
    /// worker cannot reach them.
    pub fn is_cloud_runnable_oz_model_id(&self, id: &LLMId, app: &AppContext) -> bool {
        self.get_llm_info(id, app)
            .is_none_or(|info| info.provider != LLMProvider::Ollama)
    }

    /// Returns a cloud-runnable Oz model id, falling back to server-side
    /// automatic model selection when the requested model is local-only.
    pub(crate) fn cloud_runnable_oz_model_id_or_fallback(
        &self,
        id: &LLMId,
        app: &AppContext,
    ) -> String {
        if self.is_cloud_runnable_oz_model_id(id, app) {
            id.to_string()
        } else {
            CLOUD_FALLBACK_OZ_MODEL_ID.to_owned()
        }
    }

    /// True when the pane's active Agent Mode model can run in a Warp cloud
    /// (Oz) agent (see [`Self::is_cloud_runnable_oz_model_id`]).
    pub(crate) fn is_active_base_model_cloud_runnable(
        &self,
        scope: &(impl TeamScope + ?Sized),
        terminal_view_id: EntityId,
        app: &AppContext,
    ) -> bool {
        self.is_cloud_runnable_oz_model_id(
            &self
                .get_active_base_model(scope, app, Some(terminal_view_id))
                .id,
            app,
        )
    }

    /// Updates the active execution profile's default Agent Mode model.
    #[cfg(test)]
    pub fn update_active_profile_base_model(
        &self,
        preferred_llm_id: &LLMId,
        terminal_view_id: Option<EntityId>,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let profiles = AIExecutionProfilesModel::handle(ctx);
        let profile_id = profiles
            .as_ref(ctx)
            .active_profile(terminal_view_id, ctx)
            .id()
            .clone();
        let (persisted, changed) = profiles.update(ctx, |profiles, ctx| {
            let profile = profiles
                .get_profile_by_id(&profile_id, ctx)
                .expect("active execution profile should exist");
            if profile.data().base_model.as_ref() == Some(preferred_llm_id) {
                return (true, false);
            }
            profiles.set_base_model(&profile_id, Some(preferred_llm_id.clone()), ctx);
            profiles.set_context_window_limit(&profile_id, None, ctx);
            let persisted = profiles
                .get_profile_by_id(&profile_id, ctx)
                .is_some_and(|profile| {
                    profile.data().base_model.as_ref() == Some(preferred_llm_id)
                        && profile.data().context_window_limit.is_none()
                });
            (persisted, persisted)
        });
        if changed {
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
        persisted
    }

    /// Returns the effective default base model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_base_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> &'a LLMInfo {
        Self::fallback_llm_info(
            &UserWorkspaces::as_ref(app)
                .feature_model_choice_for_scope(scope)
                .agent_mode,
        )
    }

    pub fn get_default_base_model_for_team_uid<'a>(
        &'a self,
        team_uid: Option<ServerId>,
        app: &'a AppContext,
    ) -> &'a LLMInfo {
        Self::fallback_llm_info(
            &UserWorkspaces::as_ref(app)
                .feature_model_choice_for_team_uid(team_uid)
                .agent_mode,
        )
    }

    /// Returns the effective default coding model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_coding_model<'a>(
        &'a self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> &'a LLMInfo {
        Self::fallback_llm_info(
            &UserWorkspaces::as_ref(app)
                .feature_model_choice_for_scope(scope)
                .coding,
        )
    }

    /// Returns the preferred Codex model, if set by the server.
    pub fn get_preferred_codex_model<'a>(
        &self,
        scope: &(impl TeamScope + ?Sized),
        app: &'a AppContext,
    ) -> Option<&'a LLMInfo> {
        let agent_mode = &UserWorkspaces::as_ref(app)
            .feature_model_choice_for_scope(scope)
            .agent_mode;
        agent_mode
            .preferred_codex_model_id
            .as_ref()
            .and_then(|id| agent_mode.info_for_id(id))
    }

    /// Returns `true` when the most recent authed agent-mode model-list fetch
    /// failed, so the server-provided model list is currently unavailable.
    pub fn agent_mode_models_unavailable(&self, scope: &(impl TeamScope + ?Sized)) -> bool {
        self.agent_mode_models_unavailable_for_team_uid(scope.team_uid())
    }

    pub fn agent_mode_models_unavailable_for_team_uid(&self, team_uid: Option<ServerId>) -> bool {
        self.agent_mode_models_unavailable
            .get(&team_uid)
            .copied()
            .unwrap_or(false)
    }

    /// Sets whether the authed agent-mode model list is currently unavailable.
    /// Called from the authed fetch path on failure, from
    /// [`Self::on_server_update`] on any successful model-list update, and
    /// from tests.
    pub(crate) fn set_agent_mode_models_unavailable_for_team_uid(
        &mut self,
        team_uid: Option<ServerId>,
        unavailable: bool,
    ) {
        self.agent_mode_models_unavailable
            .insert(team_uid, unavailable);
    }

    #[cfg(feature = "integration_tests")]
    pub fn is_available_agent_mode_llm(
        &self,
        scope: &(impl TeamScope + ?Sized),
        id: &LLMId,
        app: &AppContext,
    ) -> bool {
        UserWorkspaces::as_ref(app)
            .feature_model_choice_for_scope(scope)
            .agent_mode
            .info_for_id(id)
            .is_some()
    }

    /// Creates a pane-level override for the Agent Mode LLM.
    pub fn update_preferred_agent_mode_llm(
        &mut self,
        scope: &(impl TeamScope + ?Sized),
        preferred_llm_id: &LLMId,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_preferred_agent_mode_llm_for_team_uid(
            scope.team_uid(),
            preferred_llm_id,
            terminal_view_id,
            ctx,
        )
    }

    pub fn update_preferred_agent_mode_llm_for_team_uid(
        &mut self,
        team_uid: Option<ServerId>,
        preferred_llm_id: &LLMId,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let profile_default_model_id = self
            .get_active_profile_base_model_for_team_uid(team_uid, ctx, Some(terminal_view_id))
            .id
            .clone();

        // Only remove override if we're setting to the profile's default.
        // Otherwise, always set the override explicitly.
        let changed = if preferred_llm_id == &profile_default_model_id {
            self.base_llm_for_terminal_view
                .remove(&terminal_view_id)
                .is_some()
        } else {
            self.base_llm_for_terminal_view
                .insert(terminal_view_id, preferred_llm_id.clone())
                != Some(preferred_llm_id.clone())
        };

        if changed {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    /// Pins an explicit child-run model independently of profile or TUI
    /// defaults. Persist the pin whenever it changes, but notify active-model
    /// subscribers only when the surface's effective selection changes.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn set_agent_mode_llm_override(
        &mut self,
        scope: &(impl TeamScope + ?Sized),
        terminal_view_id: EntityId,
        model_id: LLMId,
        ctx: &mut ModelContext<Self>,
    ) {
        let previous_effective_model_id = self
            .get_active_base_model(scope, ctx, Some(terminal_view_id))
            .id
            .clone();
        let stored_selection_changed = self
            .base_llm_for_terminal_view
            .insert(terminal_view_id, model_id.clone())
            != Some(model_id);
        if stored_selection_changed {
            self.trigger_snapshot_save(ctx);
            if self
                .get_active_base_model(scope, ctx, Some(terminal_view_id))
                .id
                != previous_effective_model_id
            {
                ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
            }
        }
    }

    /// Copies the raw per-pane Agent Mode override from `source_terminal_view_id`
    /// onto `new_terminal_view_id`, removing any existing override when the
    /// source has none. Combined with copying the source's execution profile,
    /// this reproduces the source pane's model resolution exactly. Unlike
    /// [`Self::update_preferred_agent_mode_llm`], the copied override is not
    /// normalized against the destination's current profile default, so it is
    /// order-independent with respect to the profile copy.
    pub(crate) fn copy_agent_mode_selection(
        &mut self,
        source_terminal_view_id: EntityId,
        new_terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = match self
            .base_llm_for_terminal_view
            .get(&source_terminal_view_id)
            .cloned()
        {
            Some(id) => {
                self.base_llm_for_terminal_view
                    .insert(new_terminal_view_id, id.clone())
                    != Some(id)
            }
            None => self
                .base_llm_for_terminal_view
                .remove(&new_terminal_view_id)
                .is_some(),
        };

        if changed {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    /// Triggers a snapshot save to persist LLM override changes.
    fn trigger_snapshot_save(&self, ctx: &mut ModelContext<Self>) {
        ctx.dispatch_global_action("workspace:save_app", ());
    }

    pub fn update_preferred_coding_llm(
        &self,
        scope: &(impl TeamScope + ?Sized),
        preferred_llm_id: &LLMId,
        terminal_view_id: Option<EntityId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let new_value = if preferred_llm_id
            == &UserWorkspaces::as_ref(ctx)
                .feature_model_choice_for_scope(scope)
                .coding
                .default_id
        {
            None
        } else {
            Some(preferred_llm_id.clone())
        };

        let mut changed = false;
        AIExecutionProfilesModel::handle(ctx).update(ctx, |profiles, ctx| {
            let profile = profiles.active_profile(terminal_view_id, ctx);

            if profile.data().coding_model != new_value {
                profiles.set_coding_model(profile.id(), new_value, ctx);
                changed = true;
            }
        });

        if changed {
            ctx.emit(LLMPreferencesEvent::UpdatedActiveCodingLLM);
        }
    }

    pub fn new_choices_since_last_update(&self) -> Option<Vec<LLMInfo>> {
        self.last_update.as_ref().map(|update| {
            // We don't want to display new choices if they are warp branded.
            let filter_choices: Vec<LLMInfo> = update
                .new_choices
                .clone()
                .into_iter()
                .filter(|choice| !choice.display_name.starts_with("lite"))
                .collect();

            filter_choices
        })
    }

    pub fn should_show_new_choices_popup(&self, view_id: EntityId) -> bool {
        self.last_update.as_ref().is_some_and(|update| {
            let popup_state = &*update.popup_visibility_state.lock();
            matches!(popup_state, UpdatePopupVisibilityState::WaitingToBeShown)
                || matches!(
                popup_state,
                UpdatePopupVisibilityState::Visible(id) if *id == view_id)
        })
    }

    pub fn mark_new_choices_popup_as_shown(&self, view_id: EntityId) {
        if let Some(update) = self.last_update.as_ref()
            && matches!(
                &*update.popup_visibility_state.lock(),
                UpdatePopupVisibilityState::WaitingToBeShown
            )
        {
            *update.popup_visibility_state.lock() = UpdatePopupVisibilityState::Visible(view_id);
        }
    }

    pub fn hide_llm_popup(&self, view_id: EntityId) {
        if !self.should_show_new_choices_popup(view_id) {
            return;
        }
        let Some(last_update) = self.last_update.as_ref() else {
            return;
        };
        *last_update.popup_visibility_state.lock() = UpdatePopupVisibilityState::Hidden;
    }

    /// Offers `models`, which must not be empty, in every model picker.
    #[cfg(not(target_family = "wasm"))]
    pub fn set_local_models(
        &mut self,
        models: &[local_agent::LocalModel],
        ctx: &mut ModelContext<Self>,
    ) {
        let choices = || models.iter().map(local_llm_info).collect::<Vec<_>>();
        let default_id: LLMId = models
            .first()
            .map(|model| model.name.clone())
            .unwrap_or_default()
            .into();
        let available = || AvailableLLMs::new(default_id.clone(), choices(), None);
        match (available(), available()) {
            (Ok(agent_mode), Ok(coding)) => self.on_server_update(
                None,
                ModelsByFeature {
                    agent_mode,
                    coding,
                    cli_agent: None,
                    computer_use: None,
                },
                ctx,
            ),
            (Err(error), _) | (_, Err(error)) => report_error!(error),
        }
    }

    pub fn update_feature_model_choices(
        &mut self,
        choices_result: Result<ModelsByFeature, anyhow::Error>,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Ok(choices) = choices_result {
            self.on_server_update(None, choices, ctx);
        }
    }

    fn on_server_update(
        &mut self,
        team_uid: Option<ServerId>,
        update: ModelsByFeature,
        ctx: &mut ModelContext<Self>,
    ) {
        // Clear the unavailable flag on every successful model-list update.
        self.set_agent_mode_models_unavailable_for_team_uid(team_uid, false);

        let old = UserWorkspaces::as_ref(ctx)
            .feature_model_choice_for_team_uid(team_uid)
            .clone();
        let has_existing_persisted_config = old != ModelsByFeature::default();

        UserWorkspaces::handle(ctx).update(ctx, |workspaces, _| {
            workspaces.set_feature_model_choice_for_team_uid(team_uid, update);
        });

        self.reconcile_disabled_model_preferences(team_uid, ctx);

        let new_choices = get_new_agent_mode_choices(
            &old.agent_mode,
            &UserWorkspaces::as_ref(ctx)
                .feature_model_choice_for_team_uid(team_uid)
                .agent_mode,
        );
        if !new_choices.is_empty() {
            self.last_update = Some(AvailableLLMsUpdate {
                new_choices,
                // We shouldn't show the update for the initial LLM config creation.
                popup_visibility_state: Arc::new(FairMutex::new(
                    if has_existing_persisted_config {
                        UpdatePopupVisibilityState::WaitingToBeShown
                    } else {
                        UpdatePopupVisibilityState::Hidden
                    },
                )),
            });
        }

        ctx.emit(LLMPreferencesEvent::UpdatedAvailableLLMs);
    }

    /// Reconcile stored model selections with the current model catalog and credentials.
    ///
    /// An `AdminDisabled` base-model selection and its context limit are preserved because
    /// profiles are shared across teams. Other unusable base, coding, CLI agent, and computer use
    /// selections are cleared.
    ///
    /// Note: model selections are only cleared when the model ID is *recognized*
    /// on this device (present in the model catalog). An unrecognized ID is silently
    /// preserved so that cross-device profiles are not erroneously reset and synced
    /// back to cloud, which would destroy the user's settings on their primary device.
    fn reconcile_disabled_model_preferences(
        &self,
        team_uid: Option<ServerId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let models_by_feature = UserWorkspaces::as_ref(ctx)
            .feature_model_choice_for_team_uid(team_uid)
            .clone();
        let profiles_model = AIExecutionProfilesModel::handle(ctx);
        profiles_model.update(ctx, |profiles, ctx| {
            for profile_id in profiles.get_all_profile_ids() {
                if let Some(profile) = profiles.get_profile_by_id(&profile_id, ctx) {
                    let profile_data = profile.data();
                    let preferred_base_model = profile_data.base_model.clone();
                    let effective_base_model_id = preferred_base_model
                        .as_ref()
                        .unwrap_or(&models_by_feature.agent_mode.default_id);
                    let preferred_base_model_info = models_by_feature
                        .agent_mode
                        .info_for_id(effective_base_model_id);
                    let preferred_base_model_is_recognized =
                        preferred_base_model.is_none() || preferred_base_model_info.is_some();
                    let preferred_base_model_is_admin_disabled = preferred_base_model_info
                        .is_some_and(|info| {
                            info.disable_reason == Some(DisableReason::AdminDisabled)
                        });
                    let effective_base_model_usable = models_by_feature
                        .agent_mode
                        .usable_info_for_id(effective_base_model_id);
                    let effective_base_model_unusable = effective_base_model_usable.is_none();
                    let effective_base_model_is_configurable = effective_base_model_usable
                        .is_some_and(|info| info.context_window.is_configurable);
                    let has_context_window_limit = profile_data.context_window_limit.is_some();

                    if preferred_base_model.is_some()
                        && preferred_base_model_is_recognized
                        && !preferred_base_model_is_admin_disabled
                        && effective_base_model_unusable
                    {
                        profiles.set_base_model(&profile_id, None, ctx);
                    }
                    if has_context_window_limit
                        && preferred_base_model_is_recognized
                        && !preferred_base_model_is_admin_disabled
                        && (effective_base_model_unusable || !effective_base_model_is_configurable)
                    {
                        profiles.set_context_window_limit(&profile_id, None, ctx);
                    }
                    if let Some(preferred_llm_id) = &profile.data().coding_model {
                        let is_recognized = models_by_feature
                            .coding
                            .info_for_id(preferred_llm_id)
                            .is_some();
                        if is_recognized
                            && models_by_feature
                                .coding
                                .usable_info_for_id(preferred_llm_id)
                                .is_none()
                        {
                            profiles.set_coding_model(&profile_id, None, ctx);
                        }
                    }
                    if let Some(preferred_llm_id) = &profile.data().cli_agent_model {
                        let is_recognized = self
                            .get_cli_agent_available(team_uid, ctx)
                            .info_for_id(preferred_llm_id)
                            .is_some();
                        if is_recognized
                            && self
                                .get_cli_agent_available(team_uid, ctx)
                                .usable_info_for_id(preferred_llm_id)
                                .is_none()
                        {
                            profiles.set_cli_agent_model(&profile_id, None, ctx);
                        }
                    }
                    if let Some(preferred_llm_id) = &profile.data().computer_use_model
                        && self
                            .get_computer_use_available(team_uid, ctx)
                            .usable_info_for_id(preferred_llm_id)
                            .is_none()
                    {
                        profiles.set_computer_use_model(&profile_id, None, ctx);
                    }
                }
            }
        });
    }

    pub fn vision_supported(
        &self,
        scope: &(impl TeamScope + ?Sized),
        app: &AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> bool {
        self.vision_supported_for_team_uid(scope.team_uid(), app, terminal_view_id)
    }

    pub fn vision_supported_for_team_uid(
        &self,
        team_uid: Option<ServerId>,
        app: &AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> bool {
        self.get_active_base_model_for_team_uid(team_uid, app, terminal_view_id)
            .vision_supported
    }

    pub(crate) fn reset_session_overrides(&mut self) {
        self.base_llm_for_terminal_view.clear();
        self.computer_use_llm_for_terminal_view.clear();
    }

    pub fn get_base_llm_override(&self, terminal_view_id: EntityId) -> Option<String> {
        if let Some(override_str) = self
            .base_llm_for_terminal_view
            .get(&terminal_view_id)
            .and_then(|llm_id| serde_json::to_string(llm_id).ok())
        {
            return Some(override_str);
        }

        log::debug!("LLM override not found in memory for terminal view: {terminal_view_id:?}");
        None
    }

    /// Removes the LLM override for a terminal view.
    /// This ensures that the new profile's default model is used.
    pub fn remove_llm_override(
        &mut self,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let old = self.base_llm_for_terminal_view.remove(&terminal_view_id);
        if old.is_some() {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self {
            agent_mode_models_unavailable: HashMap::new(),
            last_update: None,
            base_llm_for_terminal_view: HashMap::new(),
            computer_use_llm_for_terminal_view: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum LLMPreferencesEvent {
    UpdatedAvailableLLMs,
    UpdatedActiveAgentModeLLM,
    UpdatedActiveCodingLLM,
}

impl Entity for LLMPreferences {
    type Event = LLMPreferencesEvent;
}

impl SingletonEntity for LLMPreferences {}

fn get_new_agent_mode_choices(
    old_config: &AvailableLLMs,
    new_config: &AvailableLLMs,
) -> Vec<LLMInfo> {
    let old_ids: HashSet<_> = old_config.choices.iter().map(|info| &info.id).collect();
    new_config
        .choices
        .iter()
        .filter(|info| !old_ids.contains(&info.id))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "llms_tests.rs"]
mod tests;
