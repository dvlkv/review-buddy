//! Review Buddy: a native GitHub pull request reviewer with Claude built in.

mod app;
mod demo;
mod discuss;
mod file_tree;
mod pr_store;
mod pr_store_ai;
mod rt;
mod services;
mod theme;
mod ui;

#[cfg(test)]
mod tests;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Review Buddy")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("Review Buddy", options, Box::new(move |cc| Ok(Box::new(ui::ReviewBuddyApp::new(cc, args)))))
}
