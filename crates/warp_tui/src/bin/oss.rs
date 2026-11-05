//! OSS-channel `warp-tui` binary and `default-run` target.
//!
//! This is what bare `cargo run -p warp_tui` builds (mirrors `app/src/bin/oss.rs`). It is a
//! console application (no GUI window,
//! no app bundle), so unlike the GUI binaries it sets no `windows_subsystem`
//! attribute and embeds no `Info.plist`.

use anyhow::Result;
use egress::Policy;
use warp_core::AppId;
use warp_core::channel::{Channel, ChannelConfig, ChannelState, OzConfig, WarpServerConfig};

fn main() -> Result<()> {
    egress::enforce(Policy::default());

    let mut state = ChannelState::new(
        Channel::Oss,
        ChannelConfig {
            app_id: AppId::new("dev", "termloc", "Termloc"),
            logfile_name: "termloc-tui.log".into(),
            server_config: WarpServerConfig::disabled(),
            oz_config: OzConfig::disabled(),
            mcp_static_config: None,
        },
    );
    if cfg!(debug_assertions) {
        state = state.with_additional_features(warp_core::features::DEBUG_FLAGS);
    }
    ChannelState::set(state);

    warp_tui::run()
}
