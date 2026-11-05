use serde::Deserialize;

/// The process to run for a harness driven over the Agent Client Protocol.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcpLaunchSpec {
    pub program: String,
    pub args: Vec<String>,
}
