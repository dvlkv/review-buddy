//! Inbox sidebar: collapsible search sections (plus pinned repos) of PR rows, and a
//! bottom bar with "Open PR…", the last refresh time and a refresh button.

use egui::{Align, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, UiBuilder, Vec2};
use review_buddy_core::{CheckState, PRRef, PRSummary, ReviewDecision};

use crate::app::AppState;
use crate::theme::Palette;
use crate::ui::common::{pal, relative};

enum Action {
    Open(PRRef),
    Refresh,
    OpenUrl,
}

pub fn sidebar(ui: &mut Ui, state: &mut AppState) {
    let p = pal(ui);
    let mut action = None;

    // Opaque bottom bar, laid out first so the list scrolls above it.
    egui::Panel::bottom("inbox_bar")
        .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(10, 7)))
        .show_separator_line(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let open = ui
                    .add(egui::Button::new(RichText::new("🔗 Open PR…").size(12.0).color(p.accent)).frame(false))
                    .on_hover_text("Open a pull request by URL");
                if open.clicked() {
                    action = Some(Action::OpenUrl);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let r = refresh_button(ui, &p).on_hover_text("Refresh inbox");
                    if r.clicked() {
                        action = Some(Action::Refresh);
                    }
                    if let Some(t) = state.inbox.last_refresh {
                        let d = chrono::Utc::now() - chrono::Duration::from_std(t.elapsed()).unwrap_or_default();
                        ui.label(RichText::new(relative(&d)).size(10.0).color(p.muted_text));
                    }
                });
            });
        });

    let current = state.current.clone();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        for section in &state.inbox.sections {
            let open_id = ui.id().with(("inbox_section_open", &section.id));
            let mut is_open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(true);
            if section_header(ui, &p, &section.title, section.is_loading, section.items.len(), is_open) {
                is_open = !is_open;
                ui.data_mut(|d| d.insert_temp(open_id, is_open));
            }
            if !is_open {
                continue;
            }
            if let Some(e) = &section.error {
                egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 10, top: 2, bottom: 4 }).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(format!("⚠ {e}")).size(11.0).color(p.del_marker)).wrap());
                });
            } else if section.items.is_empty() && !section.is_loading {
                egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 10, top: 2, bottom: 4 }).show(ui, |ui| {
                    ui.label(RichText::new("Nothing here").size(11.0).color(p.muted_text));
                });
            }
            let show_repo = !section.id.starts_with("repo:");
            for pr in &section.items {
                let selected = current.as_ref() == Some(&pr.pr_ref);
                let resp = inbox_row(ui, &p, pr, show_repo, selected);
                if resp.clicked() {
                    action = Some(Action::Open(pr.pr_ref.clone()));
                }
                resp.context_menu(|ui| {
                    if ui.button("Open on GitHub").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(pr.pr_ref.web_url()));
                        ui.close();
                    }
                    if ui.button("Copy URL").clicked() {
                        ui.ctx().copy_text(pr.pr_ref.web_url());
                        ui.close();
                    }
                });
            }
            ui.add_space(8.0);
        }
    });

    match action {
        Some(Action::Open(r)) => state.open(r),
        Some(Action::Refresh) => state.refresh_inbox(),
        Some(Action::OpenUrl) => state.ui.show_open_url = true,
        None => {}
    }
}

