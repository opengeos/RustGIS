//! Browser file-picker extension filters, shared with `ui-egui` so desktop and browser filters
//! cannot drift.

use rustgis_ui_egui::{ITEM_EXTS, PROJECT_EXTS};

/// Returns the static extension filter list for a named picker `purpose`, when it uses one.
pub(crate) fn filter_extensions(purpose: &str) -> Option<&'static [&'static str]> {
    match purpose {
        "project" => Some(PROJECT_EXTS),
        "item" => Some(ITEM_EXTS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser picker re-exports `ui-egui`'s canonical project and item extension slices.
    #[test]
    fn browser_picker_extensions_match_ui_egui_lists() {
        assert_eq!(filter_extensions("project"), Some(rustgis_ui_egui::PROJECT_EXTS));
        assert_eq!(filter_extensions("item"), Some(rustgis_ui_egui::ITEM_EXTS));
        assert_eq!(filter_extensions("unknown"), None);
    }
}
