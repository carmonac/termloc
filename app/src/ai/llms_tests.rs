use std::sync::Arc;

use warpui::App;

use super::*;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::auth::AuthStateProvider;
use crate::auth::auth_manager::AuthManager;
use crate::cloud_object::model::persistence::CloudModel;
use crate::network::NetworkStatus;
use crate::server::cloud_objects::update_manager::UpdateManager;
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::team::MockTeamClient;
use crate::server::server_api::workspace::MockWorkspaceClient;
use crate::server::sync_queue::SyncQueue;
use crate::settings::PrivacySettings;
use crate::terminal::input::models::query_model_picker_choices;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::team::Team;
use crate::workspaces::team_tester::TeamTesterStatus;
use crate::workspaces::user_workspaces::{
    TeamContextForOperation, TeamlessScopeForTest, UserWorkspaces,
};
use crate::workspaces::workspace::Workspace;
use crate::{LaunchMode, TuiEntryPoint};

// -- DisableReason::should_clear_preference tests --

#[test]
fn should_clear_preference_admin_disabled() {
    assert!(DisableReason::AdminDisabled.should_clear_preference());
}

#[test]
fn should_clear_preference_unavailable() {
    assert!(DisableReason::Unavailable.should_clear_preference());
}

#[test]
fn should_not_clear_preference_out_of_requests() {
    // Transient — never clears.
    assert!(!DisableReason::OutOfRequests.should_clear_preference());
}

#[test]
fn should_not_clear_preference_provider_outage() {
    // Transient — never clears.
    assert!(!DisableReason::ProviderOutage.should_clear_preference());
}

#[test]
fn should_clear_preference_requires_upgrade() {
    assert!(DisableReason::RequiresUpgrade.should_clear_preference());
}

#[test]
fn llm_info_deserializes_without_base_model_name() {
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": {
                "request_multiplier": 1,
                "credit_multiplier": null
            },
            "description": null,
            "disable_reason": null,
            "vision_supported": false,
            "spec": null,
            "provider": "Unknown"
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.base_model_name, "gpt-4o");
}

#[test]
fn llm_info_deserializes_host_configs_as_vec() {
    // Wire format from server: host_configs is a Vec
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": { "request_multiplier": 1, "credit_multiplier": null },
            "provider": "OpenAI",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" },
                { "enabled": false, "model_routing_host": "RemovedHost" }
            ]
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize vec format");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.host_configs.len(), 2);
    assert!(
        info.host_configs
            .get(&LLMModelHost::DirectApi)
            .unwrap()
            .enabled
    );
    assert!(
        !info
            .host_configs
            .get(&LLMModelHost::Unknown)
            .unwrap()
            .enabled
    );
}

#[test]
fn llm_info_round_trip_serializes_and_deserializes() {
    // Start with wire format (Vec)
    let wire_json = r#"{
            "display_name": "claude-3",
            "base_model_name": "claude-3",
            "id": "claude-3",
            "usage_metadata": { "request_multiplier": 2, "credit_multiplier": 1.5 },
            "description": "A powerful model",
            "vision_supported": true,
            "provider": "Anthropic",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" }
            ]
        }"#;

    // Deserialize from wire format
    let info: LLMInfo = serde_json::from_str(wire_json).expect("should deserialize");

    // Serialize (produces HashMap format)
    let serialized = serde_json::to_string(&info).expect("should serialize");

    // Deserialize again (from HashMap format)
    let round_tripped: LLMInfo =
        serde_json::from_str(&serialized).expect("should deserialize after round trip");

    assert_eq!(info, round_tripped);
}

fn disabled_agent_llm(id: &str, display_name: &str) -> LLMInfo {
    LLMInfo {
        disable_reason: Some(DisableReason::Unavailable),
        ..agent_llm(id, display_name)
    }
}

// -- Disable-aware default fallback tests --

fn server_llm(id: &str, disable_reason: Option<DisableReason>) -> LLMInfo {
    LLMInfo {
        display_name: id.to_string(),
        base_model_name: id.to_string(),
        id: id.into(),
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 1,
            credit_multiplier: None,
        },
        description: None,
        disable_reason,
        vision_supported: false,
        spec: None,
        provider: LLMProvider::Unknown,
        host_configs: HashMap::new(),
        discount_percentage: None,
        context_window: LLMContextWindow::default(),
    }
}

fn available(default_id: &str, choices: Vec<LLMInfo>) -> AvailableLLMs {
    AvailableLLMs {
        default_id: default_id.into(),
        choices,
        preferred_codex_model_id: None,
    }
}