/// Sidebar-style section header: chevron, title, spinner, count. Returns true when toggled.
fn section_header(ui: &mut Ui, p: &Palette, title: &str, loading: bool, count: usize, open: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
    let painter = ui.painter_at(rect);
    let hovered = resp.hovered();
    // Chevron (shown on hover like macOS sidebars, and always when collapsed).
    let c = Pos2::new(rect.left() + 10.0, rect.center().y);
    if hovered || !open {
        let s = 3.0;
        let pts = if open {
            vec![c + Vec2::new(-s, -s * 0.5), c + Vec2::new(0.0, s * 0.5), c + Vec2::new(s, -s * 0.5)]
        } else {
            vec![c + Vec2::new(-s * 0.5, -s), c + Vec2::new(s * 0.5, 0.0), c + Vec2::new(-s * 0.5, s)]
        };
        painter.add(Shape::line(pts, Stroke::new(1.5, p.muted_text)));
    }
    let galley = painter.layout_no_wrap(title.to_string(), FontId::proportional(11.0), p.muted_text);
    let tx = rect.left() + 18.0;
    let tw = galley.size().x;
    painter.galley(Pos2::new(tx, rect.center().y - galley.size().y / 2.0), galley, p.muted_text);
    if loading {
        let sr = Rect::from_center_size(Pos2::new(tx + tw + 10.0, rect.center().y), Vec2::splat(10.0));
        ui.put(sr, egui::Spinner::new().size(10.0).color(p.muted_text));
    }
    if count > 0 {
        painter.text(
            Pos2::new(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(11.0),
            p.muted_text,
        );
    }
    resp.clicked()
}

fn inbox_row(ui: &mut Ui, p: &Palette, pr: &PRSummary, show_repo: bool, selected: bool) -> egui::Response {
    let bg_idx = ui.painter().add(Shape::Noop);
    let outer = egui::Margin { left: 6, right: 6, top: 0, bottom: 0 };
    let inner = egui::Margin::symmetric(8, 5);
    let full = ui.available_rect_before_wrap();
    let builder = UiBuilder::new()
        .max_rect(Rect::from_min_max(
            full.min + Vec2::new(outer.left as f32 + inner.left as f32, inner.top as f32),
            Pos2::new(full.max.x - outer.right as f32 - inner.right as f32, full.max.y),
        ))
        .layout(Layout::top_down(Align::Min))
        .sense(Sense::click());
    let res = ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
        let w = ui.available_width();
        ui.set_width(w);
        // Title with the PR / draft icon.
        ui.horizontal_top(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::new(12.0, 15.0), Sense::hover());
            let icon_rect = Rect::from_center_size(Pos2::new(ir.center().x, ir.top() + 8.0), Vec2::splat(11.0));
            if pr.is_draft {
                draft_icon(ui, icon_rect, p.muted_text);
            } else {
                pr_icon(ui, icon_rect, p.success);
            }
            ui.add(
                egui::Label::new(RichText::new(&pr.title).size(12.0).strong().color(p.text))
                    .wrap_mode(egui::TextWrapMode::Wrap)
                    .selectable(false)
                    .halign(Align::Min),
            );
        });
        // repo#number · author ...... checks review
        let small = |s: String| RichText::new(s).size(10.5).color(p.muted_text);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if pr.is_draft {
                crate::ui::common::pill(ui, "Draft", p.muted_text, false);
            }
            let num = if show_repo { format!("{}#{}", pr.pr_ref.name_with_owner(), pr.pr_ref.number) } else { format!("#{}", pr.pr_ref.number) };
            ui.add(egui::Label::new(small(num)).truncate().selectable(false));
            ui.add(egui::Label::new(small("·".into())).selectable(false));
            ui.add(egui::Label::new(small(pr.author.clone())).truncate().selectable(false));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                review_icon(ui, p, pr.review_decision);
                checks_icon(ui, p, pr.checks);
            });
        });
        // +adds −dels  💬 n ...... updated
        let mono = |s: String, c: Color32| RichText::new(s).font(FontId::monospace(10.0)).color(c);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(egui::Label::new(mono(format!("+{}", pr.additions), p.add_marker)).selectable(false));
            ui.add(egui::Label::new(mono(format!("−{}", pr.deletions), p.del_marker)).selectable(false));
            if pr.comment_count > 0 {
                let (r, _) = ui.allocate_exact_size(Vec2::new(11.0, 12.0), Sense::hover());
                bubble_icon(ui, r, p.muted_text);
                ui.add(egui::Label::new(mono(pr.comment_count.to_string(), p.muted_text)).selectable(false));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Label::new(mono(relative(&pr.updated_at), p.muted_text)).selectable(false));
            });
        });
        ui.add_space(inner.bottom as f32);
    });
    let resp = res.response;
    let bg_rect = Rect::from_min_max(
        Pos2::new(full.left() + outer.left as f32, resp.rect.top() - inner.top as f32),
        Pos2::new(full.right() - outer.right as f32, resp.rect.bottom()),
    );
    // Advance past the top inset reserved above the scope.
    ui.advance_cursor_after_rect(bg_rect);
    let fill = if selected {
        if p.dark { p.accent.gamma_multiply(0.22) } else { p.accent.gamma_multiply(0.12) }
    } else if resp.hovered() {
        p.subtle
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().set(bg_idx, Shape::rect_filled(bg_rect, CornerRadius::same(6), fill));
    }
    if selected {
        let bar = Rect::from_min_size(bg_rect.left_top() + Vec2::new(0.0, 4.0), Vec2::new(3.0, bg_rect.height() - 8.0));
        ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.add_space(2.0);
    resp
}

fn checks_icon(ui: &mut Ui, p: &Palette, c: CheckState) {
    let (color, tip): (Color32, &str) = match c {
        CheckState::Success => (p.success, "Checks passed"),
        CheckState::Failure | CheckState::Error => (p.del_marker, "Checks failing"),
        CheckState::Pending | CheckState::Expected => (p.attention, "Checks running"),
        _ => return,
    };
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
    let painter = ui.painter();
    let st = Stroke::new(1.6, color);
    match c {
        CheckState::Success => {
            painter.add(Shape::line(vec![lerp(r, 0.12, 0.55), lerp(r, 0.4, 0.8), lerp(r, 0.88, 0.22)], st));
        }
        CheckState::Failure | CheckState::Error => {
            painter.line_segment([lerp(r, 0.2, 0.2), lerp(r, 0.8, 0.8)], st);
            painter.line_segment([lerp(r, 0.8, 0.2), lerp(r, 0.2, 0.8)], st);
        }
        _ => {
            painter.circle_filled(r.center(), 3.0, color);
        }
    }
    resp.on_hover_text(tip);
}

