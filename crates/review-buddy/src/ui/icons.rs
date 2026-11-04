//! Vector icons drawn with the painter, plus buttons and labels that put an icon next to
//! text. Unicode symbols come from fallback fonts with their own metrics and sit off the
//! text's center line; painted icons are centered on it exactly.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};
use std::f32::consts::{FRAC_PI_2, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Menu,
    Gear,
    Sidebar,
    SidebarLeft,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    ChevronLeft,
    Refresh,
    Link,
    Sparkle,
    Comment,
    Check,
    Close,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Pencil,
    Stop,
    Dots,
    Description,
}

/// A leading symbol + space, or a trailing space + symbol, that maps to an icon.
const LEADING: &[(&str, Icon)] = &[
    ("☰ ", Icon::Description),
    ("✦ ", Icon::Sparkle),
    ("💬 ", Icon::Comment),
    ("↻ ", Icon::Refresh),
    ("⟳ ", Icon::Refresh),
    ("🔗 ", Icon::Link),
    ("◨ ", Icon::Sidebar),
    ("✓ ", Icon::Check),
    ("← ", Icon::ArrowLeft),
    ("→ ", Icon::ArrowRight),
    ("↑ ", Icon::ArrowUp),
    ("⬆ ", Icon::ArrowUp),
    ("■ ", Icon::Stop),
    ("▴ ", Icon::ChevronUp),
    ("✎ ", Icon::Pencil),
];
const TRAILING: &[(&str, Icon)] = &[(" ▾", Icon::ChevronDown)];
const ALONE: &[(&str, Icon)] = &[
    ("☰", Icon::Menu),
    ("⚙", Icon::Gear),
    ("↻", Icon::Refresh),
    ("⟳", Icon::Refresh),
    ("✦", Icon::Sparkle),
    ("⋯", Icon::Dots),
    ("◨", Icon::Sidebar),
    ("‹", Icon::ChevronLeft),
    ("▾", Icon::ChevronDown),
    ("▸", Icon::ChevronRight),
    ("✕", Icon::Close),
    ("✎", Icon::Pencil),
];

/// Splits "✦ Discuss" into (Sparkle, "Discuss", None) and "Review ▾" into (None, "Review", ChevronDown).
pub fn split(text: &str) -> (Option<Icon>, &str, Option<Icon>) {
    if let Some(&(_, i)) = ALONE.iter().find(|(g, _)| *g == text) {
        return (Some(i), "", None);
    }
    let mut rest = text;
    let mut lead = None;
    for (g, i) in LEADING {
        if let Some(r) = rest.strip_prefix(g) {
            lead = Some(*i);
            rest = r;
            break;
        }
    }
    let mut trail = None;
    for (g, i) in TRAILING {
        if let Some(r) = rest.strip_suffix(g) {
            trail = Some(*i);
            rest = r;
            break;
        }
    }
    (lead, rest, trail)
}

