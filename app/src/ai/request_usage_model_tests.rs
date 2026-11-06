use std::sync::Arc;

use chrono::Duration;
use warp_core::features::FeatureFlag;
use warp_graphql::billing::{AddonCreditsOption, OveragesPricing, PricingInfo};
use warpui::{App, ModelHandle};

use super::*;
use crate::ai::credit_availability::{AICreditAvailability, AICreditDenialReason, AICreditSource};
use crate::auth::AuthStateProvider;
use crate::pricing::PricingInfoModel;
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::team::MockTeamClient;
use crate::server::server_api::workspace::MockWorkspaceClient;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::{
    AiOverages, CustomerType, EnterpriseCreditsAutoReloadPolicy, EnterprisePayAsYouGoPolicy,
    PurchaseAddOnCreditsPolicy, Workspace, WorkspaceUid,
};

fn create_test_workspace() -> (WorkspaceUid, Workspace) {
    let server_id: crate::server::ids::ServerId = 1_i64.into();
    let uid = WorkspaceUid::from(server_id);
    let workspace = Workspace::from_local_cache(uid, "Test Workspace".to_string(), None, None);
    (uid, workspace)
}

fn add_user_workspaces_with_workspace(app: &mut App, workspace: Workspace) {
    app.add_singleton_model(|ctx| {
        UserWorkspaces::mock(
            Arc::new(MockTeamClient::new()),
            Arc::new(MockWorkspaceClient::new()),
            vec![workspace],
            ctx,
        )
    });
}

fn add_request_usage_model(app: &mut App) -> ModelHandle<AIRequestUsageModel> {
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    add_request_usage_model_without_auth(app)
}

fn add_request_usage_model_for_logged_out_users(app: &mut App) -> ModelHandle<AIRequestUsageModel> {
    app.add_singleton_model(|_| AuthStateProvider::new_logged_out_for_test());
    add_request_usage_model_without_auth(app)
}
fn register_user_preferences_for_tests(app: &mut App) {
    if app
        .models_of_type::<settings::PrivatePreferences>()
        .is_empty()
    {
        app.update(crate::settings::init_and_register_user_preferences);
    }
}

fn add_request_usage_model_without_auth(app: &mut App) -> ModelHandle<AIRequestUsageModel> {
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    register_user_preferences_for_tests(app);
    app.update(|ctx| {
        warpui_extras::secure_storage::register_noop("test", ctx);
    });
    app.add_singleton_model(|_| PricingInfoModel::new());
    app.add_singleton_model(|ctx| {
        AIRequestUsageModel::new_for_test(ServerApiProvider::as_ref(ctx).get_ai_client(), ctx)
    })
}

fn set_addon_credits_pricing_info(app: &mut App) {
    PricingInfoModel::handle(app).update(app, |model, ctx| {
        model.update_pricing_info(
            PricingInfo {
                plans: vec![],
                overages: OveragesPricing {
                    price_per_request_usd_cents: 1,
                },
                addon_credits_options: vec![AddonCreditsOption {
                    credits: 1000,
                    price_usd_cents: 1000,
                    usage_cents: None,
                }],
                promotion_message: None,
            },
            ctx,
        );
    });
}

fn standard_purchase_policy() -> PurchaseAddOnCreditsPolicy {
    PurchaseAddOnCreditsPolicy {
        enabled: true,
        premium_enabled: false,
        price_premium_bps: 0,
    }
}

fn premium_purchase_policy() -> PurchaseAddOnCreditsPolicy {
    PurchaseAddOnCreditsPolicy {
        enabled: false,
        premium_enabled: true,
        price_premium_bps: 1000,
    }
}

fn enable_auto_reload(workspace: &mut Workspace) {
    workspace
        .settings
        .addon_credits_settings
        .auto_reload_enabled = true;
    workspace
        .settings
        .addon_credits_settings
        .selected_auto_reload_credit_denomination = Some(1000);
}

