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
    install_crash_log();
    let args: Vec<String> = std::env::args().collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Review Buddy")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("Review Buddy", options, Box::new(move |cc| Ok(Box::new(ui::ReviewBuddyApp::new(cc, args)))))
}

/// Appends every panic (any thread) with a backtrace to
/// `~/Library/Logs/ReviewBuddy/crash.log`. A panic on the main thread ends the app, so it
/// also shows an alert instead of the window just vanishing.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>").to_string();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".into());
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let report = format!(
            "=== panic at unix {when} (v{}) thread '{name}'\n{msg}\n  at {at}\n{}\n\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::Path::new(&home).join("Library/Logs/ReviewBuddy");
        let path = dir.join("crash.log");
        if std::fs::create_dir_all(&dir).is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(report.as_bytes());
            }
        }
        if name == "main" && !cfg!(test) {
            let text = format!("Review Buddy crashed: {msg}\n\nDetails were saved to {}", path.display());
            let script = format!("display alert \"Review Buddy crashed\" message {:?} as critical", text);
            let _ = std::process::Command::new("osascript").arg("-e").arg(script).status();
        }
    }));
}
