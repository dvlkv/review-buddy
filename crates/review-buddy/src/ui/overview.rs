//! Summary screen (stub; replaced by the full view).
use crate::pr_store::PRStore;
#[derive(Default)]
pub struct OverviewState {}
pub fn overview(ui: &mut egui::Ui, store: &mut PRStore, _state: &mut OverviewState) {
    if ui.button("Back to files").clicked() {
        store.show_overview = false;
    }
}