#[test]
fn refresh_request_usage_returns_no_fresh_limit_when_logged_out() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model_for_logged_out_users(&mut app);
        let refresh =
            request_usage_model.update(&mut app, |model, ctx| model.refresh_request_usage(ctx));

        assert_eq!(refresh.await.unwrap(), None);
    });
}
#[test]
fn test_request_limit_info() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model(&mut app);
        request_usage_model.update(&mut app, |request_usage_model, _ctx| {
            request_usage_model.request_limit_info = RequestLimitInfo {
                limit: 200,
                num_requests_used_since_refresh: 39,
                included_usage_cents: None,
                usage_cents_used_since_last_refresh: None,
                next_refresh_time: ServerTimestamp::new(Utc::now() + Duration::days(1)),
                is_unlimited: false,
                request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
                is_unlimited_voice: false,
                voice_request_limit: 100,
                voice_requests_used_since_last_refresh: 0,
                is_unlimited_codebase_indices: false,
                max_codebase_indices: 3,
                max_files_per_repo: 5000,
                embedding_generation_batch_size: 100,
            };
            assert_eq!(200, request_usage_model.request_limit());
            assert_eq!(39, request_usage_model.requests_used());
            assert_eq!(161, request_usage_model.requests_remaining());
        })
    });
}

#[test]
fn test_cached_request_limit_info_without_usage_cents_deserializes() {
    let mut cached = serde_json::to_value(RequestLimitInfo::default()).unwrap();
    let cached_object = cached.as_object_mut().unwrap();
    cached_object.remove("included_usage_cents");
    cached_object.remove("usage_cents_used_since_last_refresh");

    let request_limit_info: RequestLimitInfo = serde_json::from_value(cached).unwrap();

    assert_eq!(request_limit_info.included_usage_cents, None);
    assert_eq!(request_limit_info.usage_cents_used_since_last_refresh, None);
}

#[test]
fn test_request_limit_info_round_trips_usage_cents() {
    let request_limit_info = RequestLimitInfo {
        included_usage_cents: Some(1800.0),
        usage_cents_used_since_last_refresh: Some(70.2),
        ..RequestLimitInfo::default()
    };

    let serialized = serde_json::to_string(&request_limit_info).unwrap();
    let deserialized: RequestLimitInfo = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.included_usage_cents, Some(1800.0));
    assert_eq!(deserialized.usage_cents_used_since_last_refresh, Some(70.2));
}

#[test]
fn test_request_limit_info_with_limit() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model(&mut app);
        request_usage_model.update(&mut app, |request_usage_model, _ctx| {
            request_usage_model.request_limit_info = RequestLimitInfo {
                limit: 999999999,
                num_requests_used_since_refresh: 39,
                included_usage_cents: None,
                usage_cents_used_since_last_refresh: None,
                next_refresh_time: ServerTimestamp::new(Utc::now() + Duration::minutes(1)),
                is_unlimited: false,
                request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
                is_unlimited_voice: false,
                voice_request_limit: 100,
                voice_requests_used_since_last_refresh: 0,
                is_unlimited_codebase_indices: false,
                max_codebase_indices: 3,
                max_files_per_repo: 5000,
                embedding_generation_batch_size: 100,
            };
            assert_eq!(999999999, request_usage_model.request_limit());
            assert_eq!(39, request_usage_model.requests_used());
            assert_eq!(999999960, request_usage_model.requests_remaining());
        })
    });
}

#[test]
fn test_request_limit_info_past_refresh_time() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model(&mut app);
        request_usage_model.update(&mut app, |request_usage_model, _ctx| {
            request_usage_model.request_limit_info = RequestLimitInfo {
                limit: 200,
                num_requests_used_since_refresh: 39,
                included_usage_cents: None,
                usage_cents_used_since_last_refresh: None,
                next_refresh_time: ServerTimestamp::new(Utc::now() - Duration::seconds(1)),
                is_unlimited: false,
                request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
                is_unlimited_voice: false,
                voice_request_limit: 100,
                voice_requests_used_since_last_refresh: 0,
                is_unlimited_codebase_indices: false,
                max_codebase_indices: 3,
                max_files_per_repo: 5000,
                embedding_generation_batch_size: 100,
            };
            assert_eq!(200, request_usage_model.request_limit());
            assert_eq!(0, request_usage_model.requests_used());
            assert_eq!(200, request_usage_model.requests_remaining());
        })
    });
}

