//! Inbox sidebar (stub; replaced by the full view).
use crate::app::AppState;
pub fn sidebar(ui: &mut egui::Ui, state: &mut AppState) {
    let mut open = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for s in &state.inbox.sections {
            ui.heading(&s.title);
            for pr in &s.items {
                if ui.link(&pr.title).clicked() {
                    open = Some(pr.pr_ref.clone());
                }
            }
        }
    });
    if let Some(r) = open {
        state.open(r);
    }
}
