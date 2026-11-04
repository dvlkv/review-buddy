//! PR header (stub).
use crate::pr_store::PRStore;
/// Returns true when the Discuss panel should open.
pub fn pr_header(ui: &mut egui::Ui, store: &mut PRStore) -> bool {
    if let Some(d) = &store.detail {
        ui.heading(&d.title);
    }
    false
}
