//! Small shared widgets: buttons, pills, avatars, dates, suggestion previews.

use egui::{Color32, CornerRadius, FontId, Response, RichText, Sense, Stroke, Ui, Vec2};
use review_buddy_core::Date;

use crate::theme::Palette;

pub fn pal(ui: &Ui) -> Palette {
    Palette::current(ui.ctx())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    Default,
    Primary,
    Ai,
    Danger,
    Invisible,
}

/// A button in one of the app's styles (Zed's filled / tinted / ghost). Disabled buttons
/// don't respond.
pub fn gh_button(ui: &mut Ui, text: &str, kind: BtnKind, enabled: bool) -> Response {
    gh_button_ex(ui, text, &[], kind, enabled)
}

/// [`gh_button`] sized for the widest of `text` and `variants` (labels that change).
pub fn gh_button_ex(ui: &mut Ui, text: &str, variants: &[&str], kind: BtnKind, enabled: bool) -> Response {
    let look = btn_look(&pal(ui), kind);
    super::icons::button_ex(ui, text, variants, FontId::proportional(13.0), look, 24.0, enabled)
}

/// [`gh_button_ex`] that shows a spinner instead of its label while `busy`.
pub fn gh_button_busy(ui: &mut Ui, text: &str, variants: &[&str], kind: BtnKind, enabled: bool, busy: bool) -> Response {
    let look = btn_look(&pal(ui), kind);
    super::icons::button_full(ui, text, variants, FontId::proportional(13.0), look, 24.0, enabled, busy)
}

pub fn btn_look(p: &Palette, kind: BtnKind) -> super::icons::Look {
    let (fill, stroke, fg) = match kind {
        BtnKind::Default => (p.subtle, p.border, p.text),
        BtnKind::Primary => p.tinted(p.accent),
        BtnKind::Ai => p.tinted(p.ai_accent),
        BtnKind::Danger => p.tinted(p.del_marker),
        BtnKind::Invisible => (Color32::TRANSPARENT, Color32::TRANSPARENT, p.accent),
    };
    super::icons::Look { fill, fg, stroke, native: false, frame: true, radius: CornerRadius::same(p.radius) }
}

/// A rounded label like GitHub's "Pending" / "Outdated" badges.
pub fn pill(ui: &mut Ui, text: &str, color: Color32, filled: bool) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = pal(ui);
    let bg = if filled { color } else { crate::theme::over(color, 0.14, p.canvas) };
    let border = if filled { color } else { crate::theme::over(color, 0.4, p.canvas) };
    ui.painter().rect(rect, CornerRadius::same(p.radius), bg, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, if filled { pal(ui).canvas } else { color });
    resp
}

/// The user's GitHub avatar; a colored circle with their initial until it loads (or offline).
pub fn avatar(ui: &mut Ui, login: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if let Some(tex) = super::avatars::get(ui.ctx(), login) {
        egui::Image::new(&tex).corner_radius(CornerRadius::same((size / 2.0) as u8)).paint_at(ui, rect);
        return resp.on_hover_text(login);
    }
    let h = review_buddy_core::fnv1a_utf16(login);
    let hue = (h % 360) as f32 / 360.0;
    let c: Color32 = egui::ecolor::Hsva::new(hue, 0.45, 0.75, 1.0).into();
    ui.painter().circle_filled(rect.center(), size / 2.0, c);
    let initial = login.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, FontId::proportional(size * 0.55), Color32::WHITE);
    resp
}

/// "3 hours ago", like GitHub.
pub fn relative(d: &Date) -> String {
    let secs = (chrono::Utc::now() - *d).num_seconds();
    let plural = |n: i64, u: &str| format!("{n} {u}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        s if s < 45 => "just now".into(),
        s if s < 3600 => plural((s / 60).max(1), "minute"),
        s if s < 86_400 => plural(s / 3600, "hour"),
        s if s < 30 * 86_400 => plural(s / 86_400, "day"),
        s if s < 365 * 86_400 => plural(s / (30 * 86_400), "month"),
        s => plural(s / (365 * 86_400), "year"),
    }
}

/// A bordered card with optional fill.
pub fn card<R>(ui: &mut Ui, fill: Color32, stroke: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(6))
        .show(ui, add)
        .inner
}

/// GitHub's "Suggested change" box: removed lines, then proposed lines.
pub fn suggestion_preview(ui: &mut Ui, old: &[String], new: &[String]) {
    let p = pal(ui);
    card(ui, p.canvas, p.border, |ui| {
        ui.set_width(ui.available_width());
        egui::Frame::new().fill(p.subtle).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("± Suggested change").strong().size(13.0));
        });
        let mono = FontId::monospace(13.0);
        for (lines, bg, marker, mc) in [(old, p.del_bg, "-", p.del_marker), (new, p.add_bg, "+", p.add_marker)] {
            for l in lines {
                egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(8, 1)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(marker).font(mono.clone()).color(mc));
                        ui.label(RichText::new(l).font(mono.clone()).color(p.text));
                    });
                });
            }
        }
    });
}

/// Indeterminate spinner sized for inline use.
pub fn spinner(ui: &mut Ui) {
    ui.add(egui::Spinner::new().size(12.0));
}
