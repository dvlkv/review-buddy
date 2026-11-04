//! Discuss side panel (stub; replaced by the full view).
use crate::pr_store::PRStore;
#[derive(Default)]
pub struct DiscussPanelState {
    pub last_active: Option<String>,
}
pub fn discuss_panel(ui: &mut egui::Ui, store: &mut PRStore, _state: &mut DiscussPanelState) {
    ui.label(format!("{} discussions", store.discuss_threads.len()));
}
