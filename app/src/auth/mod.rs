pub mod auth_manager;
mod user_properties;
pub use warp_server_auth::{auth_state, credentials, user, user_uid};

#[cfg(feature = "tui")]
use ::settings::{Setting, SettingsManager};
#[cfg(feature = "tui")]
use ai::index::full_source_code_embedding::manager::CodebaseIndexManager;
pub use auth_manager::AuthManager;
pub use auth_state::AuthStateProvider;
#[cfg(feature = "tui")]
use url::Url;
pub use user_uid::UserUid;
#[cfg(feature = "tui")]
use warp_core::channel::ChannelState;
#[cfg(feature = "tui")]
use warp_core::user_preferences::GetUserPreferences as _;
#[cfg(feature = "tui")]
use warp_errors::report_error;
#[cfg(feature = "tui")]
use warpui::{AppContext, SingletonEntity};

#[cfg(feature = "tui")]
use crate::ai::agent_conversations_model::AgentConversationsModel;
#[cfg(feature = "tui")]
use crate::ai::blocklist::BlocklistAIHistoryModel;
#[cfg(feature = "tui")]
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
#[cfg(feature = "tui")]
use crate::ai::llms::LLMPreferences;
#[cfg(feature = "tui")]
use crate::ai::request_usage_model::AIRequestUsageModel;
#[cfg(feature = "tui")]
use crate::ai_assistant::requests::REQUEST_LIMIT_INFO_CACHE_KEY;
#[cfg(feature = "tui")]
use crate::cloud_object::model::persistence::CloudModel;
#[cfg(feature = "tui")]
use crate::env_vars::manager::EnvVarCollectionManager;
#[cfg(feature = "tui")]
use crate::notebooks::manager::NotebookManager;
#[cfg(feature = "tui")]
use crate::server::cloud_objects::update_manager::UpdateManager;
#[cfg(feature = "tui")]
use crate::server::sync_queue::SyncQueue;
#[cfg(feature = "tui")]
use crate::server::telemetry::TelemetryEvent;
#[cfg(feature = "tui")]
use crate::settings::{
    AISettings, CRASH_REPORTING_ENABLED_DEFAULTS_KEY, CloudPreferencesSettings, PrivacySettings,
    TELEMETRY_ENABLED_DEFAULTS_KEY,
};
#[cfg(feature = "tui")]
use crate::terminal::shared_session::manager::Manager as SharedSessionManager;
#[cfg(feature = "tui")]
use crate::workflows::manager::WorkflowManager;
#[cfg(feature = "tui")]
use crate::workspaces::update_manager::TeamUpdateManager;
#[cfg(feature = "tui")]
use crate::{GlobalResourceHandlesProvider, persistence, send_telemetry_sync_from_app_ctx};

/// Returns the configured Warp web logout URL.
///
/// Keep this derived from the channel's server root so local and non-production
/// builds log out of the same web session they use for authentication.
#[cfg(feature = "tui")]
pub fn web_logout_url() -> String {
    format!(
        "{}/logout",
        ChannelState::server_root_url().trim_end_matches('/')
    )
}

/// Returns the configured Warp web logout URL with a validated browser continuation.
///
/// TUI logout only continues to the same Warp web origin's device page. This
/// keeps the logout endpoint from becoming an open redirect if an unexpected
/// device-authorization response reaches the client.
#[cfg(feature = "tui")]
pub fn web_logout_url_with_continue(continue_url: &str) -> Option<String> {
    let mut logout_url =
        Url::parse(&web_logout_url()).expect("configured Warp web logout URL must be valid");
    let continue_url = Url::parse(continue_url).ok()?;
    let has_required_query = continue_url
        .query_pairs()
        .any(|(key, value)| key == "user_code" && !value.is_empty())
        && continue_url
            .query_pairs()
            .any(|(key, value)| key == "source" && value == "warp-agent-cli");
    if continue_url.origin() != logout_url.origin()
        || continue_url.path() != "/device"
        || !continue_url.username().is_empty()
        || continue_url.password().is_some()
        || continue_url.fragment().is_some()
        || !has_required_query
    {
        return None;
    }
    logout_url
        .query_pairs_mut()
        .append_pair("continue", continue_url.as_str());
    Some(logout_url.into())
}

#[cfg(all(test, feature = "tui"))]
#[path = "mod_tests.rs"]
mod tests;

