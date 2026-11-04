//! File tree column (stub; replaced by the full view).
use crate::pr_store::PRStore;
#[derive(Default)]
pub struct FileTreeState {}
pub fn file_tree(ui: &mut egui::Ui, store: &mut PRStore, _state: &mut FileTreeState) {
    let mut go = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, f) in store.files().iter().enumerate() {
            if ui.link(f.path()).clicked() {
                go = Some(i);
            }
        }
    });
    if let Some(i) = go {
        store.scroll_to_file(i);
    }
}
