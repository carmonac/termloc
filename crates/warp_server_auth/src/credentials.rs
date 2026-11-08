//! Representation of user credentials.

/// Represents the different ways a user can be authenticated.
#[derive(Clone, Debug)]
pub enum Credentials {
    /// Request-scoped or externally managed bearer token.
    Bearer(String),
    /// The built-in user of local-only builds, which has no server to authenticate with.
    Local,
    /// Test credentials used in unit tests, integration tests, and skip_login builds.
    Test,
}

impl Credentials {
    /// Returns the short-lived token to use in HTTP requests to the server.
    pub fn bearer_token(&self) -> AuthToken {
        match self {
            Credentials::Bearer(token) => AuthToken::Bearer(token.clone()),
            Credentials::Local => AuthToken::NoAuth,
            Credentials::Test => AuthToken::NoAuth,
        }
    }

    /// Returns whether these credentials are externally managed and should not trigger local token
    /// refresh or reauth flows.
    pub fn is_externally_managed(&self) -> bool {
        match self {
            Credentials::Bearer(_) => true,
            Credentials::Local => false,
            Credentials::Test => false,
        }
    }
}

/// Represents different types of authentication tokens.
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// Request-scoped or externally managed bearer token.
    Bearer(String),
    /// No authentication token available.
    NoAuth,
}

impl AuthToken {
    /// Returns the token string to use in an Authorization header, or `None` if there is no
    /// header-based auth.
    pub fn as_bearer_token(&self) -> Option<&str> {
        match self {
            AuthToken::Bearer(token) => Some(token),
            AuthToken::NoAuth => None,
        }
    }

    /// Returns the bearer token as an owned string, or `None` if auth is not header-based.
    pub fn bearer_token(&self) -> Option<String> {
        match self {
            AuthToken::Bearer(token) => Some(token.clone()),
            AuthToken::NoAuth => None,
        }
    }
}
