pub mod iap;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, future};

#[cfg(not(target_family = "wasm"))]
use async_compat::{Compat, CompatExt};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::HeaderValue;
pub use http::header::AUTHORIZATION;
use http::header::HeaderName;
pub use http::{HeaderMap, StatusCode};
use reqwest::IntoUrl;
use reqwest_eventsource::RequestBuilderExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use warp_core::channel::ChannelState;
use warp_errors::report_error;

use crate::iap::{IapTokenProvider, proxy_auth_header};

pub mod headers {
    /// Custom Warp header indicating the version of the Warp app.
    pub const CLIENT_RELEASE_VERSION_HEADER_KEY: &str = "X-Warp-Client-Version";
}

/// A wrapper around a `reqwest::Client` to execute requests. Returns a custom `RequestBuilder` type
/// that ensures any call to the underlying `reqwest::Client` are properly adapted so that they can
/// run outside of a Tokio context.
pub struct Client {
    wrapped: reqwest::Client,
    #[cfg(target_family = "wasm")]
    omit_credentials: bool,

    /// A callback that is executed before every request is sent with a cloned
    /// version of the outbound request.  If for some reason the request cannot be
    /// cloned the function is not called.
    before_request_sent: Option<RequestHookFn>,

    /// A callback that is executed on after each response is received.
    after_response_received: Option<ResponseHookFn>,

    /// If set, provides IAP bearer tokens to attach as `Proxy-Authorization`
    /// headers on outbound requests to the Warp staging server. Wired in by
    /// the app layer on IAP-enabled builds (staging).
    iap_token_provider: Option<Arc<dyn IapTokenProvider>>,
}

/// Type for 'hook' functions to be executed prior to sending a request. A reference to the
/// outbound request object is given as the first argument. The second argument the request's
/// serialized JSON payload, if any.
pub type RequestHookFn = Box<dyn Fn(&reqwest::Request, &Option<String>) + 'static + Send + Sync>;

/// Type for 'hook' functions to be executed after receiving a response. The sole argument is a
/// reference to the inbound response object.
pub type ResponseHookFn = Box<dyn Fn(&reqwest::Response) + 'static + Send + Sync>;

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        // The WASM version of this type has no bound on `Send`, which is not implemented on
        // `wasm_bindgen::JsValue`, which is ultimately used in reqwest_eventsource::Error.
        // Furthermore, `Send` is an unnecessary bound when targeting wasm because the browser is
        // single-threaded (and we don't leverage WebWorkers for async execution in WoW).
        pub type EventSourceStream = futures::stream::LocalBoxStream<
            'static,
            Result<reqwest_eventsource::Event, reqwest_eventsource::Error>,
        >;
    } else {
        pub type EventSourceStream = futures::stream::BoxStream<
            'static,
            Result<reqwest_eventsource::Event, reqwest_eventsource::Error>,
        >;
    }
}

/// A custom request builder that is a wrapper around a `request::RequestBuilder`. Ensures any async
/// call to the underyling `reqwest::RequestBuilder` are properly adapted to run outside of a Tokio
/// context via a call to `compat`.
pub struct RequestBuilder<'a> {
    wrapped: reqwest::RequestBuilder,
    client: &'a Client,

    // The JSON payload of the request, if any, serialized to a pretty-printed String.
    serialized_payload: Option<String>,

    prevent_sleep_reason: Option<&'static str>,
}

pub struct Request {
    wrapped: reqwest::Request,
    serialized_payload: Option<String>,
    prevent_sleep_reason: Option<&'static str>,
}

