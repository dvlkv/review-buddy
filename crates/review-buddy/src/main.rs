fn main() -> eframe::Result {
    eframe::run_simple_native("hello", Default::default(), |ctx, _| {
        egui::CentralPanel::default().show(ctx, |ui| ui.label("hi"));
    })
}
