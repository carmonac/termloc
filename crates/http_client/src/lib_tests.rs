use super::*;

#[test]
fn clients_send_no_automatic_headers_and_external_clients_preserve_signing_headers() {
    let client = Client::new();
    let url = "http://127.0.0.1:9/object?X-Amz-Signature=test%2Bsignature&x-id=PutObject";
    let api_request = client.get(url).build().unwrap().wrapped;
    assert!(api_request.headers().is_empty());

    let external_client = Client::new_external();
    let request = external_client
        .put(url)
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