#[test]
fn deserialized_available_llms_with_missing_default_does_not_panic() {
    // `AvailableLLMs::new()` guarantees `default_id` is one of `choices`, but
    // deserialization (e.g. a stale persisted cache or a server payload)
    // bypasses `new()`. Build such a struct, round-trip it through serde, and
    // confirm `default_llm_info()` falls back to the first choice instead of
    // panicking (Sentry: "Default LLM ID must be present in choices").
    let original = available(
        "missing-default",
        vec![server_llm("gpt-x", None), server_llm("gpt-y", None)],
    );
    let json = serde_json::to_string(&original).expect("should serialize");
    let deserialized: AvailableLLMs = serde_json::from_str(&json).expect("should deserialize");

    assert_eq!(deserialized.default_id.as_str(), "missing-default");
    assert_eq!(deserialized.default_llm_info().id.as_str(), "gpt-x");
}

#[test]
fn active_models_fall_back_to_usable_choice_when_default_disabled() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);

        // The base/coding default is admin-disabled but another hosted choice
        // is usable; every hosted CLI agent choice is admin-disabled.
        let models = ModelsByFeature {
            agent_mode: available(
                "auto",
                vec![
                    server_llm("auto", Some(DisableReason::AdminDisabled)),
                    server_llm("gpt-x", None),
                ],
            ),
            coding: available(
                "auto",
                vec![
                    server_llm("auto", Some(DisableReason::AdminDisabled)),
                    server_llm("gpt-x", None),
                ],
            ),
            cli_agent: Some(available(
                "cli-agent-auto",
                vec![server_llm(
                    "cli-agent-auto",
                    Some(DisableReason::AdminDisabled),
                )],
            )),
            computer_use: None,
        };
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(models), ctx);
        });

        llm_preferences.read(&app, |preferences, app| {
            // Falls back to the first usable hosted choice.
            assert_eq!(
                preferences
                    .get_active_base_model(&TeamlessScopeForTest, app, None)
                    .id
                    .as_str(),
                "gpt-x"
            );
            assert_eq!(
                preferences
                    .get_active_coding_model(&TeamlessScopeForTest, app, None)
                    .id
                    .as_str(),
                "gpt-x"
            );
            // No usable CLI choice → falls back to the (disabled) default.
            assert_eq!(
                preferences
                    .get_active_cli_agent_model(&TeamlessScopeForTest, app, None)
                    .id
                    .as_str(),
                "cli-agent-auto"
            );
        });
    });
}

/// Runs picker-query assertions with searchable, selectable, and disabled model fixtures plus
/// the app singletons consulted by model eligibility logic. The scope is teamless: these
/// fixtures exercise ordering and filtering, not team credential policy.
fn with_model_picker_query_test_context(
    f: impl FnOnce(&LLMPreferences, &dyn TeamScope, &AppContext) + 'static,
) {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(UserWorkspaces::default_mock);
        let scope = UserWorkspaces::teamless_context_resolver_for_test();

        let agent_mode = AvailableLLMs::new(
            "auto".into(),
            vec![
                agent_llm("auto", "auto (cost-efficient)"),
                agent_llm("gpt-5", "GPT 5"),
                disabled_agent_llm("disabled-gpt", "GPT Disabled"),
            ],
            None,
        )
        .expect("choices are non-empty");
        UserWorkspaces::handle(&app).update(&mut app, |workspaces, _| {
            workspaces.set_workspaceless_models_by_feature(ModelsByFeature {
                agent_mode,
                ..Default::default()
            });
        });

        let preferences = LLMPreferences::for_test();
        app.read(|app_ctx| {
            f(&preferences, &scope(app_ctx), app_ctx);
        });
    });
}

#[test]
fn active_models_use_default_when_usable() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);

        let models = ModelsByFeature {
            agent_mode: available(
                "auto",
                vec![server_llm("auto", None), server_llm("gpt-x", None)],
            ),
            coding: available("auto", vec![server_llm("auto", None)]),
            cli_agent: Some(available(
                "cli-agent-auto",
                vec![server_llm("cli-agent-auto", None)],
            )),
            computer_use: None,
        };
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(models), ctx);
        });

        llm_preferences.read(&app, |preferences, app| {
            assert_eq!(
                preferences
                    .get_active_base_model(&TeamlessScopeForTest, app, None)
                    .id
                    .as_str(),
                "auto"
            );
            assert_eq!(
                preferences
                    .get_active_cli_agent_model(&TeamlessScopeForTest, app, None)
                    .id
                    .as_str(),
                "cli-agent-auto"
            );
        });
    });
}

