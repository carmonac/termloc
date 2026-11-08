use std::sync::Arc;

use anyhow::{Result, bail};
use futures::StreamExt as _;
use warp_server_auth::auth_state::AuthState;
use warp_server_auth::credentials::{AuthToken, Credentials};

use crate::auth::{AuthEvent, UserUid};

/// Header carrying the request-local team scope for an operation whose team is inferred
/// from the current window rather than named explicitly in the request body. See
/// `specs/multi-team-api-context/TECH.md`. The server authenticates membership and rejects
/// a header that disagrees with the request body or an existing resource; adding this header
/// is not itself proof of membership.
pub const TEAM_UID_HEADER: &str = "X-Warp-Team-Uid";

/// Owns shared transport, authentication, and authenticated request decoration.
pub struct BaseClient {
    client: Arc<http_client::Client>,
    external_client: Arc<http_client::Client>,
    auth_state: Arc<AuthState>,
    event_sender: async_channel::Sender<AuthEvent>,
    iap_token_provider: Option<Arc<dyn http_client::iap::IapTokenProvider>>,
}

impl BaseClient {
    pub fn new(
        client: Arc<http_client::Client>,
        external_client: Arc<http_client::Client>,
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
        iap_token_provider: Option<Arc<dyn http_client::iap::IapTokenProvider>>,
    ) -> Self {
        Self {
            client,
            external_client,
            auth_state,
            event_sender,
            iap_token_provider,
        }
    }

    /// Returns the shared HTTP client for request construction.
    pub fn http_client(&self) -> &http_client::Client {
        self.client.as_ref()
    }

    /// Returns the pooled external HTTP client.
    pub fn external_http_client(&self) -> &http_client::Client {
        self.external_client.as_ref()
    }

    /// Returns an owned handle to the shared HTTP client for GraphQL operations.
    pub fn owned_http_client(&self) -> Arc<http_client::Client> {
        self.client.clone()
    }

    pub fn anonymous_id(&self) -> String {
        self.auth_state.anonymous_id()
    }

    pub fn user_id(&self) -> Option<UserUid> {
        self.auth_state.user_id()
    }

    /// Returns whether the authenticated principal is a service account.
    pub fn is_service_account(&self) -> bool {
        self.auth_state.is_service_account()
    }

    pub fn access_token_ignoring_validity(&self) -> Option<String> {
        self.auth_state.get_access_token_ignoring_validity()
    }

    pub fn allowed_to_refresh_token(&self) -> bool {
        self.auth_state
            .credentials()
            .is_none_or(|credentials| !credentials.is_externally_managed())
    }

    pub async fn get_or_refresh_access_token(&self) -> Result<AuthToken> {
        if cfg!(feature = "skip_login") {
            bail!("skip_login enabled; failing all authenticated requests");
        }
        let Some(credentials) = self.auth_state.credentials() else {
            bail!("missing authentication credentials");
        };
        match credentials {
            Credentials::Bearer(token) => Ok(AuthToken::Bearer(token)),
            Credentials::Local => bail!("local-only builds have no server to authenticate with"),
            Credentials::Test => Ok(AuthToken::NoAuth),
        }
    }

    /// Returns a sender for asynchronous work that emits auth events without borrowing this client.
    pub fn event_sender(&self) -> async_channel::Sender<AuthEvent> {
        self.event_sender.clone()
    }
    /// Sends an auth event from synchronous client-owned response handling.
    pub fn send_auth_event(
        &self,
        event: AuthEvent,
    ) -> Result<(), async_channel::TrySendError<AuthEvent>> {
        self.event_sender.try_send(event)
    }

    /// Notifies the application when an enabled IAP-backed request receives an IAP challenge.
    pub fn observe_iap_challenge(&self, response: &http_client::Response) -> bool {
        if self.iap_token_provider.is_none()
            || !http_client::iap::is_iap_challenge(response.status(), response.headers())
        {
            return false;
        }
        log::warn!(
            "Received IAP challenge (status {}); notifying IapManager",
            response.status()
        );
        if let Err(error) = self.send_auth_event(AuthEvent::IapChallengeReceived) {
            log::warn!("Failed to enqueue IapChallengeReceived event: {error}");
        }
        true
    }

    /// Wraps an eventsource stream so IAP challenges notify the application without changing the
    /// original stream result or reconnecting it.
    pub fn wrap_eventsource_with_iap_detection(
        &self,
        stream: http_client::EventSourceStream,
    ) -> http_client::EventSourceStream {
        if self.iap_token_provider.is_none() {
            return stream;
        }
        let event_sender = self.event_sender();
        let wrapped = stream.map(move |event| {
            if let Err(reqwest_eventsource::Error::InvalidStatusCode(status, ref response)) = event
                && http_client::iap::is_iap_challenge(status, response.headers())
            {
                log::warn!(
                    "Received IAP challenge on eventsource (status {status}); notifying IapManager"
                );
                if let Err(error) = event_sender.try_send(AuthEvent::IapChallengeReceived) {
                    log::warn!(
                        "Failed to enqueue IapChallengeReceived event from eventsource: {error}"
                    );
                }
            }
            event
        });
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                wrapped.boxed_local()
            } else {
                wrapped.boxed()
            }
        }
    }

    /// Inspects a WebSocket handshake error for an IAP challenge and notifies the application.
    #[cfg(not(target_family = "wasm"))]
    pub fn report_ws_iap_challenge(&self, error: &anyhow::Error) {
        if self.iap_token_provider.is_none() || !crate::iap::ws_connect_is_iap_challenge(error) {
            return;
        }
        log::warn!("Received IAP challenge on websocket handshake; notifying IapManager");
        if let Err(error) = self.send_auth_event(AuthEvent::IapChallengeReceived) {
            log::warn!("Failed to enqueue IapChallengeReceived: {error}");
        }
    }

    #[cfg(target_family = "wasm")]
    pub fn report_ws_iap_challenge(&self, _error: &anyhow::Error) {}

    /// Returns the current IAP proxy authorization header for transports outside the HTTP client.
    pub fn iap_proxy_auth_header(&self) -> Option<(&'static str, String)> {
        self.iap_token_provider
            .as_ref()?
            .cached_token()
            .map(|token| http_client::iap::proxy_auth_header(&token))
    }
}

#[cfg(test)]
#[path = "base_client_tests.rs"]
mod tests;
