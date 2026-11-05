use super::*;

fn row(id: &str, is_selectable: bool, is_profile_default: bool) -> TuiModelMenuRow {
    TuiModelMenuRow {
        id: id.into(),
        title: id.to_owned(),
        is_selectable,
        is_profile_default,
        discount_percentage: None,
    }
}

#[test]
fn empty_query_prefers_active_model_and_filtered_query_prefers_best_match() {
    let rows = vec![
        row("auto", true, false),
        row("gpt-4", true, false),
        row("gpt-5", true, false),
    ];

    assert_eq!(
        preferred_selection_index(&rows, &LLMId::from("gpt-4"), true),
        Some(1)
    );
    assert_eq!(
        preferred_selection_index(&rows, &LLMId::from("gpt-4"), false),
        Some(2)
    );
}

#[test]
fn model_selection_skips_disabled_rows() {
    let rows = vec![
        row("auto", true, false),
        row("gpt-5", true, false),
        row("disabled", false, false),
    ];

    assert_eq!(
        preferred_selection_index(&rows, &LLMId::from("disabled"), true),
        Some(1)
    );
    assert_eq!(
        preferred_selection_index(&rows, &LLMId::from("auto"), false),
        Some(1)
    );
}

#[test]
fn snapshot_marks_the_profile_default_model() {
    let default = snapshot_row(&row("auto", true, true));
    assert_eq!(default.state_suffix.as_deref(), Some("(default)"));

    let other = snapshot_row(&row("gpt-5", true, false));
    assert_eq!(other.state_suffix, None);
}