/// Paints `icon` inside the square `r` (about the text's cap height plus a bit).
pub fn paint(painter: &egui::Painter, r: Rect, icon: Icon, color: Color32) {
    let s = r.width();
    let c = r.center();
    let w = (s / 9.0).clamp(1.1, 1.8);
    let st = Stroke::new(w, color);
    let at = |x: f32, y: f32| Pos2::new(r.left() + s * x, r.top() + s * y);
    let line = |pts: Vec<Pos2>| {
        painter.add(Shape::line(pts, st));
    };
    match icon {
        Icon::Menu | Icon::Description => {
            let (x0, x1) = if icon == Icon::Menu { (0.12, 0.88) } else { (0.14, 0.86) };
            for (y, x_end) in [(0.24, x1), (0.5, x1), (0.76, if icon == Icon::Description { 0.62 } else { x1 })] {
                painter.line_segment([at(x0, y), at(x_end, y)], st);
            }
        }
        Icon::Gear => {
            let teeth = 8;
            let (ro, ri) = (s * 0.48, s * 0.36);
            let pts: Vec<Pos2> = (0..teeth * 4)
                .map(|k| {
                    let a = k as f32 / (teeth * 4) as f32 * TAU;
                    let rad = if (k / 2) % 2 == 0 { ro } else { ri };
                    c + Vec2::angled(a) * rad
                })
                .collect();
            painter.add(Shape::closed_line(pts, st));
            painter.circle_stroke(c, s * 0.15, st);
        }
        Icon::SidebarLeft => {
            let b = Rect::from_min_max(at(0.04, 0.14), at(0.96, 0.86));
            painter.rect_stroke(b, CornerRadius::same(2), st, egui::StrokeKind::Middle);
            let x = r.left() + s * 0.38;
            painter.rect_filled(Rect::from_min_max(b.min, Pos2::new(x, b.bottom())), CornerRadius { nw: 2, sw: 2, ne: 0, se: 0 }, color.gamma_multiply(0.45));
            painter.line_segment([Pos2::new(x, b.top()), Pos2::new(x, b.bottom())], st);
        }
        Icon::Sidebar => {
            let b = Rect::from_min_max(at(0.04, 0.14), at(0.96, 0.86));
            painter.rect_stroke(b, CornerRadius::same(2), st, egui::StrokeKind::Middle);
            let x = r.left() + s * 0.62;
            painter.rect_filled(Rect::from_min_max(Pos2::new(x, b.top()), b.max), CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, color.gamma_multiply(0.45));
            painter.line_segment([Pos2::new(x, b.top()), Pos2::new(x, b.bottom())], st);
        }
        Icon::ChevronDown => line(vec![at(0.2, 0.36), at(0.5, 0.66), at(0.8, 0.36)]),
        Icon::ChevronUp => line(vec![at(0.2, 0.64), at(0.5, 0.34), at(0.8, 0.64)]),
        Icon::ChevronRight => line(vec![at(0.36, 0.2), at(0.66, 0.5), at(0.36, 0.8)]),
        Icon::ChevronLeft => line(vec![at(0.64, 0.2), at(0.34, 0.5), at(0.64, 0.8)]),
        Icon::Refresh => {
            let rad = s * 0.38;
            let start = -FRAC_PI_2 + 0.55;
            let end = start + TAU - 1.1;
            let pts: Vec<Pos2> = (0..=24).map(|k| c + Vec2::angled(start + (end - start) * k as f32 / 24.0) * rad).collect();
            painter.add(Shape::line(pts, st));
            let tip = c + Vec2::angled(end) * rad;
            let tangent = Vec2::angled(end + FRAC_PI_2);
            let normal = Vec2::angled(end);
            let back = tip - tangent * s * 0.26;
            painter.add(Shape::convex_polygon(vec![tip + tangent * s * 0.08, back + normal * s * 0.2, back - normal * s * 0.2], color, Stroke::NONE));
        }
        Icon::Link => {
            // Two rounded links at 45°.
            let d = Vec2::new(1.0, -1.0).normalized();
            for center in [c - d * s * 0.17, c + d * s * 0.17] {
                let half = s * 0.24;
                let rad = s * 0.14;
                let a = center - d * half;
                let b = center + d * half;
                let pts: Vec<Pos2> = (0..=10)
                    .map(|k| a + Vec2::angled(d.angle() + FRAC_PI_2 + std::f32::consts::PI * k as f32 / 10.0) * rad)
                    .chain((0..=10).map(|k| b + Vec2::angled(d.angle() - FRAC_PI_2 + std::f32::consts::PI * k as f32 / 10.0) * rad))
                    .collect();
                painter.add(Shape::closed_line(pts, st));
            }
        }
        Icon::Sparkle => {
            let big = s * 0.5;
            let k = s * 0.1;
            let pts = [
                c + Vec2::new(0.0, -big),
                c + Vec2::new(k, -k),
                c + Vec2::new(big, 0.0),
                c + Vec2::new(k, k),
                c + Vec2::new(0.0, big),
                c + Vec2::new(-k, k),
                c + Vec2::new(-big, 0.0),
                c + Vec2::new(-k, -k),
            ];
            // Fill as four triangles (the star isn't convex).
            for i in 0..4 {
                painter.add(Shape::convex_polygon(vec![c, pts[i * 2], pts[i * 2 + 1]], color, Stroke::NONE));
                painter.add(Shape::convex_polygon(vec![c, pts[i * 2 + 1], pts[(i * 2 + 2) % 8]], color, Stroke::NONE));
            }
        }
        Icon::Comment => {
            let b = Rect::from_min_max(at(0.06, 0.12), at(0.94, 0.72));
            painter.rect_stroke(b, CornerRadius::same((s * 0.18) as u8), st, egui::StrokeKind::Middle);
            line(vec![at(0.3, 0.72), at(0.24, 0.92), at(0.5, 0.72)]);
        }
        Icon::Check => line(vec![at(0.14, 0.52), at(0.4, 0.78), at(0.88, 0.24)]),
        Icon::Close => {
            painter.line_segment([at(0.22, 0.22), at(0.78, 0.78)], st);
            painter.line_segment([at(0.78, 0.22), at(0.22, 0.78)], st);
        }
        Icon::ArrowLeft | Icon::ArrowRight | Icon::ArrowUp => {
            let (from, to, h1, h2) = match icon {
                Icon::ArrowLeft => (at(0.88, 0.5), at(0.12, 0.5), at(0.44, 0.18), at(0.44, 0.82)),
                Icon::ArrowRight => (at(0.12, 0.5), at(0.88, 0.5), at(0.56, 0.18), at(0.56, 0.82)),
                _ => (at(0.5, 0.9), at(0.5, 0.1), at(0.18, 0.42), at(0.82, 0.42)),
            };
            painter.line_segment([from, to], st);
            line(vec![h1, to, h2]);
        }
        Icon::Pencil => {
            painter.line_segment([at(0.2, 0.8), at(0.8, 0.2)], Stroke::new(w * 1.8, color));
            painter.line_segment([at(0.12, 0.88), at(0.2, 0.8)], st);
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.6)), CornerRadius::same(1), color);
        }
        Icon::Dots => {
            for x in [0.2, 0.5, 0.8] {
                painter.circle_filled(at(x, 0.5), s * 0.08, color);
            }
        }
    }
}

