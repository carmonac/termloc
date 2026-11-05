use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::AppId;

const DISABLED_SERVER_URL: &str = "http://127.0.0.1:9";

#[derive(Debug, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// The application ID for this channel.
    pub app_id: AppId,

    /// The name of the file to which logs should be written.
    pub logfile_name: Cow<'static, str>,

    /// Configuration for talking to Warp's servers.
    pub server_config: WarpServerConfig,
    /// Configuration for autoupdate functionality.
    /// Configuration for statically-bundled MCP OAuth credentials.
    pub mcp_static_config: Option<McpStaticConfig>,
}

/// Configuration for GCP Identity-Aware Proxy authentication, present only on staging builds.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IapConfig {
    /// The IAP OAuth2 client ID used as the audience for identity tokens.
    pub audiences: Cow<'static, str>,
    /// The service account email to impersonate when acquiring IAP credentials.
    pub service_account_email: Cow<'static, str>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WarpServerConfig {
    /// The root URL for the standard server pool.
    pub server_root_url: Cow<'static, str>,
    /// The pre-warmed sandbox control origin, defaulting to the server root URL.
    #[serde(default)]
    /// The URL for the RTC server, which serves real-time updates for Warp Drive objects.
    pub rtc_server_url: Cow<'static, str>,
    /// The URL for the session sharing server, or [`None`] if session sharing is not
    /// supported.
    pub session_sharing_server_url: Option<Cow<'static, str>>,
    /// The API key to use when making requests to Firebase Authentication endpoints.
    pub firebase_auth_api_key: Cow<'static, str>,
    /// Configuration for GCP Identity-Aware Proxy authentication, present only on
    /// staging builds. [`None`] on production builds.
    #[serde(default)]
    pub iap_config: Option<IapConfig>,
}

impl WarpServerConfig {
    /// Points every server URL at a closed loopback port, for builds that never talk to Warp.
    pub fn disabled() -> Self {
        Self {
            server_root_url: DISABLED_SERVER_URL.into(),
            rtc_server_url: "ws://127.0.0.1:9/graphql/v2".into(),
            session_sharing_server_url: None,
            firebase_auth_api_key: "".into(),
            iap_config: None,
        }
    }
}

/// Configuration for statically-bundled MCP OAuth credentials.
///
/// These are credentials for OAuth providers where dynamic client registration
/// is not supported and we instead ship pre-registered client IDs and secrets.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpStaticConfig {
    /// Per-provider OAuth credentials.
    pub providers: Vec<McpOAuthProviderConfig>,
}

/// A single OAuth provider's credentials for MCP authentication.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpOAuthProviderConfig {
    /// The issuer URL of the OAuth provider (e.g. `https://github.com/login/oauth`).
    pub issuer: Cow<'static, str>,
    /// The OAuth client ID registered for this channel.
    pub client_id: Cow<'static, str>,
    /// The OAuth client secret registered for this channel.
    pub client_secret: Cow<'static, str>,
    /// A separately registered native client that permits loopback redirects.
    /// This must never be inferred from the custom-scheme client.
    #[serde(default)]
    pub loopback_client: Option<McpOAuthLoopbackClientConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpOAuthLoopbackClientConfig {
    pub client_id: Cow<'static, str>,
    /// Native OAuth clients should normally be public, but retain optional
    /// secret support for providers that require one.
    #[serde(default)]
    pub client_secret: Option<Cow<'static, str>>,
}
