//! GitHub avatars: downloaded once per login on a background thread, cached on disk
//! (~/Library/Caches/ReviewBuddy/avatars) and in memory as textures. Until one arrives —
//! or when offline — callers draw the initial instead.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use egui::{Context, TextureHandle};

enum Entry {
    Loading,
    Ready(TextureHandle),
    Failed,
}

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static C: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pixels to request: enough for a 28 pt avatar on a Retina screen.
const PX: u32 = 96;
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

fn disk_path(login: &str) -> Option<PathBuf> {
    let safe: String = login.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dirs_cache().map(|d| d.join(format!("{safe}.img")))
}

fn dirs_cache() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/ReviewBuddy/avatars"))
}

fn enabled() -> bool {
    !cfg!(test) && std::env::var_os("REVIEW_BUDDY_NO_AVATARS").is_none()
}

/// The avatar texture for `login`, starting a download the first time it's asked for.
pub fn get(ctx: &Context, login: &str) -> Option<TextureHandle> {
    if login.is_empty() || !enabled() {
        return None;
    }
    let mut map = cache().lock().unwrap();
    match map.get(login) {
        Some(Entry::Ready(t)) => return Some(t.clone()),
        Some(_) => return None,
        None => {}
    }
    map.insert(login.to_string(), Entry::Loading);
    drop(map);
    let (ctx, login) = (ctx.clone(), login.to_string());
    std::thread::spawn(move || {
        let entry = match load(&login) {
            Some(img) => Entry::Ready(ctx.load_texture(format!("avatar:{login}"), img, egui::TextureOptions::LINEAR)),
            None => Entry::Failed,
        };
        cache().lock().unwrap().insert(login, entry);
        ctx.request_repaint();
    });
    None
}

fn load(login: &str) -> Option<egui::ColorImage> {
    let path = disk_path(login);
    let fresh = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < MAX_AGE)
        .unwrap_or(false);
    if fresh {
        if let Some(img) = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| decode(&b)) {
            return Some(img);
        }
    }
    match download(login) {
        Some(bytes) => {
            let img = decode(&bytes)?;
            if let Some(p) = &path {
                let _ = std::fs::create_dir_all(p.parent().unwrap());
                let _ = std::fs::write(p, &bytes);
            }
            Some(img)
        }
        // Offline: a stale copy beats the initial.
        None => path.and_then(|p| std::fs::read(p).ok()).and_then(|b| decode(&b)),
    }
}

fn download(login: &str) -> Option<Vec<u8>> {
    // Bots ("dependabot[bot]") have their avatar under the app's name.
    let name = login.strip_suffix("[bot]").unwrap_or(login);
    let url = format!("https://github.com/{name}.png?size={PX}");
    let resp = ureq::get(&url).timeout(Duration::from_secs(15)).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(5 << 20).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()))
}