/// A wrapper around a `reqwest::Response` that ensures any async calls to the underlying `Response`
/// a properly adapted to be run outside of a Tokio context.
pub struct Response(reqwest::Response);

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        #[cfg_attr(target_family = "wasm", expect(unused_mut))]
        let mut builder = reqwest::Client::builder();

        // Set some HTTP/2-related settings that aren't available on wasm.
        #[cfg(not(target_family = "wasm"))]
        {
            builder = builder
                .http2_keep_alive_interval(Duration::from_secs(60))
                // If a pong is not received within 15s, consider the connection dead.
                .http2_keep_alive_timeout(Duration::from_secs(15))
                // Send these even when there aren't active streams, to ensure that we detect
                // dead connections before we attempt to use them.
                .http2_keep_alive_while_idle(true);
        }

        Self::from_client_builder(builder).expect("should not fail to create client")
    }

    /// Creates a pooled client without IAP tokens or browser credentials.
    pub fn new_external() -> Self {
        #[cfg_attr(not(target_family = "wasm"), expect(unused_mut))]
        let mut client = Self::new();
        #[cfg(target_family = "wasm")]
        {
            client.omit_credentials = true;
        }
        client
    }

    #[cfg(feature = "test-util")]
    pub fn new_for_test() -> Self {
        let client_builder = reqwest::ClientBuilder::new()
            // Don't load any SSL/TLS certificates, as doing so can be slow and we should
            // never be making real requests in tests.
            .tls_certs_only([])
            // Disable proxy usage in tests, as loading system proxy configuration can be
            // slow.
            .no_proxy();
        Self::from_client_builder(client_builder).expect("should not fail to create client")
    }

    pub fn from_client_builder(client_builder: reqwest::ClientBuilder) -> reqwest::Result<Self> {
        egress::build_client(client_builder).map(|client| Self {
            wrapped: client,
            #[cfg(target_family = "wasm")]
            omit_credentials: false,
            before_request_sent: None,
            after_response_received: None,
            iap_token_provider: None,
        })
    }

    pub fn set_before_request_fn(&mut self, hook_fn: RequestHookFn) {
        self.before_request_sent = Some(hook_fn);
    }

    pub fn set_after_response_fn(&mut self, hook_fn: ResponseHookFn) {
        self.after_response_received = Some(hook_fn);
    }

    pub fn set_iap_token_provider(&mut self, provider: Arc<dyn IapTokenProvider>) {
        self.iap_token_provider = Some(provider);
    }

    fn builder(
        &self,
        wrapped: reqwest::RequestBuilder,
        iap_token: Option<String>,
    ) -> RequestBuilder<'_> {
        #[cfg(target_family = "wasm")]
        let wrapped = if self.omit_credentials {
            wrapped.fetch_credentials_omit()
        } else {
            wrapped
        };
        let mut builder = RequestBuilder {
            wrapped,
            client: self,
            serialized_payload: None,
            prevent_sleep_reason: None,
        };

        if let Some(token) = iap_token {
            let (name, value) = proxy_auth_header(&token);
            builder = builder.header(name, value);
        }

        builder
    }

    pub fn get<U: IntoUrl + Clone>(&self, url: U) -> RequestBuilder<'_> {
        let iap_token = self.iap_token_for(url.clone());
        self.builder(self.wrapped.get(url), iap_token)
    }

    pub fn post<U: IntoUrl + Clone>(&self, url: U) -> RequestBuilder<'_> {
        let iap_token = self.iap_token_for(url.clone());
        self.builder(self.wrapped.post(url), iap_token)
    }

    pub fn put<U: IntoUrl + Clone>(&self, url: U) -> RequestBuilder<'_> {
        let iap_token = self.iap_token_for(url.clone());
        self.builder(self.wrapped.put(url), iap_token)
    }

    pub fn patch<U: IntoUrl + Clone>(&self, url: U) -> RequestBuilder<'_> {
        let iap_token = self.iap_token_for(url.clone());
        self.builder(self.wrapped.patch(url), iap_token)
    }

    pub fn delete<U: IntoUrl + Clone>(&self, url: U) -> RequestBuilder<'_> {
        let iap_token = self.iap_token_for(url.clone());
        self.builder(self.wrapped.delete(url), iap_token)
    }

    /// Returns the IAP bearer token to attach to a request targeting
    /// `url`, scoped to the Warp server's origin.
    fn iap_token_for<U: IntoUrl>(&self, url: U) -> Option<String> {
        let provider = self.iap_token_provider.as_ref()?;
        let url = url.into_url().ok()?;
        if !is_warp_server_origin(&url) {
            return None;
        }
        provider.cached_token()
    }

    pub async fn execute(&self, request: Request) -> reqwest::Result<Response> {
        self.execute_inner(request).await
    }

    /// Core request execution logic shared by all platforms.
    async fn execute_inner(&self, request: Request) -> reqwest::Result<Response> {
        let Request {
            wrapped: request,
            serialized_payload,
            prevent_sleep_reason,
        } = request;

        if let Some(before_response_send_fn) = &self.before_request_sent {
            before_response_send_fn(&request, &serialized_payload);
        }

        let _guard = prevent_sleep_reason.map(prevent_sleep::prevent_sleep);

        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                let result = self.wrapped.execute(request).await?;
            } else {
                // Explicitly await the future before converting from tokio -> futures. This is because
                // certain calls to tokio (such as tokio::time::sleep) will panic upon creation if they
                // are not in a tokio runtime. Wrapping the call in an async block first makes sure that it
                // is lazily evaluated, ensuring that it is created within a tokio runtime.
                let result = Compat::new(async { self.wrapped.execute(request).await }).await?;
            }
        }

        if let Some(after_response_received_fn) = &self.after_response_received {
            after_response_received_fn(&result);
        }

        Ok(Response(result))
    }
}

