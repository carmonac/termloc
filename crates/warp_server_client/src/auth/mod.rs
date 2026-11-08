use std::fmt;

pub use user_uid::{TEST_USER_EMAIL, TEST_USER_UID, UserUid};
pub use warp_server_auth::user_uid;

/// Header key used to associate unauthenticated requests with an experiment identity.
pub const EXPERIMENT_ID_HEADER: &str = "X-Warp-Experiment-Id";

/// Authentication and authenticated-transport conditions observed by shared client code.
#[derive(Clone)]
pub enum AuthEvent {
    /// A staging API call was blocked, which may indicate a firewall misconfiguration.
    StagingAccessBlocked,
    /// The user's account has been disabled.
    UserAccountDisabled,
}

impl fmt::Debug for AuthEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StagingAccessBlocked => f.write_str("StagingAccessBlocked"),
            Self::UserAccountDisabled => f.write_str("UserAccountDisabled"),
        }
    }
}
