use std::collections::HashSet;

use super::*;

#[test]
fn every_language_has_exactly_one_preference() {
    let languages: Vec<Language> = LanguagePreference::ALL
        .into_iter()
        .filter_map(LanguagePreference::language)
        .collect();
    assert_eq!(languages, Language::ALL);
}

#[test]
fn command_names_round_trip() {
    let names: HashSet<&str> = LanguagePreference::ALL
        .into_iter()
        .map(LanguagePreference::command_name)
        .collect();
    assert_eq!(names.len(), LanguagePreference::ALL.len());
    for preference in LanguagePreference::ALL {
        assert_eq!(
            LanguagePreference::from_command_name(preference.command_name()),
            Some(preference)
        );
    }
    assert_eq!(
        LanguagePreference::from_command_name(" ES "),
        Some(LanguagePreference::Spanish)
    );
    assert_eq!(LanguagePreference::from_command_name("klingon"), None);
}

#[test]
fn explicit_preferences_resolve_to_their_language() {
    assert_eq!(LanguagePreference::Japanese.resolve(), Language::Japanese);
    assert_eq!(LanguagePreference::English.resolve(), Language::English);
}
