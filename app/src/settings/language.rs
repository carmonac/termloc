use i18n::Language;
use settings::macros::define_settings_group;
use settings::{Setting as _, SupportedPlatforms, SyncToCloud};
use warpui::{AppContext, SingletonEntity};

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "The language of the user interface.",
    rename_all = "snake_case"
)]
pub enum LanguagePreference {
    #[default]
    System,
    English,
    Spanish,
    Portuguese,
    French,
    German,
    Italian,
    Ukrainian,
    Japanese,
    ChineseSimplified,
}

impl LanguagePreference {
    pub const ALL: [LanguagePreference; 10] = [
        LanguagePreference::System,
        LanguagePreference::English,
        LanguagePreference::Spanish,
        LanguagePreference::Portuguese,
        LanguagePreference::French,
        LanguagePreference::German,
        LanguagePreference::Italian,
        LanguagePreference::Ukrainian,
        LanguagePreference::Japanese,
        LanguagePreference::ChineseSimplified,
    ];

    /// The explicitly chosen language, or `None` to follow the system.
    pub fn language(self) -> Option<Language> {
        match self {
            LanguagePreference::System => None,
            LanguagePreference::English => Some(Language::English),
            LanguagePreference::Spanish => Some(Language::Spanish),
            LanguagePreference::Portuguese => Some(Language::Portuguese),
            LanguagePreference::French => Some(Language::French),
            LanguagePreference::German => Some(Language::German),
            LanguagePreference::Italian => Some(Language::Italian),
            LanguagePreference::Ukrainian => Some(Language::Ukrainian),
            LanguagePreference::Japanese => Some(Language::Japanese),
            LanguagePreference::ChineseSimplified => Some(Language::ChineseSimplified),
        }
    }

    pub fn resolve(self) -> Language {
        self.language().unwrap_or_else(i18n::system_language)
    }

    /// The name shown in language pickers: each language in its own script, so it stays
    /// recognizable whatever the current language is.
    pub fn display_name(self) -> String {
        match self.language() {
            Some(language) => language.native_name().to_owned(),
            None => i18n::t!(
                "System default ({language})",
                language = i18n::system_language().native_name()
            ),
        }
    }

    /// The value written to and parsed from slash-command arguments.
    pub fn command_name(self) -> &'static str {
        match self {
            LanguagePreference::System => "system",
            LanguagePreference::English => "en",
            LanguagePreference::Spanish => "es",
            LanguagePreference::Portuguese => "pt",
            LanguagePreference::French => "fr",
            LanguagePreference::German => "de",
            LanguagePreference::Italian => "it",
            LanguagePreference::Ukrainian => "uk",
            LanguagePreference::Japanese => "ja",
            LanguagePreference::ChineseSimplified => "zh",
        }
    }

    pub fn from_command_name(name: &str) -> Option<LanguagePreference> {
        LanguagePreference::ALL
            .into_iter()
            .find(|preference| preference.command_name().eq_ignore_ascii_case(name.trim()))
    }
}

define_settings_group!(LanguageSettings, settings: [
    language: LanguageSetting {
        type: LanguagePreference,
        default: LanguagePreference::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::ALL,
        private: false,
        toml_path: "appearance.language",
        description: "The language of the user interface. System follows the operating system's preferred languages and falls back to English.",
    },
]);

impl LanguageSettings {
    pub fn preference(&self) -> LanguagePreference {
        *self.language.value()
    }
}

/// Applies the language setting and keeps it applied, redrawing every window when it changes.
pub fn init(ctx: &mut AppContext) {
    apply(ctx);
    ctx.subscribe_to_model(
        &LanguageSettings::handle(ctx),
        |_, event, ctx| match event {
            LanguageSettingsChangedEvent::LanguageSetting { .. } => {
                apply(ctx);
                ctx.invalidate_all_views();
            }
        },
    );
}

fn apply(ctx: &AppContext) {
    let language = if cfg!(feature = "integration_tests") {
        Language::English
    } else {
        LanguageSettings::as_ref(ctx).preference().resolve()
    };
    i18n::set_language(language);
}

#[cfg(test)]
#[path = "language_tests.rs"]
mod tests;
