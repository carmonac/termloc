//! Domain types for the server-authoritative AI credit availability decision
//! (`User.aiCreditAvailability`). The server evaluates the same credit
//! waterfall used to authorize AI requests, so these values are the source of
//! truth for whether the user can start an interactive AI request.

/// Stable, client-safe reason the server reports when no inference access
/// exists. `None` is reported when the user is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AICreditDenialReason {
    None,
    OutOfCredits,
    Delinquent,
    EnterpriseTeamSpendLimitHit,
    EnterprisePerUserSpendLimitHit,
    EnterprisePerUnassignedUserSpendLimitHit,
    EnterpriseWorkspaceSpendLimitHit,
    /// A reason from a newer server that this client version doesn't know.
    /// Treated as a generic denial for presentation purposes.
    Unknown,
}

/// The credit source the server selected when inference access exists.
/// Capability-only access (e.g. BYO API key) has no credit source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AICreditSource {
    BaseLimit,
    BonusGrant,
    Payg,
    Overage,
    AmbientBonusGrant,
    /// A source from a newer server that this client version doesn't know.
    Unknown,
}

/// The server-authoritative answer to "can this user start an interactive AI
/// request right now".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AICreditAvailability {
    pub available: bool,
    pub denial_reason: AICreditDenialReason,
    pub credit_source: Option<AICreditSource>,
}

impl AICreditAvailability {
    pub fn available_with_source(credit_source: Option<AICreditSource>) -> Self {
        Self {
            available: true,
            denial_reason: AICreditDenialReason::None,
            credit_source,
        }
    }

    pub fn unavailable(denial_reason: AICreditDenialReason) -> Self {
        Self {
            available: false,
            denial_reason,
            credit_source: None,
        }
    }
}
