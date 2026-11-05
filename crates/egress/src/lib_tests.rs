use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use super::*;

const ALLOWED_HOST: &str = "allowed.example";

fn enforce_test_policy() {
    enforce(Policy::new([ALLOWED_HOST.to_owned()]));
}

fn client() -> reqwest::Client {
    enforce_test_policy();
    guard(reqwest::Client::builder()).build().unwrap()
}

fn is_blocked(error: &reqwest::Error) -> bool {
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(error) = source {
        if error.is::<Blocked>() {
            return true;
        }
        source = error.source();
    }
    false
}

/// Serves `response` to one connection and returns the port it listens on.
async fn serve_once(response: String) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).await;
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    port
}

#[test]
fn policy_allows_loopback_and_listed_hosts_only() {
    let policy = Policy::new([" Ollama.LAN. ".to_owned(), String::new()]);

    for host in [
        "localhost",
        "LOCALHOST",
        "127.0.0.1",
        "127.8.9.1",
        "::1",
        "[::1]",
        "::ffff:127.0.0.1",
        "ollama.lan",
    ] {
        assert!(policy.allows(host), "{host} should be allowed");
    }

    for host in [
        "",
        "app.warp.dev",
        "0.0.0.0",
        "192.168.1.10",
        "::ffff:192.0.2.1",
        "foo.localhost",
        "127.0.0.1.example.com",
        "sub.ollama.lan",
    ] {
        assert!(!policy.allows(host), "{host} should be blocked");
    }
}

#[test]
fn check_uri_uses_the_active_policy() {
    enforce_test_policy();

    assert!(check_uri(&"ws://127.0.0.1:9/graphql".parse().unwrap()).is_ok());
    assert!(check_uri(&format!("wss://{ALLOWED_HOST}/").parse().unwrap()).is_ok());
    assert!(check_uri(&"wss://rtc.app.warp.dev/".parse().unwrap()).is_err());
}

#[test]
fn unreadable_connector_request_is_blocked() {
    enforce_test_policy();

    assert!(guard::check_request(&"something else").is_err());
}

#[tokio::test]
async fn request_to_a_blocked_host_never_connects() {
    let error = client()
        .get("http://192.0.2.1:9/")
        .send()
        .await
        .unwrap_err();

    assert!(is_blocked(&error), "unexpected error: {error:?}");
}

#[tokio::test]
async fn request_to_loopback_succeeds() {
    let port = serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_owned()).await;

    let response = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 204);
}

#[tokio::test]
async fn redirect_to_a_blocked_host_is_refused() {
    let port = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1:9/\r\nContent-Length: 0\r\n\r\n"
            .to_owned(),
    )
    .await;

    let error = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap_err();

    assert!(is_blocked(&error), "unexpected error: {error:?}");
}