#[test]
fn team_catalog_hydration_preserves_shared_profile_and_resolves_against_active_team() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(PrivacySettings::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        let fable_id = LLMId::from("claude-5-1-fable");
        let mut enabled_fable = server_llm(fable_id.as_str(), None);
        enabled_fable.context_window.is_configurable = true;
        let mut disabled_fable = server_llm(fable_id.as_str(), Some(DisableReason::AdminDisabled));
        disabled_fable.context_window.is_configurable = true;
        let enabled_models = ModelsByFeature {
            agent_mode: available(
                "auto-genius",
                vec![server_llm("auto-genius", None), enabled_fable],
            ),
            ..Default::default()
        };
        let disabled_models = ModelsByFeature {
            agent_mode: available(
                "auto-genius",
                vec![server_llm("auto-genius", None), disabled_fable],
            ),
            ..Default::default()
        };
        let engineering_team_uid = ServerId::from(101);
        let disabled_team_uid = ServerId::from(102);
        let engineering_team = Team::from_local_cache(
            engineering_team_uid,
            "Engineering".to_string(),
            None,
            None,
            None,
            Some(enabled_models.clone()),
        );
        let disabled_team = Team::from_local_cache(
            disabled_team_uid,
            "Disabled team".to_string(),
            None,
            None,
            None,
            Some(disabled_models.clone()),
        );
        let workspace = Workspace::from_local_cache(
            ServerId::from(100).into(),
            "Warp".to_string(),
            Some(vec![engineering_team, disabled_team]),
            Some(disabled_models.clone()),
        );
        let refreshed_workspace = workspace.clone();
        app.add_singleton_model(|ctx| {
            UserWorkspaces::mock(
                Arc::new(MockTeamClient::new()),
                Arc::new(MockWorkspaceClient::new()),
                vec![workspace],
                ctx,
            )
        });
        let profiles_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);
        let profile_id = profiles_model.read(&app, |profiles, _| profiles.default_profile_id());
        profiles_model.update(&mut app, |profiles, ctx| {
            profiles.set_base_model(&profile_id, Some(fable_id.clone()), ctx);
            profiles.set_context_window_limit(&profile_id, Some(200_000), ctx);
        });
        UserWorkspaces::handle(&app).update(&mut app, |workspaces, ctx| {
            workspaces.update_workspaces(vec![refreshed_workspace], ctx);
        });

        let engineering_scope = TeamContextForOperation::new_for_test(engineering_team_uid);
        let disabled_scope = TeamContextForOperation::new_for_test(disabled_team_uid);
        llm_preferences.read(&app, |preferences, ctx| {
            assert_eq!(
                preferences
                    .get_active_base_model(&engineering_scope, ctx, None)
                    .id,
                fable_id
            );
            assert_eq!(
                preferences
                    .get_active_base_model(&disabled_scope, ctx, None)
                    .id
                    .as_str(),
                "auto-genius"
            );
            let disabled_fable = preferences
                .get_base_llm_choices_for_agent_mode(&disabled_scope, ctx)
                .find(|llm| llm.id == fable_id)
                .expect("the preferred model should remain visible on the disabled team");
            assert_eq!(
                disabled_fable.disable_reason,
                Some(DisableReason::AdminDisabled)
            );
        });
        profiles_model.read(&app, |profiles, ctx| {
            assert_eq!(
                profiles.default_profile(ctx).data().base_model.as_ref(),
                Some(&fable_id)
            );
            assert_eq!(
                profiles.default_profile(ctx).data().context_window_limit,
                Some(200_000)
            );
        });
    });
}

#[test]
fn reconcile_clears_unavailable_base_model_preference() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        let profiles_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);
        let profile_id = profiles_model.read(&app, |profiles, _| profiles.default_profile_id());
        let unavailable_model_id = LLMId::from("unavailable-model");
        let mut unavailable_model = server_llm(
            unavailable_model_id.as_str(),
            Some(DisableReason::Unavailable),
        );
        unavailable_model.context_window.is_configurable = true;
        profiles_model.update(&mut app, |profiles, ctx| {
            profiles.set_base_model(&profile_id, Some(unavailable_model_id), ctx);
            profiles.set_context_window_limit(&profile_id, Some(200_000), ctx);
        });
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(
                Ok(ModelsByFeature {
                    agent_mode: available(
                        "auto-genius",
                        vec![server_llm("auto-genius", None), unavailable_model],
                    ),
                    ..Default::default()
                }),
                ctx,
            );
        });

        profiles_model.read(&app, |profiles, ctx| {
            let profile = profiles.default_profile(ctx);
            assert_eq!(profile.data().base_model, None);
            assert_eq!(profile.data().context_window_limit, None);
        });
    });
}

