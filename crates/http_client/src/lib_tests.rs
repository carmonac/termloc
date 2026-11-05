use super::*;

struct TestIapTokenProvider;

impl IapTokenProvider for TestIapTokenProvider {
    fn cached_token(&self) -> Option<String> {
        Some("test-iap-token".into())
    }
}

#[test]
fn clients_send_no_automatic_headers_and_external_clients_preserve_signing_headers() {
    let mut client = Client::new();
    client.set_iap_token_provider(Arc::new(TestIapTokenProvider));
    let url = format!(
        "{}/object?X-Amz-Signature=test%2Bsignature&x-id=PutObject",
        ChannelState::server_root_url()
    );
    let api_request = client.get(&url).build().unwrap().wrapped;
    assert_eq!(api_request.headers().len(), 1);
    assert!(
        api_request
            .headers()
            .contains_key(iap::IAP_PROXY_AUTH_HEADER)
    );

    let external_client = Client::new_external();
    let request = external_client
        .put(&url)
        .header("x-amz-checksum-crc32c", "EjRWeA==")
        .header(AUTHORIZATION, "AWS4-HMAC-SHA256 test-signature")
        .build()
        .unwrap()
        .wrapped;
    assert_eq!(request.url().as_str(), url);
    assert_eq!(request.headers().len(), 2);
    assert_eq!(request.headers()["x-amz-checksum-crc32c"], "EjRWeA==");
    assert_eq!(
        request.headers()[AUTHORIZATION],
        "AWS4-HMAC-SHA256 test-signature"
    );
}
