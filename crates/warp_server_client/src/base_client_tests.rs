use std::sync::Arc;

use warp_server_auth::auth_state::AuthState;

use super::BaseClient;

struct StaticIapTokenProvider;

impl http_client::iap::IapTokenProvider for StaticIapTokenProvider {
    fn cached_token(&self) -> Option<String> {
        Some("iap-token".to_string())
    }
}

#[test]
fn iap_proxy_auth_header_uses_configured_provider() {
    let (event_sender, _) = async_channel::unbounded();
    let client = BaseClient::new(
        Arc::new(http_client::Client::new()),
        Arc::new(http_client::Client::new_external()),
        Arc::new(AuthState::new_for_test()),
        event_sender,
        Some(Arc::new(StaticIapTokenProvider)),
    );

    assert_eq!(
        client.iap_proxy_auth_header(),
        Some((
            http_client::iap::IAP_PROXY_AUTH_HEADER,
            "Bearer iap-token".to_string()
        ))
    );
}