fn is_warp_server_origin(url: &reqwest::Url) -> bool {
    [
        ChannelState::server_root_url(),
        ChannelState::rtc_http_url(),
    ]
    .iter()
    .filter_map(|candidate| reqwest::Url::parse(candidate.as_ref()).ok())
    .any(|candidate| candidate.origin() == url.origin())
}

impl<'a> RequestBuilder<'a> {
    pub fn build(self) -> reqwest::Result<Request> {
        self.build_split().1
    }

    pub fn build_split(self) -> (&'a Client, reqwest::Result<Request>) {
        let request = self.wrapped.build().map(|request| Request {
            wrapped: request,
            serialized_payload: self.serialized_payload,
            prevent_sleep_reason: self.prevent_sleep_reason,
        });
        (self.client, request)
    }

    pub async fn send(self) -> reqwest::Result<Response> {
        let (client, request) = self.build_split();
        client.execute(request?).await
    }

    pub fn json<T: Serialize + ?Sized>(self, json: &T) -> RequestBuilder<'a> {
        let serialized_payload =
            match serde_json::to_string_pretty(json).map_err(anyhow::Error::from) {
                Ok(payload) => Some(payload),
                Err(err) => {
                    report_error!(err.context("Failed to serialize JSON request payload."));
                    None
                }
            };
        Self {
            wrapped: self.wrapped.json(json),
            serialized_payload,
            ..self
        }
    }

    pub fn proto<T: prost::Message>(self, proto: &T) -> RequestBuilder<'a> {
        let bytes = proto.encode_to_vec();
        let serialized = String::from_utf8(bytes.clone());

        Self {
            wrapped: self
                .wrapped
                .header(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-protobuf"),
                )
                .body(bytes),
            serialized_payload: serialized.ok(),
            ..self
        }
    }

    /// Sends the request to the endpoint, which is assumed to be a streaming server-sent-events
    /// endpoint, and returns a corresponding `EventSource`.
    pub fn eventsource(self) -> EventSourceStream {
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                let mut stream = self
                    .wrapped
                    .eventsource()
                    .expect("Request type for SSE endpoint must be cloneable.");

                let stream = stream! {
                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(event) => {
                                yield Ok(event);
                            }
                            Err(err) => {
                                yield Err(err);

                                // Close the stream if an error occurs.
                                stream.close();
                            }
                        }
                    }
                };
            } else {
                let mut stream = self
                    .wrapped
                    .eventsource()
                    .expect("Request type for SSE endpoint must be cloneable.");

                let stream = stream! {
                    // Wrap the stream with async-compat since reqwest requires Tokio.
                    while let Some(event) = stream.next().compat().await {
                        match event {
                            Ok(event) => {
                                yield Ok(event);
                            }
                            Err(err) => {
                                yield Err(err);

                                // Close the stream if an error occurs.
                                stream.close();
                            }
                        }
                    }
                };
            }
        }
        let stream = stream.take_while(|event| {
            if let Err(reqwest_eventsource::Error::StreamEnded) = event {
                return future::ready(false);
            }
            future::ready(true)
        });

        // Wrap the stream in one that holds onto a prevent_sleep guard, if one is required here.
        let stream = prevent_sleep::Stream::wrap(
            stream,
            self.prevent_sleep_reason.map(prevent_sleep::prevent_sleep),
        );

        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                stream.boxed_local()
            } else {
                stream.boxed()
            }
        }
    }

    pub fn basic_auth<U, P>(self, username: U, password: Option<P>) -> RequestBuilder<'a>
    where
        U: fmt::Display,
        P: fmt::Display,
    {
        Self {
            wrapped: self.wrapped.basic_auth(username, password),
            ..self
        }
    }

    pub fn bearer_auth<T>(self, token: T) -> RequestBuilder<'a>
    where
        T: fmt::Display,
    {
        Self {
            wrapped: self.wrapped.bearer_auth(token),
            ..self
        }
    }

    // The `timeout` argument is unused on wasm.
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    pub fn timeout(self, timeout: Duration) -> RequestBuilder<'a> {
        cfg_if::cfg_if! {
            // reqwest provides no ability to configure a request timeout
            // on wasm, so make this a no-op (it's the best we can do).
            if #[cfg(target_family = "wasm")] {
                self
            } else {
                Self {
                    wrapped: self.wrapped.timeout(timeout),
                    ..self
                }
            }
        }
    }

    pub fn header<K, V>(self, key: K, value: V) -> RequestBuilder<'a>
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        Self {
            wrapped: self.wrapped.header(key, value),
            ..self
        }
    }

    pub fn body<T: Into<reqwest::Body>>(self, body: T) -> RequestBuilder<'a> {
        Self {
            wrapped: self.wrapped.body(body),
            ..self
        }
    }

    pub fn form<T: Serialize + ?Sized>(self, form: &T) -> RequestBuilder<'a> {
        let serialized_payload =
            match serde_urlencoded::to_string(form).map_err(anyhow::Error::from) {
                Ok(payload) => Some(payload),
                Err(err) => {
                    report_error!(err.context("Failed to serialize url-encoded form payload"));
                    None
                }
            };
        Self {
            wrapped: self.wrapped.form(form),
            serialized_payload,
            ..self
        }
    }

    /// Attach a `multipart/form-data` body.
    /// Not available on wasm because reqwest's multipart builder API is native-only.
    #[cfg(not(target_family = "wasm"))]
    pub fn multipart(self, form: reqwest::multipart::Form) -> RequestBuilder<'a> {
        Self {
            wrapped: self.wrapped.multipart(form),
            ..self
        }
    }

    /// Prevents the system from sleeping due to idle while this request is in progress.
    ///
    /// The provided reason will be used in user-visible logging, so make sure it is
    /// descriptive and reasonably formatted (e.g. "Agent mode request in-progress").
    pub fn prevent_sleep(self, reason: &'static str) -> RequestBuilder<'a> {
        Self {
            prevent_sleep_reason: Some(reason),
            ..self
        }
    }
}