/// Icon size for a font: a little taller than the cap height, like SF Symbols.
pub fn size_for(font: &FontId) -> f32 {
    (font.size * 0.95).round()
}

/// Lays out [icon] text [icon] from `x`, vertically centered on `cy`. Returns its width.
#[allow(clippy::too_many_arguments)]
fn paint_row(ui: &Ui, lead: Option<Icon>, text: &str, trail: Option<Icon>, font: &FontId, color: Color32, x: f32, cy: f32) -> f32 {
    let painter = ui.painter();
    let isz = size_for(font);
    let gap = (font.size * 0.4).round();
    let mut x = x;
    let start = x;
    if let Some(i) = lead {
        paint(painter, Rect::from_center_size(Pos2::new(x + isz / 2.0, cy), Vec2::splat(isz)), i, color);
        x += isz;
        if !text.is_empty() {
            x += gap;
        }
    }
    if !text.is_empty() {
        let g = painter.layout_no_wrap(text.to_string(), font.clone(), color);
        let h = g.size().y;
        x += g.size().x;
        painter.galley(Pos2::new(x - g.size().x, cy - h / 2.0), g, color);
    }
    if let Some(i) = trail {
        let tsz = (isz * 0.7).round();
        x += gap * 0.75;
        paint(painter, Rect::from_center_size(Pos2::new(x + tsz / 2.0, cy), Vec2::splat(tsz)), i, color);
        x += tsz;
    }
    x - start
}

fn row_width(ui: &Ui, lead: Option<Icon>, text: &str, trail: Option<Icon>, font: &FontId) -> f32 {
    let isz = size_for(font);
    let gap = (font.size * 0.4).round();
    let tw = if text.is_empty() { 0.0 } else { ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x) };
    let mut w = tw;
    if lead.is_some() {
        w += isz + if text.is_empty() { 0.0 } else { gap };
    }
    if trail.is_some() {
        w += gap * 0.75 + (isz * 0.7).round();
    }
    w
}

/// Colors for one button style.
#[derive(Clone, Copy)]
pub struct Look {
    pub fill: Color32,
    pub fg: Color32,
    pub stroke: Color32,
    /// Use egui's widget visuals (hover/press backgrounds) instead of `fill`.
    pub native: bool,
    pub frame: bool,
    pub radius: CornerRadius,
}

/// A button with optional icons around its text. Text like "✦ Discuss" or "Review ▾" is
/// split into icon + label.
pub fn button(ui: &mut Ui, text: &str, font: FontId, look: Look, min_h: f32, enabled: bool) -> Response {
    button_ex(ui, text, &[], font, look, min_h, enabled)
}

/// Like [`button`], but as wide as the widest of `text` and `variants`, so a button whose
/// label changes ("Fix it" → "Fixing…", "Show" → "Hide") never changes size.
pub fn button_ex(ui: &mut Ui, text: &str, variants: &[&str], font: FontId, look: Look, min_h: f32, enabled: bool) -> Response {
    button_full(ui, text, variants, font, look, min_h, enabled, false)
}