#[test]
fn test_request_limit_info_is_unlimited_true() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model(&mut app);
        request_usage_model.update(&mut app, |request_usage_model, _ctx| {
            request_usage_model.request_limit_info = RequestLimitInfo {
                limit: 999999999,
                num_requests_used_since_refresh: 39,
                included_usage_cents: None,
                usage_cents_used_since_last_refresh: None,
                next_refresh_time: ServerTimestamp::new(Utc::now() + Duration::minutes(1)),
                is_unlimited: true,
                request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
                is_unlimited_voice: false,
                voice_request_limit: 100,
                voice_requests_used_since_last_refresh: 0,
                is_unlimited_codebase_indices: false,
                max_codebase_indices: 3,
                max_files_per_repo: 5000,
                embedding_generation_batch_size: 100,
            };
            assert_eq!(999999999, request_usage_model.request_limit());
            assert_eq!(39, request_usage_model.requests_used());
            assert_eq!(999999999, request_usage_model.requests_remaining());
        })
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_remaining_requests() {
    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // Some requests remaining, no bonus or overages needed.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 5);
            assert!(model.has_any_ai_remaining(ctx));
        });
    });
}

#[test]
fn test_ambient_credits_banner_dismissal_is_persisted() {
    App::test((), |mut app| async move {
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            assert!(!model.is_ambient_credits_banner_dismissed());
            model.dismiss_ambient_credits_banner(ctx);
            assert!(model.is_ambient_credits_banner_dismissed());
        });

        app.update(|ctx| {
            let stored_value = ctx
                .private_user_preferences()
                .read_value(AMBIENT_CREDITS_BANNER_DISMISSED_KEY)
                .unwrap();
            assert_eq!(stored_value, Some("true".to_owned()));
        });
    });
}

#[test]
fn test_ambient_credits_banner_dismissal_loads_from_preferences() {
    App::test((), |mut app| async move {
        register_user_preferences_for_tests(&mut app);
        app.update(|ctx| {
            ctx.private_user_preferences()
                .write_value(AMBIENT_CREDITS_BANNER_DISMISSED_KEY, "true".to_owned())
                .unwrap();
        });

        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, _ctx| {
            assert!(model.is_ambient_credits_banner_dismissed());
        });
    });
}
#[test]
fn test_has_any_ai_remaining_false_when_no_requests_or_bonus() {
    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // At limit, no bonus credits and no overages.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            assert!(!model.has_any_ai_remaining(ctx));
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_user_bonus_credits() {
    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);

            // User-level bonus credits remaining.
            model.bonus_grants = vec![BonusGrant {
                created_at: Utc::now(),
                cost_cents: 0,
                expiration: Some(Utc::now() + chrono::Duration::days(7)),
                grant_type: BonusGrantType::Any,
                reason: "test user bonus".to_string(),
                user_facing_message: None,
                request_credits_granted: 5,
                request_credits_remaining: 5,
                usage_cents_granted: None,
                usage_cents_remaining: None,
                scope: BonusGrantScope::User,
            }];

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when user bonus credits exist",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_workspace_overages() {
    App::test((), |mut app| async move {
        // Create a workspace with overages enabled and remaining.
        let (_uid, mut workspace) = create_test_workspace();
        workspace.settings.usage_based_pricing_settings.enabled = true;
        workspace
            .settings
            .usage_based_pricing_settings
            .max_monthly_spend_cents = Some(1_000);
        workspace.billing_metadata.ai_overages = Some(AiOverages {
            current_monthly_request_cost_cents: 100,
            current_monthly_requests_used: 100,
            current_period_end: Utc::now() + chrono::Duration::days(7),
        });

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests left and no bonus credits.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected overages to count as remaining AI when standard requests are exhausted",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_workspace_bonus_credits() {
    App::test((), |mut app| async move {
        let (uid, workspace) = create_test_workspace();
        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);

            // Workspace-level bonus credits remaining.
            model.bonus_grants = vec![BonusGrant {
                created_at: Utc::now(),
                cost_cents: 0,
                expiration: Some(Utc::now() + chrono::Duration::days(7)),
                grant_type: BonusGrantType::Any,
                reason: "test workspace bonus".to_string(),
                user_facing_message: None,
                request_credits_granted: 5,
                request_credits_remaining: 5,
                usage_cents_granted: None,
                usage_cents_remaining: None,
                scope: BonusGrantScope::Workspace(uid),
            }];

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when workspace bonus credits exist",
            );
        });
    });
}

