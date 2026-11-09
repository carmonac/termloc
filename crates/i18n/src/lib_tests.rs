use std::collections::BTreeSet;

use super::*;

fn placeholders(text: &str) -> BTreeSet<&str> {
    text.match_indices('{')
        .filter_map(|(start, _)| {
            let rest = &text[start + 1..];
            let end = rest.find('}')?;
            let name = &rest[..end];
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then_some(name)
        })
        .collect()
}

#[test]
fn every_entry_translates_into_every_language_and_keeps_placeholders() {
    let mut entries = Entries::new();
    for (name, source) in SOURCES {
        let parsed = parse_entries(source)
            .unwrap_or_else(|err| panic!("the {name} catalog is invalid: {err}"));
        for (key, translations) in parsed {
            if let Some(previous) = entries.get(&key) {
                assert_eq!(
                    previous, &translations,
                    "{key:?} is translated differently in {name}"
                );
            }
            entries.insert(key, translations);
        }
    }
    let codes: BTreeSet<&str> = Language::ALL
        .into_iter()
        .filter(|language| *language != Language::English)
        .map(Language::code)
        .collect();
    for (source, translations) in &entries {
        let translated: BTreeSet<&str> = translations.keys().map(String::as_str).collect();
        assert_eq!(
            translated, codes,
            "{source:?} doesn't list exactly the supported languages"
        );
        for (code, translation) in translations {
            assert!(
                !translation.is_empty(),
                "{code} translation of {source:?} is empty"
            );
            assert_eq!(
                placeholders(source),
                placeholders(translation),
                "{code} translation of {source:?} changes its placeholders"
            );
        }
    }
}

#[test]
fn missing_translations_fall_back_to_english() {
    let source = "A string no catalog will ever translate";
    for language in Language::ALL {
        assert_eq!(translate_in(language, source), source);
    }
}

#[test]
fn translates_known_strings() {
    assert_eq!(translate_in(Language::Spanish, "Settings"), "Ajustes");
    assert_eq!(translate_in(Language::Japanese, "Settings"), "設定");
    assert_eq!(translate_in(Language::English, "Settings"), "Settings");
}

#[test]
fn matches_recased_sources() {
    assert_eq!(translate_in(Language::Spanish, "SETTINGS"), "Ajustes");
}

#[test]
fn fills_placeholders() {
    assert_eq!(
        fill_placeholders("{count} de {total}", &[("count", &3), ("total", &"7")]),
        "3 de 7"
    );
}

#[test]
fn matches_locale_tags() {
    assert_eq!(Language::from_locale_tag("es-ES"), Some(Language::Spanish));
    assert_eq!(
        Language::from_locale_tag("pt_BR.UTF-8"),
        Some(Language::Portuguese)
    );
    assert_eq!(
        Language::from_locale_tag("zh-Hans-CN"),
        Some(Language::ChineseSimplified)
    );
    assert_eq!(
        Language::from_locale_tag("zh_CN"),
        Some(Language::ChineseSimplified)
    );
    assert_eq!(Language::from_locale_tag("zh-Hant-TW"), None);
    assert_eq!(Language::from_locale_tag("zh-TW"), None);
    assert_eq!(
        Language::from_locale_tag("uk-UA"),
        Some(Language::Ukrainian)
    );
    assert_eq!(Language::from_locale_tag("ko-KR"), None);
}

fn unescape_rust_string(literal: &str) -> String {
    let mut result = String::new();
    let mut chars = literal.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some('"') => result.push('"'),
            Some('\\') => result.push('\\'),
            Some('\'') => result.push('\''),
            Some('\n') => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
            }
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}

/// Every `t!("…")` literal in the workspace must have an entry, so a new string can't silently
/// stay in English.
#[test]
fn every_translated_literal_in_the_workspace_has_an_entry() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pattern = regex::Regex::new(r#"\bt!\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
    let mut missing = BTreeSet::new();
    for root in ["app/src", "crates"] {
        for entry in walkdir::WalkDir::new(workspace.join(root))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
            .filter(|entry| !entry.path().starts_with(workspace.join("crates/i18n")))
        {
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for capture in pattern.captures_iter(&contents) {
                let source = unescape_rust_string(&capture[1]);
                if catalog(Language::Spanish).get(&source).is_none() {
                    missing.insert(format!("{}: {source:?}", entry.path().display()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these strings have no translation:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}
