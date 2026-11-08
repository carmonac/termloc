use super::{INDEXING_DISABLED_ADMIN_TEXT, codebase_indexing_disabled_admin_text};

#[test]
fn current_team_disabling_indexing_uses_generic_tooltip_text() {
    assert_eq!(
        codebase_indexing_disabled_admin_text(Some("Team A"), true),
        INDEXING_DISABLED_ADMIN_TEXT
    );
}

#[test]
fn other_team_disabling_indexing_names_that_team() {
    assert_eq!(
        codebase_indexing_disabled_admin_text(Some("Team A"), false),
        "Codebase indexing is unavailable because Team A has disabled it."
    );
}