/// An error returned from `Response::error_for_status` that includes response metadata.
/// This allows callers to inspect headers (like X-Warp-Error-Code) and the response body when
/// handling errors.
#[derive(Debug)]
pub struct ResponseError {
    pub source: reqwest::Error,
    pub headers: Box<HeaderMap>,
    pub body: Option<String>,
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(f)
    }
}

impl std::error::Error for ResponseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl Response {
    pub async fn text(self) -> reqwest::Result<String> {
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                self.0.text().await
            } else {
                Compat::new(async { self.0.text().compat().await }).await
            }
        }
    }

    pub fn status(&self) -> StatusCode {
        self.0.status()
    }

    pub async fn json<T: DeserializeOwned>(self) -> reqwest::Result<T> {
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                self.0.json().await
            } else {
                Compat::new(async { self.0.json().compat().await }).await
            }
        }
    }

    /// Checks the response status and returns an error if it's not successful.
    /// Unlike `reqwest::Response::error_for_status`, this returns a `ResponseError`
    /// that includes the response headers, allowing callers to inspect them.
    pub fn error_for_status(self) -> Result<Self, ResponseError> {
        let headers = self.0.headers().clone();
        match self.0.error_for_status() {
            Ok(response) => Ok(Self(response)),
            Err(source) => Err(ResponseError {
                source,
                headers: Box::new(headers),
                body: None,
            }),
        }
    }

    /// Checks the response status and returns an error if it's not successful.
    /// Unlike `error_for_status`, this also reads and preserves the response body on errors.
    pub async fn error_for_status_with_body(self) -> Result<Self, ResponseError> {
        let headers = self.0.headers().clone();
        match self.0.error_for_status_ref() {
            Ok(_) => Ok(self),
            Err(source) => {
                let body = self.text().await.ok();
                Err(ResponseError {
                    source,
                    headers: Box::new(headers),
                    body,
                })
            }
        }
    }

    /// Returns a reference to the underlying response if the status is successful,
    /// otherwise returns an error with headers preserved.
    pub fn error_for_status_ref(&self) -> Result<&reqwest::Response, ResponseError> {
        let headers = self.0.headers().clone();
        match self.0.error_for_status_ref() {
            Ok(response) => Ok(response),
            Err(source) => Err(ResponseError {
                source,
                headers: Box::new(headers),
                body: None,
            }),
        }
    }

    pub async fn bytes(self) -> reqwest::Result<Bytes> {
        self.0.bytes().await
    }

    pub fn bytes_stream(self) -> impl Stream<Item = reqwest::Result<Bytes>> {
        self.0.bytes_stream()
    }

    pub fn headers(&self) -> &http::HeaderMap {
        self.0.headers()
    }

    pub fn url(&self) -> &reqwest::Url {
        self.0.url()
    }
}

