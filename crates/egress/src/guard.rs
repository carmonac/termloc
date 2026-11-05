use std::fmt::Debug;
use std::task::{Context, Poll};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use http::Uri;
use reqwest::{Client, ClientBuilder};
use tower::{Layer, Service};

use crate::{Blocked, check_host, check_uri, is_enforced};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Builds a client whose connections, redirects included, are subject to the active policy.
#[allow(clippy::disallowed_methods)]
pub fn build_client(builder: ClientBuilder) -> reqwest::Result<Client> {
    // A system proxy could carry traffic for loopback addresses off this machine.
    let builder = if is_enforced() {
        builder.no_proxy()
    } else {
        builder
    };
    builder.connector_layer(GuardLayer).build()
}

#[derive(Clone)]
struct GuardLayer;

impl<S> Layer<S> for GuardLayer {
    type Service = GuardService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GuardService { inner }
    }
}

#[derive(Clone)]
struct GuardService<S> {
    inner: S,
}

impl<S, R> Service<R> for GuardService<S>
where
    S: Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    R: Debug,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<S::Response, BoxError>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        match check_request(&request) {
            Ok(()) => self.inner.call(request).boxed(),
            Err(blocked) => futures::future::ready(Err(blocked.into())).boxed(),
        }
    }
}

/// reqwest's connector request is a sealed wrapper around the destination `Uri`, and its `Debug`
/// output is the only public way to read it. An unexpected format is treated as a blocked host so
/// that a reqwest upgrade cannot silently disable the policy.
pub(crate) fn check_request(request: &impl Debug) -> Result<(), Blocked> {
    let debug = format!("{request:?}");
    match destination(&debug) {
        Some(uri) => check_uri(&uri),
        None => check_host(&debug),
    }
}

fn destination(debug: &str) -> Option<Uri> {
    debug
        .strip_prefix("Unnameable(")?
        .strip_suffix(')')?
        .parse()
        .ok()
}
