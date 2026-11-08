use std::sync::Arc;

use anyhow::{Result, bail};
use warp_server_auth::auth_state::AuthState;
use warp_server_auth::credentials::{AuthToken, Credentials};

use crate::auth::{AuthEvent, UserUid};

/// Owns shared transport, authentication, and authenticated request decoration.
pub struct BaseClient {
    client: Arc<http_client::Client>,
    external_client: Arc<http_client::Client>,
    auth_state: Arc<AuthState>,
    event_sender: async_channel::Sender<AuthEvent>,
}

impl BaseClient {
    pub fn new(
        client: Arc<http_client::Client>,
        external_client: Arc<http_client::Client>,
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
    ) -> Self {
        Self {
            client,
            external_client,
            auth_state,
            event_sender,
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
}