#[test]
fn test_total_workspace_and_team_bonus_credits_counts_both_scopes() {
    App::test((), |mut app| async move {
        let (uid, workspace) = create_test_workspace();
        let other_uid = WorkspaceUid::from(crate::server::ids::ServerId::from(2_i64));
        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, _ctx| {
            let make = |scope, remaining| BonusGrant {
                created_at: Utc::now(),
                cost_cents: 0,
                expiration: None,
                grant_type: BonusGrantType::Any,
                reason: "test".to_string(),
                user_facing_message: None,
                request_credits_granted: remaining,
                request_credits_remaining: remaining,
                usage_cents_granted: None,
                usage_cents_remaining: None,
                scope,
            };
            model.bonus_grants = vec![
                make(BonusGrantScope::User, 5),
                make(BonusGrantScope::Team(uid), 7),
                make(BonusGrantScope::Workspace(uid), 11),
                make(BonusGrantScope::Team(other_uid), 13),
            ];

            assert_eq!(
                model.total_workspace_and_team_bonus_credits_remaining(uid),
                18
            );
            assert_eq!(model.total_user_interactive_bonus_credits_remaining(), 5);
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_payg_enabled() {
    App::test((), |mut app| async move {
        // Create a workspace with pay-as-you-go enabled.
        let (_uid, mut workspace) = create_test_workspace();
        workspace.billing_metadata.customer_type = CustomerType::Enterprise;
        workspace
            .billing_metadata
            .tier
            .enterprise_pay_as_you_go_policy = Some(EnterprisePayAsYouGoPolicy { enabled: true });

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining, no bonus credits.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when pay-as-you-go is enabled",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_enterprise_auto_reload() {
    App::test((), |mut app| async move {
        // Create a workspace with enterprise auto-reload enabled.
        let (_uid, mut workspace) = create_test_workspace();
        workspace.billing_metadata.customer_type = CustomerType::Enterprise;
        workspace
            .billing_metadata
            .tier
            .enterprise_credits_auto_reload_policy =
            Some(EnterpriseCreditsAutoReloadPolicy { enabled: true });

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining, no bonus credits.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when enterprise auto-reload is enabled",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_with_enterprise_auto_reload_policy_on_non_enterprise() {
    App::test((), |mut app| async move {
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .enterprise_credits_auto_reload_policy =
            Some(EnterpriseCreditsAutoReloadPolicy { enabled: true });

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false when enterprise auto-reload policy is enabled for a non-enterprise workspace",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_self_serve_auto_reload() {
    App::test((), |mut app| async move {
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .purchase_add_on_credits_policy = Some(standard_purchase_policy());
        enable_auto_reload(&mut workspace);

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);
        set_addon_credits_pricing_info(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when self-serve auto-reload is enabled",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_premium_auto_reload() {
    App::test((), |mut app| async move {
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .purchase_add_on_credits_policy = Some(premium_purchase_policy());
        enable_auto_reload(&mut workspace);

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);
        set_addon_credits_pricing_info(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when premium-plan auto-reload is enabled",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_when_premium_auto_reload_would_exceed_limit() {
    App::test((), |mut app| async move {
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .purchase_add_on_credits_policy = Some(premium_purchase_policy());
        enable_auto_reload(&mut workspace);
        // The reload's list price is $10.00 but the premium price is $11.00;
        // a $10.50 monthly limit only blocks the reload when the premium
        // surcharge is included in the check.
        workspace
            .settings
            .addon_credits_settings
            .max_monthly_spend_cents = Some(1050);

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);
        set_addon_credits_pricing_info(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false when the premium-priced reload would exceed the monthly spend limit",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_true_with_self_serve_auto_reload_and_billing_v2_disabled() {
    App::test((), |mut app| async move {
        let _guard = FeatureFlag::BillingAndUsagePageV2.override_enabled(false);

        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .purchase_add_on_credits_policy = Some(standard_purchase_policy());
        enable_auto_reload(&mut workspace);

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);
        set_addon_credits_pricing_info(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be true when self-serve auto-reload is enabled without Billing and Usage V2",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_with_add_on_credits_policy_when_purchase_would_exceed_limit() {
    App::test((), |mut app| async move {
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .purchase_add_on_credits_policy = Some(standard_purchase_policy());
        enable_auto_reload(&mut workspace);
        workspace
            .settings
            .addon_credits_settings
            .max_monthly_spend_cents = Some(1000);
        workspace.bonus_grants_purchased_this_month.cents_spent = 500;

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);
        set_addon_credits_pricing_info(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false when add-on credit purchase would exceed the monthly spend limit",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_with_workspace_no_pricing_no_overages_no_credits() {
    App::test((), |mut app| async move {
        // Create a workspace with no tier pricing (default).
        let (_uid, workspace) = create_test_workspace();

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining, no bonus credits.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false with no pricing, no overages, no credits",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_both_payg_and_autoreload_disabled() {
    App::test((), |mut app| async move {
        // Create a workspace with policies but payg disabled and auto-reload disabled.
        let (_uid, mut workspace) = create_test_workspace();
        workspace
            .billing_metadata
            .tier
            .enterprise_pay_as_you_go_policy = Some(EnterprisePayAsYouGoPolicy { enabled: false });
        workspace
            .billing_metadata
            .tier
            .enterprise_credits_auto_reload_policy =
            Some(EnterpriseCreditsAutoReloadPolicy { enabled: false });

        add_user_workspaces_with_workspace(&mut app, workspace);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining, no bonus credits.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.bonus_grants.clear();

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false with policies but payg and auto-reload disabled",
            );
        });
    });
}

#[test]
fn test_has_any_ai_remaining_false_with_only_ambient_bonus_credits() {
    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // No standard requests remaining.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);

            // Only ambient-only bonus credits.
            model.bonus_grants = vec![BonusGrant {
                created_at: Utc::now(),
                cost_cents: 0,
                expiration: Some(Utc::now() + chrono::Duration::days(7)),
                grant_type: BonusGrantType::AmbientOnly,
                reason: "ambient trial credits".to_string(),
                user_facing_message: None,
                request_credits_granted: 1000,
                request_credits_remaining: 1000,
                usage_cents_granted: None,
                usage_cents_remaining: None,
                scope: BonusGrantScope::User,
            }];

            assert!(
                !model.has_any_ai_remaining(ctx),
                "expected has_any_ai_remaining to be false when only ambient-only bonus credits exist",
            );
        });
    });
}

#[test]
fn test_server_availability_overrides_locally_derived_state() {
    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            // Local state says AI is available.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 5);
            assert!(model.has_any_ai_remaining(ctx));

            // The server-authoritative decision wins over local state.
            model.apply_server_availability(
                Ok(AICreditAvailability::unavailable(
                    AICreditDenialReason::OutOfCredits,
                )),
                ctx,
            );
            assert!(!model.has_any_ai_remaining(ctx));

            // And in the other direction: local state is exhausted, but the
            // server reports a usable fallback source.
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.apply_server_availability(
                Ok(AICreditAvailability::available_with_source(Some(
                    AICreditSource::BonusGrant,
                ))),
                ctx,
            );
            assert!(model.has_any_ai_remaining(ctx));
        });
    });
}

#[test]
fn test_server_managed_availability_trusted_without_local_keys() {
    App::test((), |mut app| async move {
        // `available` with no credit source now means a server-managed BYO
        // path (team keys/endpoints or enterprise custom LLM) is configured;
        // the server knows this for a fact, so no local key is required.
        app.add_singleton_model(UserWorkspaces::default_mock);
        let request_usage_model = add_request_usage_model(&mut app);

        request_usage_model.update(&mut app, |model, ctx| {
            model.request_limit_info = RequestLimitInfo::new_for_test(10, 10);
            model.apply_server_availability(
                Ok(AICreditAvailability::available_with_source(None)),
                ctx,
            );
            assert!(model.has_any_ai_remaining(ctx));
        });
    });
}
