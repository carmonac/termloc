//! The headless `warp-tui` front-end's app-side entry point.
//!
//! `warp_tui` boots the real headless Warp app via [`crate::run_tui`]. Once
//! shared initialization is done, [`init`] registers the TUI models and mounts
//! the TUI.
mod mcp;
mod user_info;

pub use mcp::{
    TuiMcpAction, TuiMcpConfigDiagnostic, TuiMcpFileScope, TuiMcpFileSource, TuiMcpInstallRequest,
    TuiMcpManager, TuiMcpManagerEvent, TuiMcpServerId, TuiMcpServerSnapshot, TuiMcpServerSource,
    TuiMcpServerStatus, TuiMcpSnapshot, TuiMcpSyncedTemplateProvenance, TuiMcpTemplateVariable,
    TuiMcpTransport, TuiMcpVariableValue,
};
pub use user_info::{TuiUserInfoManager, TuiUserInfoManagerEvent, TuiUserInfoSnapshot};
use warpui::{AppContext, SingletonEntity};

use crate::TuiMountFn;
use crate::ai::mcp::FileBasedMCPManager;
use crate::tui_onboarding_markers::TuiOnboardingMarkers;

/// Entry point invoked from `run_internal` once the headless app is initialized.
pub(crate) fn init(mount: TuiMountFn, ctx: &mut AppContext) {
    ctx.add_singleton_model(TuiMcpManager::new);
    ctx.add_singleton_model(TuiUserInfoManager::new);
    let onboarding_markers = ctx.add_singleton_model(TuiOnboardingMarkers::new);
    onboarding_markers.update(ctx, |markers, ctx| {
        markers.load_current_account(ctx);
    });
    mount(ctx);
    activate_global_mcp_servers(ctx);
}

fn activate_global_mcp_servers(ctx: &mut AppContext) {
    FileBasedMCPManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.activate_global_warp_servers(ctx);
    });
}