// Log out the user, clears workspace state, stops running processes, and deletes database.
#[cfg(feature = "tui")]
pub fn log_out(app: &mut AppContext) {
    send_telemetry_sync_from_app_ctx!(TelemetryEvent::LogOut, app);

    CodebaseIndexManager::handle(app).update(app, |index_manager, ctx| {
        index_manager.reset_codebase_indexing(ctx);
    });

    let global_resource_handles = GlobalResourceHandlesProvider::as_ref(app).get();

    // As part of Logout v0, we remove sqlite3 so sessions and cloud objects don't persist between accounts.
    // TODO: Implement per-user scoping of sqlite3.
    persistence::remove(&global_resource_handles.model_event_sender);

    AuthManager::handle(app).update(app, |auth_manager, ctx| {
        auth_manager.log_out(ctx);
    });
    AIRequestUsageModel::handle(app).update(app, |usage_model, ctx| {
        usage_model.reset_server_availability(ctx);
    });
    BlocklistAIHistoryModel::handle(app).update(app, |history_model, _| {
        history_model.reset();
    });
    AgentConversationsModel::handle(app).update(app, |agent_conversations_model, _| {
        agent_conversations_model.reset();
    });
    CloudModel::handle(app).update(app, |cloud_model, _| {
        cloud_model.reset();
    });
    // Clear the sync queue so that we don't try to sync the old user's objects to the new user.
    SyncQueue::handle(app).update(app, |sync_queue, _| {
        sync_queue.clear();
    });

    // Stop the cloud object and workspace metadata polling loops that were started on login.
    UpdateManager::handle(app).update(app, |manager, _| {
        manager.stop_polling_for_updated_objects();
    });
    TeamUpdateManager::handle(app).update(app, |manager, _| {
        manager.stop_polling_for_workspace_metadata_updates();
    });
    remove_cloud_persisted_settings(app);

    let settings_profiles_are_explicit = AISettings::as_ref(app)
        .execution_profiles
        .is_value_explicitly_set();
    AIExecutionProfilesModel::handle(app).update(app, |profiles, _| {
        profiles.reset(settings_profiles_are_explicit);
    });
    LLMPreferences::handle(app).update(app, |preferences, _| {
        preferences.reset_session_overrides();
    });

    NotebookManager::handle(app).update(app, |manager, _| manager.reset());
    EnvVarCollectionManager::handle(app).update(app, |manager, _| manager.reset());
    WorkflowManager::handle(app).update(app, |manager, _| manager.reset());

    // Stop and leave all shared sessions
    SharedSessionManager::handle(app).update(app, |manager, ctx| {
        manager.stop_all_shared_sessions(ctx);
        manager.clear_joined();
    });
}

// Remove the cloud persisted settings from user defaults.
// When a user signs out, we remove cloud persisted settings of their account.
// This is so they do not experience the old settings when they log in with a different account.
// Partial deletion of user defaults is a stopgap for Logout v0. The correct solution is:
#[cfg(feature = "tui")]
fn remove_cloud_persisted_settings(app: &mut AppContext) {
    let is_settings_sync_enabled = *CloudPreferencesSettings::as_ref(app).settings_sync_enabled;
    if is_settings_sync_enabled {
        SettingsManager::handle(app).update(app, |settings_manager, ctx| {
            let errors = settings_manager.clear_cloud_settings_local_state(ctx);
            for e in errors {
                report_error!(
                    e.context("Failed to remove cloud synced setting from user defaults")
                );
            }
        });
    }

    if let Err(e) = app
        .private_user_preferences()
        .remove_value(TELEMETRY_ENABLED_DEFAULTS_KEY)
    {
        report_error!(
            anyhow::Error::new(e)
                .context("Failed to remove Telemetry Enabled Defaults Key from user defaults")
        );
    }

    if let Err(e) = app
        .private_user_preferences()
        .remove_value(CRASH_REPORTING_ENABLED_DEFAULTS_KEY)
    {
        report_error!(
            anyhow::Error::new(e).context(
                "Failed to remove Crash Reporting Enabled Defaults Key from user defaults"
            )
        );
    }

    if let Err(e) = app
        .private_user_preferences()
        .remove_value(REQUEST_LIMIT_INFO_CACHE_KEY)
    {
        report_error!(
            anyhow::Error::new(e)
                .context("Failed to remove Request Limit Defaults Key from user defaults")
        );
    }

    // Reset the Privacy Settings in the login screen to default values.
    PrivacySettings::handle(app).update(app, |privacy_settings, _| {
        privacy_settings.refresh_to_default();
    });
}
