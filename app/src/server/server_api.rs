pub mod ai;
pub mod object;

use std::ops::Deref;
use std::sync::Arc;

use ai::AIClient;
use object::ObjectClient;
use warp_core::context_flag::ContextFlag;
use warp_errors::{AnyhowErrorExt, ErrorExt, register_error};
use warp_server_client::auth::AuthEvent;
use warp_server_client::base_client::BaseClient;
use warp_server_client::network_logging::NetworkLogModel;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::ai::predict::generate_ai_input_suggestions::GenerateAIInputSuggestionsRequest;
use crate::ai::predict::generate_am_query_suggestions::GenerateAMQuerySuggestionsRequest;
use crate::ai::predict::predict_am_queries::{PredictAMQueriesRequest, PredictAMQueriesResponse};
use crate::ai::predict::{generate_ai_input_suggestions, generate_am_query_suggestions};
use crate::auth::auth_state::AuthState;
use crate::server::cloud_objects::local_object_client::LocalObjectClient;

impl Deref for ServerApi {
    type Target = BaseClient;

    fn deref(&self) -> &Self::Target {
        &self.base_client
    }
}

/// Wrapper for deserialization errors. This covers both:
/// * Using `serde` directly
/// * Using `reqwest` decoding utilities
#[derive(thiserror::Error, Debug)]
pub enum DeserializationError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Transport(reqwest::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum AIApiError {
    #[error("Internal error occurred at transport layer.")]
    Transport(#[source] reqwest::Error),

    #[error("Failed to deserialize API response.")]
    Deserialization(#[source] DeserializationError),

    #[error("No context found on context search.")]
    NoContextFound,

    #[error("Failed with status code {0}: {1}")]
    ErrorStatus(http::StatusCode, String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),

    #[error("Got error when streaming {stream_type}: {source:#}")]
    Stream {
        stream_type: &'static str,
        #[source]
        source: anyhow::Error,
    },

    /// Synthesized client-side when a response stream ends without a stream-finished
    /// event: the server always sends one, but the transport can truncate the response
    /// between chunks, surfacing as a clean EOF.
    #[error("Response stream ended unexpectedly before completion.")]
    UnexpectedEof,
}

impl From<http_client::ResponseError> for AIApiError {
    fn from(err: http_client::ResponseError) -> Self {
        Self::from_transport_error(err.source)
    }
}

impl From<reqwest::Error> for AIApiError {
    fn from(err: reqwest::Error) -> Self {
        Self::from_transport_error(err)
    }
}

impl From<serde_json::Error> for AIApiError {
    fn from(err: serde_json::Error) -> Self {
        AIApiError::Deserialization(err.into())
    }
}

impl AIApiError {
    /// Converts a transport-level reqwest error (no HTTP response) to an AIApiError.
    fn from_transport_error(err: reqwest::Error) -> Self {
        // Unfortunately, `reqwest` reports some non-decoding errors as decoding errors (e.g.
        // unexpected disconnects or timeouts while deserializing a response body). Since we
        // render deserialization and transport errors differently, we try to detect those cases
        // here.
        if err.is_timeout() {
            return AIApiError::Transport(err);
        }
        if err.is_decode() {
            #[cfg(not(target_family = "wasm"))]
            {
                use std::error::Error as _;
                let mut source = err.source();
                while let Some(underlying) = source {
                    if underlying.is::<hyper::Error>() {
                        return AIApiError::Transport(err);
                    }

                    source = underlying.source();
                }
            }

            return AIApiError::Deserialization(DeserializationError::Transport(err));
        }

        AIApiError::Transport(err)
    }

    /// Whether the error is worth an automatic recovery attempt — a fresh request may
    /// succeed. Gates both retry (pre-actions) and resume (post-actions).
    pub fn is_recoverable(&self) -> bool {
        // Don't recover from client errors, except timeouts and rate limits.
        fn is_recoverable_status(status: http::StatusCode) -> bool {
            !status.is_client_error()
                || status == http::StatusCode::REQUEST_TIMEOUT
                || status == http::StatusCode::TOO_MANY_REQUESTS
        }

        match self {
            AIApiError::ErrorStatus(status, _) => is_recoverable_status(*status),
            AIApiError::Transport(e) => {
                if let Some(status) = e.status() {
                    return is_recoverable_status(status);
                }
                true
            }
            // By default, attempt recovery on error.
            _ => true,
        }
    }
}

impl ErrorExt for AIApiError {
    fn is_actionable(&self) -> bool {
        match self {
            AIApiError::Deserialization(error) => match error {
                DeserializationError::Json(_) => true,
                DeserializationError::Transport(error) => error.is_actionable(),
            },
            AIApiError::Transport(error) => error.is_actionable(),
            AIApiError::Other(error) => error.is_actionable(),
            AIApiError::Stream { source, .. } => source.is_actionable(),
            AIApiError::ErrorStatus(_, _) => self.is_recoverable(),
            AIApiError::UnexpectedEof => true,
            AIApiError::NoContextFound => false,
        }
    }
}
register_error!(AIApiError);

#[derive(thiserror::Error, Debug)]
pub enum TranscribeError {
    #[error("Termloc is currently overloaded. Please try again later.")]
    ServerOverloaded,

    #[error("Internal error occurred at transport layer.")]
    Transport(#[source] reqwest::Error),

    #[error("Failed with status code {0}")]
    ErrorStatus(http::StatusCode),

    #[error("Failed to deserialize JSON.")]
    Deserialization(#[source] DeserializationError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ErrorExt for TranscribeError {
    fn is_actionable(&self) -> bool {
        match self {
            TranscribeError::Transport(error) => error.is_actionable(),
            TranscribeError::ErrorStatus(status) => {
                !status.is_server_error() && *status != http::StatusCode::TOO_MANY_REQUESTS
            }
            TranscribeError::Other(error) => error.is_actionable(),
            TranscribeError::Deserialization(error) => match error {
                DeserializationError::Json(_) => true,
                DeserializationError::Transport(error) => error.is_actionable(),
            },
            TranscribeError::ServerOverloaded => false,
        }
    }
}
register_error!(TranscribeError);

/// An API wrapper struct with methods to requests to warp-server.
///
/// Prefer NOT adding new methods directly on this struct; instead, add to one of the existing
/// client trait objects, or create your own. This helps keep `ServerApi` from being overloaded
/// with disparate types of calls, and allows you to mock methods in tests.
pub struct ServerApi {
    base_client: Arc<BaseClient>,
}

#[cfg(not(target_family = "wasm"))]
async fn local_input_suggestions(
    request: &GenerateAIInputSuggestionsRequest,
) -> Result<generate_ai_input_suggestions::GenerateAIInputSuggestionsResponseV2, AIApiError> {
    let context = local_agent::oneshot::NextCommandContext {
        recent_blocks: request.context_messages.clone(),
        history: request.history_context.clone(),
        system: request.system_context.clone(),
        rejected: request.rejected_suggestions.clone(),
        prefix: request.prefix.clone(),
    };
    let client = crate::ai::local::one_shot()
        .await
        .map_err(AIApiError::Other)?;
    let command = client
        .next_command(&context)
        .await
        .map_err(AIApiError::Other)?;
    Ok(
        generate_ai_input_suggestions::GenerateAIInputSuggestionsResponseV2 {
            most_likely_action: command.clone().unwrap_or_default(),
            commands: command.into_iter().collect(),
            ai_queries: Vec::new(),
        },
    )
}

impl ServerApi {
    fn new(
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
        ctx: &mut ModelContext<ServerApiProvider>,
    ) -> Self {
        let mut client = http_client::Client::new();
        let mut external_client = http_client::Client::new_external();
        if ContextFlag::NetworkLogConsole.is_enabled() {
            NetworkLogModel::handle(ctx).update(ctx, |model, model_ctx| {
                model.install_on_clients([&mut client, &mut external_client], model_ctx);
            });
        }
        Self::new_with_parts(
            Arc::new(client),
            Arc::new(external_client),
            auth_state,
            event_sender,
        )
    }

    fn new_with_parts(
        client: Arc<http_client::Client>,
        external_client: Arc<http_client::Client>,
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
    ) -> Self {
        let base_client = Arc::new(BaseClient::new(
            client,
            external_client,
            auth_state,
            event_sender,
        ));

        Self { base_client }
    }

    #[cfg(any(test, all(feature = "tui", feature = "test-util")))]
    fn new_for_test() -> Self {
        let (tx, _) = async_channel::unbounded();
        let auth_state = Arc::new(AuthState::new_for_test());
        let client = Arc::new(http_client::Client::new_for_test());

        Self::new_with_parts(
            client,
            Arc::new(http_client::Client::new_external()),
            auth_state,
            tx,
        )
    }

    #[cfg(all(test, feature = "skip_login"))]
    fn new_for_test_with_bearer_token(
        bearer_token: Option<String>,
        event_sender: async_channel::Sender<AuthEvent>,
    ) -> Self {
        let auth_state = Arc::new(AuthState::new_logged_out_for_test());
        if let Some(bearer_token) = bearer_token {
            auth_state.set_remote_server_bearer_token(bearer_token);
        }
        Self::new_with_parts(
            Arc::new(http_client::Client::new_for_test()),
            Arc::new(http_client::Client::new_external()),
            auth_state,
            event_sender,
        )
    }

    /// Predicts the next action from past context.
    pub async fn generate_ai_input_suggestions(
        &self,
        request: &GenerateAIInputSuggestionsRequest,
    ) -> Result<generate_ai_input_suggestions::GenerateAIInputSuggestionsResponseV2, AIApiError>
    {
        local_input_suggestions(request).await
    }

    /// Predicted agent queries aren't generated locally, so there are none.
    pub async fn generate_am_query_suggestions(
        &self,
        _request: &GenerateAMQuerySuggestionsRequest,
    ) -> Result<generate_am_query_suggestions::GenerateAMQuerySuggestionsResponse, AIApiError> {
        Ok(Default::default())
    }

    /// Predicted agent queries aren't generated locally, so there are none.
    pub async fn predict_am_queries(
        &self,
        _request: &PredictAMQueriesRequest,
    ) -> Result<PredictAMQueriesResponse, AIApiError> {
        Ok(PredictAMQueriesResponse {
            suggestion: String::new(),
        })
    }
}

/// A singleton entity that provides access to the global [`ServerApi`] instance,
/// or any of its implemented trait objects.
pub struct ServerApiProvider {
    server_api: Arc<ServerApi>,
}

impl ServerApiProvider {
    /// Constructs a new ServerApiProvider.
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    pub fn new(auth_state: Arc<AuthState>, ctx: &mut ModelContext<Self>) -> Self {
        let (event_sender, event_receiver) = async_channel::bounded(10);

        let server_api = ServerApi::new(auth_state.clone(), event_sender, ctx);

        ctx.spawn_stream_local(
            event_receiver,
            move |_, event, ctx| {
                match event {
                    AuthEvent::UserAccountDisabled => {
                        // We dispatch a global action here because the log out code requires
                        // `server_api`, causing a circular model reference panic when it calls
                        // `ServerApiProvider` to get access.
                        // TODO: We should remove this pattern where `ServerApiProvider` responds
                        // to events; it's prone to these sorts of circular reference issues.
                        ctx.dispatch_global_action("app:log_out", ());
                    }
                    // Re-emit the event for subscribers.
                    // TODO: we probably want a different type for the event emitted to subscribers
                    // from the one that's used for the async channel.
                    _ => ctx.emit(event),
                }
            },
            |_, _| {},
        );
        Self {
            server_api: Arc::new(server_api),
        }
    }

    /// Constructs a new SeverApiProvider for tests.
    #[cfg(any(test, all(feature = "tui", feature = "test-util")))]
    pub fn new_for_test() -> Self {
        Self {
            server_api: Arc::new(ServerApi::new_for_test()),
        }
    }

    /// Returns a handle to the underlying [`ServerApi`] object.
    /// Prefer retrieving a specific trait object related to the methods you're calling.
    pub fn get(&self) -> Arc<ServerApi> {
        self.server_api.clone()
    }

    pub fn get_ai_client(&self) -> Arc<dyn AIClient> {
        self.server_api.clone()
    }

    pub fn get_cloud_objects_client(&self) -> Arc<dyn ObjectClient> {
        Arc::new(LocalObjectClient)
    }

    /// Returns the shared HTTP client. This client is wired into network logging
    /// and includes standard Warp request headers.
    pub fn get_http_client(&self) -> Arc<http_client::Client> {
        self.server_api.owned_http_client()
    }
}

impl Entity for ServerApiProvider {
    type Event = AuthEvent;
}

impl SingletonEntity for ServerApiProvider {}