#[test]
fn reconcile_clears_requires_upgrade_base_model_preference() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        let profiles_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);
        let profile_id = profiles_model.read(&app, |profiles, _| profiles.default_profile_id());

        let upgrade_model_id = LLMId::from("upgrade-model");
        let mut upgrade_model = server_llm(
            upgrade_model_id.as_str(),
            Some(DisableReason::RequiresUpgrade),
        );
        upgrade_model.context_window.is_configurable = true;
        profiles_model.update(&mut app, |profiles, ctx| {
            profiles.set_base_model(&profile_id, Some(upgrade_model_id), ctx);
            profiles.set_context_window_limit(&profile_id, Some(200_000), ctx);
        });
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(
                Ok(ModelsByFeature {
                    agent_mode: available(
                        "auto-genius",
                        vec![server_llm("auto-genius", None), upgrade_model],
                    ),
                    ..Default::default()
                }),
                ctx,
            );
        });

        profiles_model.read(&app, |profiles, ctx| {
            let profile = profiles.default_profile(ctx);
            assert_eq!(profile.data().base_model, None);
            assert_eq!(profile.data().context_window_limit, None);
        });
    });
}

// -- execution-profile model selection tests --

fn agent_llm(id: &str, display_name: &str) -> LLMInfo {
    LLMInfo {
        display_name: display_name.to_owned(),
        base_model_name: display_name.to_owned(),
        id: id.into(),
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
    }
}

#[cfg(not(target_family = "wasm"))]
fn with_computer_use_test_context(f: impl FnOnce(&mut App) + 'static) {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(CloudModel::mock);
        app.add_singleton_model(TeamTesterStatus::mock);
        app.add_singleton_model(SyncQueue::mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(
                &LaunchMode::Tui {
                    entrypoint: TuiEntryPoint::Interactive {
                        mount: Box::new(|_| {}),
                        api_key: None,
                    },
                },
                ctx,
            )
        });
        app.add_singleton_model(|ctx| {
            UserWorkspaces::handle(ctx).update(ctx, |workspaces, _| {
                workspaces.set_workspaceless_models_by_feature(ModelsByFeature {
                    computer_use: Some(
                        AvailableLLMs::new(
                            "computer-use-agent-auto".into(),
                            vec![
                                agent_llm("computer-use-agent-auto", "auto"),
                                agent_llm("computer-use-model", "selected"),
                                agent_llm("profile-model", "profile"),
                            ],
                            None,
                        )
                        .unwrap(),
                    ),
                    ..Default::default()
                });
            });
            LLMPreferences::for_test()
        });
        f(&mut app);
    });
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn computer_use_model_falls_back_to_the_profile_without_a_terminal_override() {
    with_computer_use_test_context(|app| {
        let profiles = AIExecutionProfilesModel::handle(app);
        let preferences = LLMPreferences::handle(app);
        let terminal_view_id = EntityId::new();
        profiles.update(app, |profiles, ctx| {
            profiles.set_computer_use_model(
                &profiles.default_profile_id(),
                Some("profile-model".into()),
                ctx,
            );
        });

        preferences.read(app, |preferences, ctx| {
            assert_eq!(
                preferences
                    .get_active_computer_use_model(
                        &TeamlessScopeForTest,
                        ctx,
                        Some(terminal_view_id)
                    )
                    .id
                    .as_str(),
                "profile-model"
            );
        });
    });
}

#[test]
fn shared_model_picker_query_orders_filters_and_marks_disabled_choices() {
    with_model_picker_query_test_context(|preferences, _scope, app| {
        let all = query_model_picker_choices(
            preferences.get_base_llm_choices_for_agent_mode(&TeamlessScopeForTest, app),
            "",
        );
        assert_eq!(
            all.first().map(|choice| choice.llm.id.as_str()),
            Some("auto")
        );
        assert_eq!(
            all.last().map(|choice| choice.llm.id.as_str()),
            Some("disabled-gpt")
        );
        assert!(!all.last().expect("disabled choice").is_selectable());

        let filtered = query_model_picker_choices(
            preferences.get_base_llm_choices_for_agent_mode(&TeamlessScopeForTest, app),
            "gpt 5",
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].llm.id.as_str(), "gpt-5");
        assert!(filtered[0].name_match_result.is_some());
        assert!(filtered[0].is_selectable());
    });
}

#[test]
fn local_models_report_the_ollama_provider() {
    let info = local_llm_info(&local_agent::LocalModel {
        name: "llama3".to_string(),
        supports_tools: true,
        supports_vision: true,
        supports_thinking: false,
        supports_embedding: false,
        context_length: None,
        size: 0,
    });
    assert_eq!(info.provider, LLMProvider::Ollama);
    assert_eq!(info.provider.display_name(), "Ollama");
    assert!(info.vision_supported);
}

#[test]
fn removed_providers_in_cached_model_lists_deserialize_as_unknown() {
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": { "request_multiplier": 1, "credit_multiplier": null },
            "provider": "OpenAI"
        }"#;
    let info: LLMInfo = serde_json::from_str(raw).expect("cached models should still load");
    assert_eq!(info.provider, LLMProvider::Unknown);
}
