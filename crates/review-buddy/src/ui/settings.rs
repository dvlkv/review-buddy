//! Settings window (stub; replaced by the full view).
use crate::app::AppState;
#[derive(Default)]
pub struct SettingsState {}
pub fn settings_window(ctx: &egui::Context, state: &mut AppState, _s: &mut SettingsState) {
    let mut open = state.ui.show_settings;
    egui::Window::new("Settings").open(&mut open).show(ctx, |ui| {
        ui.label("Settings");
    });
    state.ui.show_settings = open;
}
