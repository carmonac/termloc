//! Shared display metadata for [`Harness`] variants.
//!
//! Any UI surface that shows a harness to the user — the harness selector
//! dropdown, the conversation details sidebar, etc. — should source its label,
//! icon, and brand color from here so the two surfaces cannot drift.

use i18n::t;
use pathfinder_color::ColorU;
use warp_cli::agent::Harness;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{Fill as WarpThemeFill, WarpTheme};

use crate::ai::agent::conversation::AIAgentHarness;
use crate::ai::blocklist::CLAUDE_ORANGE;
use crate::terminal::cli_agent::{GEMINI_BLUE, OPENAI_COLOR, OPENCODE_COLOR};
use crate::ui_components::icons::Icon;

/// User-visible display name for a [`Harness`].
pub fn display_name(harness: Harness) -> &'static str {
    match harness {
        Harness::Local => t!("Local Agent"),
        Harness::Claude => "Claude Code",
        Harness::OpenCode => "OpenCode",
        Harness::Gemini => "Gemini CLI",
        Harness::Pi => "Pi",
        Harness::Codex => "Codex",
        Harness::Unknown => t!("Unknown"),
    }
}

/// Leading icon for a [`Harness`].
pub fn icon_for(harness: Harness) -> Icon {
    match harness {
        Harness::Local => Icon::Agent,
        Harness::Claude => Icon::ClaudeLogo,
        Harness::OpenCode => Icon::OpenCodeLogo,
        Harness::Gemini => Icon::GeminiLogo,
        Harness::Codex => Icon::OpenAILogo,
        Harness::Pi => Icon::Agent,
        Harness::Unknown => Icon::HelpCircle,
    }
}

/// Circle background fill for a [`Harness`] icon rendered in a branded circle.
/// Matches the treatment used in the vertical-tabs sidebar.
pub fn circle_background(harness: Harness, theme: &WarpTheme) -> WarpThemeFill {
    match harness {
        Harness::Local => theme.background(),
        Harness::Claude => WarpThemeFill::Solid(CLAUDE_ORANGE),
        Harness::Codex => WarpThemeFill::Solid(OPENAI_COLOR),
        Harness::Gemini => WarpThemeFill::Solid(GEMINI_BLUE),
        Harness::OpenCode => WarpThemeFill::Solid(OPENCODE_COLOR),
        Harness::Pi | Harness::Unknown => internal_colors::fg_overlay_2(theme),
    }
}

/// Icon fill color when rendered on the branded circle background.
pub fn icon_fill_on_circle(harness: Harness, theme: &WarpTheme) -> WarpThemeFill {
    match harness {
        Harness::Local => theme.main_text_color(theme.background()),
        Harness::Claude | Harness::Codex | Harness::Gemini | Harness::OpenCode => {
            WarpThemeFill::Solid(ColorU::white())
        }
        Harness::Pi | Harness::Unknown => {
            theme.main_text_color(internal_colors::fg_overlay_2(theme))
        }
    }
}

/// Map [`AIAgentHarness`] (from `ServerAIConversationMetadata`) to the
/// canonical [`Harness`].
impl From<AIAgentHarness> for Harness {
    fn from(harness: AIAgentHarness) -> Self {
        match harness {
            AIAgentHarness::Local => Harness::Local,
            AIAgentHarness::ClaudeCode => Harness::Claude,
            AIAgentHarness::Gemini => Harness::Gemini,
            AIAgentHarness::Pi => Harness::Pi,
            AIAgentHarness::Codex => Harness::Codex,
            AIAgentHarness::Unknown => Harness::Unknown,
        }
    }
}

impl PartialEq<Harness> for AIAgentHarness {
    fn eq(&self, other: &Harness) -> bool {
        Harness::from(*self) == *other
    }
}
