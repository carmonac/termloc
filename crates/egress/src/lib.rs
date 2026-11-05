//! Process-wide policy that limits outbound connections to this machine and a user allowlist.

#[cfg(not(target_family = "wasm"))]
mod guard;

use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock, RwLock};

use http::Uri;

#[cfg(not(target_family = "wasm"))]
pub use guard::build_client;

/// Browsers decide where wasm builds may connect.
#[cfg(target_family = "wasm")]
#[allow(clippy::disallowed_methods)]
pub fn build_client(builder: reqwest::ClientBuilder) -> reqwest::Result<reqwest::Client> {
    builder.build()
}

static POLICY: OnceLock<Policy> = OnceLock::new();
static REPORTED_HOSTS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Returned when the active policy refuses a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    host: String,
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "connection to {} blocked by the egress policy",
            self.host
        )
    }
}

impl std::error::Error for Blocked {}

/// The hosts this process may connect to. Loopback addresses are always included.
#[derive(Debug, Default)]
pub struct Policy {
    allowed_hosts: RwLock<HashSet<String>>,
}

impl Policy {
    pub fn new(allowed_hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed_hosts: RwLock::new(
                allowed_hosts
                    .into_iter()
                    .map(|host| normalize(&host))
                    .filter(|host| !host.is_empty())
                    .collect(),
            ),
        }
    }

    pub fn allows(&self, host: &str) -> bool {
        let host = normalize(host);
        is_loopback(&host)
            || self
                .allowed_hosts
                .read()
                .unwrap_or_else(|err| err.into_inner())
                .contains(&host)
    }

    fn allow(&self, host: &str) {
        self.allowed_hosts
            .write()
            .unwrap_or_else(|err| err.into_inner())
            .insert(normalize(host));
    }
}

/// Starts enforcing `policy` for the rest of the process. Later calls have no effect.
pub fn enforce(policy: Policy) {
    let _ = POLICY.set(policy);
}

/// Adds `host` to the active policy, for a server the user configured.
pub fn allow_host(host: &str) {
    if let Some(policy) = POLICY.get() {
        policy.allow(host);
    }
}

/// Whether a policy is being enforced in this process.
pub fn is_enforced() -> bool {
    POLICY.get().is_some()
}

/// Checks a connection to `uri` against the active policy, if any.
pub fn check_uri(uri: &Uri) -> Result<(), Blocked> {
    check_host(uri.host().unwrap_or_default())
}

fn check_host(host: &str) -> Result<(), Blocked> {
    match POLICY.get() {
        Some(policy) if !policy.allows(host) => {
            report_blocked(host);
            Err(Blocked {
                host: host.to_owned(),
            })
        }
        Some(_) | None => Ok(()),
    }
}

fn report_blocked(host: &str) {
    let mut reported = REPORTED_HOSTS.lock().unwrap_or_else(|err| err.into_inner());
    if reported
        .get_or_insert_with(HashSet::new)
        .insert(host.to_owned())
    {
        log::warn!("Blocked outbound connection to {host}");
    }
}

fn normalize(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
