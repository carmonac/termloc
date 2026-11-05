use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Output format for agent results.
#[derive(Debug, Copy, Clone, ValueEnum, Eq, PartialEq, Default)]
pub enum OutputFormat {
    /// Output as JSON.
    #[value(name = "json")]
    Json,
    /// Output as newline-delimited JSON.
    #[value(name = "ndjson")]
    Ndjson,
    /// Output as human-readable text.
    #[default]
    #[value(name = "pretty")]
    Pretty,
    /// Output as plain text.
    #[value(name = "text")]
    Text,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.to_possible_value().expect("no values are skipped");
        f.write_str(value.get_name())
    }
}

/// Source-control provider for a repository checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepositoryForge {
    #[serde(rename = "GITHUB")]
    GitHub,
    #[serde(rename = "GITLAB")]
    GitLab,
    #[serde(rename = "AZURE_DEVOPS")]
    AzureDevOps,
}

/// The harness behind an agent conversation.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    /// The built-in local agent (default).
    #[default]
    // Conversations and filters saved by older builds hold "oz".
    #[serde(alias = "oz")]
    Local,
    /// Delegate to the `claude` CLI.
    Claude,
    /// Delegate to the `opencode` CLI.
    OpenCode,
    /// Delegate to the `gemini` CLI.
    Gemini,
    /// Delegate to the `codex` CLI.
    Codex,
    Pi,
    /// A harness this client doesn't recognize, surfaced via deserialization fallbacks.
    #[serde(other)]
    Unknown,
}

impl Harness {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Local => "Local Agent",
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini CLI",
            Self::Codex => "Codex",
            Self::Pi => "Pi",
            Self::Unknown => "Unknown",
        }
    }

    /// Parses a harness config-name string into a [`Harness`] variant. Inverse of
    /// [`Harness::config_name`]. Returns `None` for unrecognized names so callers can distinguish
    /// an unknown harness from a round-tripped [`Harness::Unknown`].
    pub fn from_config_name(name: &str) -> Option<Self> {
        match name {
            // Written by older builds.
            "local" | "oz" => Some(Harness::Local),
            "claude" => Some(Harness::Claude),
            "opencode" => Some(Harness::OpenCode),
            "gemini" => Some(Harness::Gemini),
            "codex" => Some(Harness::Codex),
            "pi" => Some(Harness::Pi),
            "unknown" => Some(Harness::Unknown),
            _ => None,
        }
    }

    /// Canonical config name for this harness. Inverse of [`Harness::from_config_name`].
    pub fn config_name(self) -> &'static str {
        match self {
            Harness::Local => "local",
            Harness::Claude => "claude",
            Harness::OpenCode => "opencode",
            Harness::Gemini => "gemini",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.config_name())
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
