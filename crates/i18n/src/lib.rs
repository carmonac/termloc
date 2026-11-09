//! Translation catalogs for termloc's user interface.
//!
//! `translations.yaml` maps each English source text to its translation in every other language,
//! so English needs no entries and any missing translation falls back to English.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
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

impl Language {
    pub const ALL: [Language; 9] = [
        Language::English,
        Language::Spanish,
        Language::Portuguese,
        Language::French,
        Language::German,
        Language::Italian,
        Language::Ukrainian,
        Language::Japanese,
        Language::ChineseSimplified,
    ];

    /// The BCP 47 tag of the language.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Spanish => "es",
            Language::Portuguese => "pt",
            Language::French => "fr",
            Language::German => "de",
            Language::Italian => "it",
            Language::Ukrainian => "uk",
            Language::Japanese => "ja",
            Language::ChineseSimplified => "zh-Hans",
        }
    }

    /// The language's name written in that language, as shown in language pickers.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Spanish => "Español",
            Language::Portuguese => "Português",
            Language::French => "Français",
            Language::German => "Deutsch",
            Language::Italian => "Italiano",
            Language::Ukrainian => "Українська",
            Language::Japanese => "日本語",
            Language::ChineseSimplified => "简体中文",
        }
    }

    /// Matches a locale tag such as `es-ES`, `pt_BR.UTF-8` or `zh-Hans-CN`. Chinese only matches
    /// when the tag doesn't ask for Traditional script or a Traditional-script region.
    pub fn from_locale_tag(tag: &str) -> Option<Language> {
        let tag = tag.split(['.', '@']).next().unwrap_or_default();
        let lower = tag.to_ascii_lowercase().replace('_', "-");
        let mut parts = lower.split('-');
        let primary = parts.next()?;
        let language = match primary {
            "en" => Language::English,
            "es" => Language::Spanish,
            "pt" => Language::Portuguese,
            "fr" => Language::French,
            "de" => Language::German,
            "it" => Language::Italian,
            "uk" => Language::Ukrainian,
            "ja" => Language::Japanese,
            "zh" => {
                let traditional = parts.any(|part| matches!(part, "hant" | "tw" | "hk" | "mo"));
                if traditional {
                    return None;
                }
                Language::ChineseSimplified
            }
            _ => return None,
        };
        Some(language)
    }

    fn index(self) -> u8 {
        Language::ALL
            .iter()
            .position(|language| *language == self)
            .unwrap_or_default() as u8
    }
}

/// One catalog per surface, so each surface's translations can be edited independently.
const SOURCES: &[(&str, &str)] = &[
    ("settings", include_str!("../translations/settings.yaml")),
    (
        "settings_ai",
        include_str!("../translations/settings_ai.yaml"),
    ),
    ("tui", include_str!("../translations/tui.yaml")),
    ("agent", include_str!("../translations/agent.yaml")),
    (
        "menus_and_palette",
        include_str!("../translations/menus_and_palette.yaml"),
    ),
    (
        "menus_and_palette_2",
        include_str!("../translations/menus_and_palette_2.yaml"),
    ),
    ("terminal", include_str!("../translations/terminal.yaml")),
    (
        "terminal_input",
        include_str!("../translations/terminal_input.yaml"),
    ),
    ("workspace", include_str!("../translations/workspace.yaml")),
];

#[derive(Clone, Default)]
struct Catalog {
    exact: HashMap<String, &'static str>,
    /// Keyed by lowercased source text, for sources that are re-cased after being written, such
    /// as keybinding descriptions converted to title case.
    case_insensitive: HashMap<String, &'static str>,
}

impl Catalog {
    /// Sources must arrive in sorted order, so that among sources differing only in case the
    /// case-insensitive lookup deterministically prefers the capitalized one.
    fn insert(&mut self, source: String, translation: &'static str) {
        self.case_insensitive
            .entry(source.to_lowercase())
            .or_insert(translation);
        self.exact.insert(source, translation);
    }

    fn get(&self, source: &str) -> Option<&'static str> {
        self.exact
            .get(source)
            .or_else(|| self.case_insensitive.get(&source.to_lowercase()))
            .copied()
    }
}
type Entries = HashMap<String, HashMap<String, String>>;

static CATALOGS: OnceLock<Vec<Catalog>> = OnceLock::new();
static CURRENT: AtomicU8 = AtomicU8::new(0);

fn parse_entries(source: &str) -> Result<Entries, serde_yaml::Error> {
    serde_yaml::from_str(source)
}

fn build_catalogs(entries: Entries) -> Vec<Catalog> {
    let mut catalogs = vec![Catalog::default(); Language::ALL.len()];
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (source, translations) in entries {
        for (code, translation) in translations {
            let Some(language) = Language::ALL.into_iter().find(|l| l.code() == code) else {
                log::warn!("Unknown language {code:?} in the translation of {source:?}");
                continue;
            };
            if !translation.is_empty() {
                catalogs[language.index() as usize]
                    .insert(source.clone(), Box::leak(translation.into_boxed_str()));
            }
        }
    }
    catalogs
}

fn catalog(language: Language) -> &'static Catalog {
    let catalogs = CATALOGS.get_or_init(|| {
        let mut entries = Entries::new();
        for (name, source) in SOURCES {
            match parse_entries(source) {
                Ok(parsed) => entries.extend(parsed),
                Err(err) => log::error!("Failed to parse the {name} translation catalog: {err}"),
            }
        }
        build_catalogs(entries)
    });
    &catalogs[language.index() as usize]
}

/// Sets the language that [`translate`] uses.
pub fn set_language(language: Language) {
    CURRENT.store(language.index(), Ordering::Relaxed);
}

pub fn language() -> Language {
    Language::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

/// The first of the operating system's preferred languages that termloc supports, or English.
pub fn system_language() -> Language {
    sys_locale::get_locales()
        .find_map(|tag| Language::from_locale_tag(&tag))
        .unwrap_or_default()
}

/// Translates English source text into the current language, returning `source` itself when
/// there is no translation.
pub fn translate(source: &str) -> &str {
    translate_in(language(), source)
}

pub fn translate_in(language: Language, source: &str) -> &str {
    if language == Language::English {
        return source;
    }
    catalog(language).get(source).unwrap_or(source)
}

/// Translates source text holding `{name}` placeholders and fills them from `args`.
pub fn translate_with_args(source: &str, args: &[(&str, &dyn Display)]) -> String {
    fill_placeholders(translate(source), args)
}

fn fill_placeholders(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut result = template.to_owned();
    for (name, value) in args {
        result = result.replace(&format!("{{{name}}}"), &value.to_string());
    }
    result
}

/// Translates English source text into the current language.
///
/// `t!("Settings")` returns a string slice with the lifetime of its argument. `t!("Delete {name}?", name = file)` fills the
/// `{name}` placeholder and returns a `String`.
#[macro_export]
macro_rules! t {
    ($source:expr) => {
        $crate::translate($source)
    };
    ($source:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::translate_with_args($source, &[$((stringify!($name), &$value)),+])
    };
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
