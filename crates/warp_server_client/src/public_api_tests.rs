use std::sync::Arc;

use futures::executor::block_on;
use warp_core::channel::ChannelState;
use warp_errors::AnyhowErrorExt as _;
use warp_server_auth::auth_state::AuthState;

use super::{HttpStatusError, PresignedDownloadResponse};
use crate::auth::AuthEvent;
use crate::base_client::BaseClient;
struct EmptyIapTokenProvider;

#[test]
fn presigned_download_keeps_api_context_off_storage() {
    let mut storage = mockito::Server::new();
    let query = "X-Amz-Signature=test%2Bsignature&key=a%2Fb&key=c+%20d";
    let url = format!("{}/snapshot?{query}", storage.url());
    let mut storage_request = storage
        .mock("GET", "/snapshot")
        .match_query(mockito::Matcher::Exact(query.into()))
        .with_status(200)
        .with_body("snapshot bytes");
    for header in [
        "x-api-source",
        "x-warp-client-id",
        "x-warp-client-version",
        "x-warp-os-category",
        "x-warp-os-name",
        "x-warp-os-version",
        "x-warp-os-linux-kernel-version",
        "x-warp-traceparent",
        "authorization",
        "proxy-authorization",
        "cookie",
    ] {
        storage_request = storage_request.match_header(header, mockito::Matcher::Missing);
    }
    let storage_request = storage_request.create();
    let api_request = {
        let mut server = ChannelState::mock_server();
        server
            .mock(
                "GET",
                "/api/v1/agent/conversations/test-download/block-snapshot",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "redirect".into(),
                "false".into(),
            ))
            .match_header("authorization", "Bearer bearer-token")
            .match_header("x-warp-os-category", mockito::Matcher::Any)
            .match_header("x-warp-os-name", mockito::Matcher::Any)
            .match_header("cookie", "session=test-default-cookie")
            .match_header("proxy-authorization", "Bearer test-default-iap")
            .match_header("x-test-client-default", "api-only")
            .with_status(200)
            .with_body(serde_json::json!({"url": url}).to_string())
            .create()
    };
    let auth_state = AuthState::new_logged_out_for_test();
    auth_state.set_remote_server_bearer_token("bearer-token".into());
    let mut defaults = http::HeaderMap::new();
    for (name, value) in [
        ("authorization", "Bearer test-default-auth"),
        ("proxy-authorization", "Bearer test-default-iap"),
        ("cookie", "session=test-default-cookie"),
        ("x-warp-client-id", "test-default-client"),
        ("x-warp-client-version", "test-default-version"),
        ("x-warp-os-category", "test-default-os"),
        ("x-warp-os-name", "test-default-os-name"),
        ("x-api-source", "test-default-source"),
        ("x-test-client-default", "api-only"),
    ] {
        defaults.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            http::HeaderValue::from_static(value),
        );
    }
    let api_http_client = http_client::Client::from_client_builder(
        reqwest::Client::builder().default_headers(defaults),
    )
    .unwrap();
    let (event_sender, _) = async_channel::unbounded();
    let client = BaseClient::new(
        Arc::new(api_http_client),
        Arc::new(http_client::Client::new_external()),
        Arc::new(auth_state),
        event_sender,
        None,
    );
    let bytes = block_on(async {
        let download = client
            .get_public_api::<PresignedDownloadResponse>(
                "agent/conversations/test-download/block-snapshot?redirect=false",
            )
            .await
            .unwrap();
        client.download_presigned(&download.url).await.unwrap()
    });
    assert_eq!(bytes.as_ref(), b"snapshot bytes");
    api_request.assert();
    storage_request.assert();
}

impl http_client::iap::IapTokenProvider for EmptyIapTokenProvider {
    fn cached_token(&self) -> Option<String> {
        None
    }
}

fn base_client(observe_iap_challenges: bool) -> (BaseClient, async_channel::Receiver<AuthEvent>) {
    base_client_with_auth(AuthState::new_for_test(), observe_iap_challenges)
}

fn base_client_with_auth(
    auth_state: AuthState,
    observe_iap_challenges: bool,
) -> (BaseClient, async_channel::Receiver<AuthEvent>) {
    let (event_sender, event_receiver) = async_channel::unbounded();
    let client = BaseClient::new(
        Arc::new(http_client::Client::new()),
        Arc::new(http_client::Client::new_external()),
        Arc::new(auth_state),
        event_sender,
        observe_iap_challenges.then(|| {
            Arc::new(EmptyIapTokenProvider) as Arc<dyn http_client::iap::IapTokenProvider>
        }),
    );
    (client, event_receiver)
}

#[test]
fn public_api_get_deserializes_successful_response() {
    let _request = {
        let mut server = ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/test/success")
            .with_status(200)
            .with_body(r#"{"value":"success"}"#)
            .create()
    };
    let (base_client, _) = base_client(false);

    let response =
        block_on(base_client.get_public_api::<serde_json::Value>("test/success")).unwrap();

    assert_eq!(response, serde_json::json!({ "value": "success" }));
}

#[test]
fn public_api_get_sends_bearer_auth() {
    let _request = {
        let mut server = ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/test/bearer-auth")
            .match_header("authorization", "Bearer bearer-token")
            .with_status(200)
            .with_body(r#"{"value":"success"}"#)
            .create()
    };
    let auth_state = AuthState::new_logged_out_for_test();
    auth_state.set_remote_server_bearer_token("bearer-token".to_string());
    let (base_client, _) = base_client_with_auth(auth_state, false);

    block_on(base_client.get_public_api::<serde_json::Value>("test/bearer-auth")).unwrap();
}

#[test]
fn ordinary_public_api_failure_preserves_shared_status_error() {
    let _request = {
        let mut server = ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/test/failure")
            .with_status(500)
            .with_body(r#"{"error":"request failed"}"#)
            .create()
    };
    let (base_client, event_receiver) = base_client(false);

    let error =
        block_on(base_client.get_public_api::<serde_json::Value>("test/failure")).unwrap_err();

    assert!(error.to_string().contains("request failed"));
    assert!(
        error
            .chain()
            .any(|cause| cause.downcast_ref::<HttpStatusError>().is_some())
    );
    assert!(event_receiver.try_recv().is_err());
}

#[test]
fn iap_challenge_failure_emits_no_event_when_observation_is_disabled() {
    let _request = {
        let mut server = ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/agent/identities")
            .with_status(401)
            .with_header(http_client::iap::IAP_GENERATED_RESPONSE_HEADER, "true")
            .with_body(r#"{"error":"IAP challenge"}"#)
            .create()
    };
    let (base_client, event_receiver) = base_client(false);

    block_on(base_client.get_public_api::<serde_json::Value>("agent/identities")).unwrap_err();

    assert!(event_receiver.try_recv().is_err());
}

#[test]
fn shared_status_error_actionability_ignores_retryable_client_failures() {
    let error = anyhow::Error::new(HttpStatusError::new(429, "retry later".to_string()))
        .context("Public API request failed");

    assert!(!error.is_actionable());
}