/// Adapter to use our HTTP client wrapper with [`oauth2`]. This is modeled on the [`reqwest`]
/// implementation of [`oauth2::AsyncHttpClient`].
impl<'c> oauth2::AsyncHttpClient<'c> for Client {
    type Error = oauth2::HttpClientError<reqwest::Error>;

    #[cfg(target_arch = "wasm32")]
    type Future = Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse, Self::Error>> + 'c>>;
    #[cfg(not(target_arch = "wasm32"))]
    type Future =
        Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse, Self::Error>> + Send + 'c>>;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let uri = request.uri().to_string();
            let iap_token = self.iap_token_for(uri);
            let builder = reqwest::RequestBuilder::from_parts(
                self.wrapped.clone(),
                request.try_into().map_err(Box::new)?,
            );

            let response = self
                .builder(builder, iap_token)
                .send()
                .await
                .map_err(Box::new)?;

            let mut builder = ::http::Response::builder().status(response.status());

            #[cfg(not(target_arch = "wasm32"))]
            {
                builder = builder.version(response.0.version());
            }

            for (name, value) in response.0.headers().iter() {
                builder = builder.header(name, value);
            }

            let response_body = response.bytes().await.map_err(Box::new)?.to_vec();
            builder
                .body(response_body)
                .map_err(oauth2::HttpClientError::Http)
        })
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    #[test]
    fn server_and_rtc_origins_match() {
        // Derive the expected origins from `ChannelState` so the assertion holds
        // regardless of which channel config the test build resolves to.
        let server = reqwest::Url::parse(ChannelState::server_root_url().as_ref()).unwrap();
        assert!(is_warp_server_origin(&server.join("/graphql/v2").unwrap()));

        let rtc = reqwest::Url::parse(ChannelState::rtc_http_url().as_ref()).unwrap();
        assert!(is_warp_server_origin(
            &rtc.join("/api/v1/agent/events/stream").unwrap()
        ));
    }

    #[test]
    fn third_party_origin_does_not_match() {
        let url = reqwest::Url::parse("https://evil.example.com/graphql/v2").unwrap();
        assert!(!is_warp_server_origin(&url));
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "lib_tests.rs"]
mod tests;
