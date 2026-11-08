use warp_core::server_timestamp::ServerTimestamp;

#[derive(Clone, Copy, Debug)]
pub enum AddonCreditAutoReloadStatus {
    Failed,
    Succeeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonCreditsOption {
    pub credits: i32,
    pub price_usd_cents: i32,
    pub usage_cents: Option<i32>,
}

impl AddonCreditsOption {
    /// Returns the purchase price in cents after applying a plan surcharge expressed in basis
    /// points (1000 bps = +10%), rounding the surcharge up to the next cent.
    pub fn price_usd_cents_with_premium(&self, premium_bps: i32) -> i32 {
        if premium_bps <= 0 {
            return self.price_usd_cents;
        }
        let price = self.price_usd_cents as i64;
        let surcharge = (price * premium_bps as i64 + 9_999) / 10_000;
        (price + surcharge) as i32
    }
}

#[derive(Debug, Clone)]
pub struct ServiceAgreement {
    pub addon_credit_auto_reload_status: Option<AddonCreditAutoReloadStatus>,
    pub current_period_end: ServerTimestamp,
    pub status: ServiceAgreementStatus,
    pub stripe_subscription_id: Option<String>,
    pub type_: ServiceAgreementType,
    pub sunsetted_to_build_ts: Option<ServerTimestamp>,
}

#[derive(Clone, Debug)]
pub enum ServiceAgreementStatus {
    Active,
    Canceled,
    PastDue,
    Unpaid,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceAgreementType {
    Enterprise,
    Legacy,
    ProTrial,
    Prosumer,
    SelfServe,
    TeamTrial,
    Turbo,
    Business,
    Lightspeed,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiCreditsUsageAndCostSubjectType {
    Team,
    User,
    ServiceAccount,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiCreditsUsageAndCostType {
    BaseLimit,
    BonusGrant,
    Payg,
    AmbientBonusGrant,
    Aggregate,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiCreditsUsageBucket {
    Ai,
    Compute,
    Platform,
    SuggestedCodeDiffs,
    Voice,
    Aggregate,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiCreditsUsageSource {
    Local,
    Cloud,
    Aggregate,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BonusGrantType {
    AmbientOnly,
    Any,
}