/// The general button. `busy`: work is running — the button stays put, disabled, with a
/// spinner in place of its label (instead of being swapped for a spinner, which would
/// shrink the row).
#[allow(clippy::too_many_arguments)]
pub fn button_full(ui: &mut Ui, text: &str, variants: &[&str], font: FontId, look: Look, min_h: f32, enabled: bool, busy: bool) -> Response {
    let enabled = enabled && !busy;
    let (lead, label, trail) = split(text);
    let icon_only = label.is_empty();
    let pad_x = if icon_only { 6.0 } else if look.frame { 10.0 } else { 4.0 };
    let w = std::iter::once(text)
        .chain(variants.iter().copied())
        .map(|t| {
            let (l, t, r) = split(t);
            row_width(ui, l, t, r, &font)
        })
        .fold(0.0, f32::max)
        + pad_x * 2.0;
    let h = min_h.max(font.size + 10.0);
    let w = if icon_only { w.max(h) } else { w };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, if label.is_empty() { text } else { label }));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let p = crate::theme::Palette::current(ui.ctx());
    let mut fg = look.fg;
    // Zed-style states: hover and press shade the fill toward the text color; ghost
    // buttons (no fill) get the theme's hover / active background. Size never changes.
    let base = if look.frame && !look.native { look.fill } else { Color32::TRANSPARENT };
    let fill = if base.a() == 0 {
        if pressed {
            p.active
        } else if hovered {
            p.hover
        } else {
            Color32::TRANSPARENT
        }
    } else if pressed {
        crate::theme::over(look.fg, 0.16, base)
    } else if hovered {
        crate::theme::over(look.fg, 0.08, base)
    } else {
        base
    };
    let stroke = if look.frame && !look.native { look.stroke } else { Color32::TRANSPARENT };
    let (fill, stroke) = if enabled { (fill, stroke) } else { (fill.gamma_multiply(0.6), stroke.gamma_multiply(0.6)) };
    ui.painter().rect(rect, look.radius, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    if !enabled {
        fg = fg.gamma_multiply(0.45);
    }
    if busy {
        let s = (font.size * 0.95).round();
        egui::Spinner::new().size(s).color(look.fg).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(s)));
        return resp;
    }
    // Centered: wider-than-needed buttons (see `button_ex`) keep their label in the middle.
    let content_w = row_width(ui, lead, label, trail, &font);
    let x = rect.center().x - content_w / 2.0;
    paint_row(ui, lead, label, trail, &font, fg, x, rect.center().y);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A segmented control: options share one frame; the selected one is filled. Every
/// segment keeps its size whatever the state. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], font_size: f32) -> bool {
    let p = crate::theme::Palette::current(ui.ctx());
    let mut changed = false;
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(p.radius + 1))
        .inner_margin(egui::Margin::same(1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.horizontal(|ui| {
                for (v, label) in options {
                    let selected = *value == *v;
                    let look = Look {
                        fill: if selected { p.selected_control } else { Color32::TRANSPARENT },
                        fg: if selected { p.text } else { p.muted_text },
                        stroke: Color32::TRANSPARENT,
                        native: false,
                        frame: true,
                        radius: CornerRadius::same(p.radius),
                    };
                    if button(ui, label, FontId::proportional(font_size), look, 22.0, true).clicked() && !selected {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// A toolbar button that looks like egui's own (`ui.button`), with icons.
pub fn tool_button(ui: &mut Ui, text: &str) -> Response {
    let p = crate::theme::Palette::current(ui.ctx());
    let look = Look { fill: Color32::TRANSPARENT, fg: p.text, stroke: Color32::TRANSPARENT, native: false, frame: true, radius: CornerRadius::same(p.radius) };
    button(ui, text, FontId::proportional(13.0), look, 24.0, true)
}

/// A frameless icon button (hover shows a background).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, color: Color32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size + 8.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, format!("{icon:?}")));
    let hovered = enabled && resp.hovered();
    if hovered {
        let p = crate::theme::Palette::current(ui.ctx());
        let bg = if resp.is_pointer_button_down_on() { p.active } else { p.hover };
        ui.painter().rect_filled(rect, CornerRadius::same(p.radius), bg);
    }
    let c = if enabled { color } else { color.gamma_multiply(0.5) };
    paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(size)), icon, c);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A non-interactive label with icons, e.g. "✓ Resolved" or "✦ AI summary".
pub fn label(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    let (lead, label, trail) = split(text);
    let font = FontId::proportional(size);
    let w = row_width(ui, lead, label, trail, &font);
    let h = ui.fonts_mut(|f| f.row_height(&font)).max(size_for(&font));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_row(ui, lead, label, trail, &font, color, rect.left(), rect.center().y);
    }
    resp
}

/// Just an icon, sized like text of `size` points.
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let font = FontId::proportional(size);
    let isz = size_for(&font);
    let h = ui.fonts_mut(|f| f.row_height(&font));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(isz, h), Sense::hover());
    paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(isz)), icon, color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_icons_from_text() {
        assert_eq!(split("✦ Discuss"), (Some(Icon::Sparkle), "Discuss", None));
        assert_eq!(split("Review (2) ▾"), (None, "Review (2)", Some(Icon::ChevronDown)));
        assert_eq!(split("⚙"), (Some(Icon::Gear), "", None));
        assert_eq!(split("Plain"), (None, "Plain", None));
        assert_eq!(split("⬆ 2 local"), (Some(Icon::ArrowUp), "2 local", None));
    }
}
