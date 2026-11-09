//! Stateless status projection for the shared read-only menu component.

use i18n::t;
use unicode_width::UnicodeWidthStr;

use crate::read_only_menu::{
    TuiReadOnlyMenu, TuiReadOnlyMenuRow, TuiReadOnlyMenuSection, TuiReadOnlyMenuText,
};
use crate::tui_builder::TuiUiBuilder;

/// Columns between the end of the widest label and the start of its value.
const LABEL_GAP_COLS: usize = 2;

/// Session and account information displayed by the `/status` menu.
pub(super) struct TuiStatusInfo {
    pub version: String,
    pub session: String,
    pub conversation_id: String,
    pub working_directory: String,
    pub org: String,
    pub email: String,
}

fn field_row(
    label: &str,
    value: &str,
    label_cols: usize,
    builder: &TuiUiBuilder,
) -> TuiReadOnlyMenuRow {
    let padding = " ".repeat(label_cols.saturating_sub(label.width()));
    TuiReadOnlyMenuRow::new([TuiReadOnlyMenuText::new([
        (
            format!("{label}{padding}"),
            builder.read_only_menu_label_style(),
        ),
        (value.to_owned(), builder.primary_text_style()),
    ])])
}

/// Builds the dedicated status menu opened by `/status`.
pub(super) fn menu(status_info: TuiStatusInfo, builder: &TuiUiBuilder) -> TuiReadOnlyMenu {
    let fields = [
        (t!("Version"), status_info.version.as_str()),
        (t!("Session"), status_info.session.as_str()),
        (t!("Conversation ID"), status_info.conversation_id.as_str()),
        (
            t!("Working directory"),
            status_info.working_directory.as_str(),
        ),
        (t!("Org"), status_info.org.as_str()),
        (t!("Email"), status_info.email.as_str()),
    ];
    let label_cols = fields
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or_default()
        + LABEL_GAP_COLS;
    let rows = fields
        .into_iter()
        .map(|(label, value)| field_row(label, value, label_cols, builder))
        .collect();
    TuiReadOnlyMenu::new(vec![TuiReadOnlyMenuSection::new(t!("Status"), rows)])
}