fn review_icon(ui: &mut Ui, p: &Palette, d: ReviewDecision) {
    let (color, tip) = match d {
        ReviewDecision::Approved => (p.success, "Approved"),
        ReviewDecision::ChangesRequested => (p.del_marker, "Changes requested"),
        _ => return,
    };
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(r.center(), 5.8, color);
    let st = Stroke::new(1.4, Color32::WHITE);
    if d == ReviewDecision::Approved {
        painter.add(Shape::line(vec![lerp(r, 0.28, 0.52), lerp(r, 0.45, 0.68), lerp(r, 0.74, 0.34)], st));
    } else {
        painter.line_segment([lerp(r, 0.5, 0.24), lerp(r, 0.5, 0.58)], st);
        painter.circle_filled(lerp(r, 0.5, 0.76), 0.9, Color32::WHITE);
    }
    resp.on_hover_text(tip);
}

fn lerp(r: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(r.left() + r.width() * x, r.top() + r.height() * y)
}

/// Octicon "git-pull-request": two branch nodes on the left, an arrow into the right node.
fn pr_icon(ui: &Ui, r: Rect, color: Color32) {
    let painter = ui.painter();
    let st = Stroke::new(1.3, color);
    let rad = r.width() * 0.13;
    let (l, rt) = (r.left() + r.width() * 0.22, r.left() + r.width() * 0.78);
    let (top, bot) = (r.top() + r.height() * 0.16, r.top() + r.height() * 0.84);
    painter.circle_stroke(Pos2::new(l, top), rad, st);
    painter.circle_stroke(Pos2::new(l, bot), rad, st);
    painter.line_segment([Pos2::new(l, top + rad), Pos2::new(l, bot - rad)], st);
    painter.circle_stroke(Pos2::new(rt, bot), rad, st);
    let mid_y = r.top() + r.height() * 0.3;
    painter.add(Shape::line(vec![Pos2::new(rt, bot - rad), Pos2::new(rt, mid_y), Pos2::new(r.center().x, mid_y)], st));
    let ax = r.center().x;
    painter.add(Shape::line(vec![Pos2::new(ax + 2.0, mid_y - 2.0), Pos2::new(ax, mid_y), Pos2::new(ax + 2.0, mid_y + 2.0)], st));
}

/// Dashed circle, like SF Symbols' "circle.dashed".
fn draft_icon(ui: &Ui, r: Rect, color: Color32) {
    let painter = ui.painter();
    let c = r.center();
    let rad = r.width() * 0.42;
    let segs = 8;
    for i in 0..segs {
        let a0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
        let a1 = a0 + std::f32::consts::TAU / segs as f32 * 0.55;
        let pts: Vec<Pos2> = (0..=4).map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / 4.0;
            c + Vec2::new(a.cos(), a.sin()) * rad
        }).collect();
        painter.add(Shape::line(pts, Stroke::new(1.3, color)));
    }
}

fn bubble_icon(ui: &Ui, r: Rect, color: Color32) {
    let painter = ui.painter();
    let b = Rect::from_min_max(r.left_top() + Vec2::new(0.5, 1.5), Pos2::new(r.right() - 0.5, r.bottom() - 3.0));
    painter.rect_stroke(b, CornerRadius::same(2), Stroke::new(1.0, color), egui::StrokeKind::Inside);
    painter.add(Shape::line(
        vec![Pos2::new(b.left() + 2.5, b.bottom()), Pos2::new(b.left() + 2.0, r.bottom() - 0.5), Pos2::new(b.left() + 5.0, b.bottom())],
        Stroke::new(1.0, color),
    ));
}

/// Borderless circular-arrow button.
fn refresh_button(ui: &mut Ui, p: &Palette) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let color = if resp.hovered() { p.text } else { p.muted_text };
    let painter = ui.painter();
    let c = r.center();
    let rad = 5.5;
    let start = -std::f32::consts::FRAC_PI_2 + 0.5;
    let end = start + std::f32::consts::TAU - 1.0;
    let pts: Vec<Pos2> = (0..=20).map(|k| {
        let a = start + (end - start) * k as f32 / 20.0;
        c + Vec2::new(a.cos(), a.sin()) * rad
    }).collect();
    painter.add(Shape::line(pts, Stroke::new(1.4, color)));
    // Arrow head at the end of the arc, pointing clockwise.
    let tip = c + Vec2::new(end.cos(), end.sin()) * rad;
    let tangent = Vec2::new(-end.sin(), end.cos());
    let normal = Vec2::new(end.cos(), end.sin());
    let back = tip - tangent * 3.2;
    painter.add(Shape::convex_polygon(
        vec![tip + tangent * 1.2, back + normal * 2.4, back - normal * 2.4],
        color,
        Stroke::NONE,
    ));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
