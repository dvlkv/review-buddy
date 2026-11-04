//! The file tree column: "Summary" row, filter box, the tree (with the "Review first"
//! section), and the "N / M files viewed" footer. Port of `FileTreeView.swift`.
//!
//! Rows are painted directly (no child widgets) and only the visible ones are drawn, so
//! PRs with thousands of files stay cheap.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use review_buddy_core::FileStatus;

use super::common::pal;
use crate::file_tree::{FileTreeBuilder, FileTreeRow};
use crate::pr_store::PRStore;
use crate::theme::Palette;

const ROW_H: f32 = 24.0;
const FOOTER_H: f32 = 30.0;

#[derive(Default)]
pub struct FileTreeState {
    /// Collapsed folder row ids.
    pub collapsed: HashSet<String>,
    /// Bumped whenever `collapsed` changes (part of the row cache key).
    collapsed_version: u64,
    rows: Vec<FileTreeRow>,
    rows_key: Option<RowsKey>,
    /// The file the tree last scrolled to follow.
    followed_file: Option<usize>,
    /// Scroll offset and viewport height from the last frame.
    offset: f32,
    view_h: f32,
}

#[derive(PartialEq)]
struct RowsKey {
    diff: usize,
    filter: String,
    collapsed: u64,
    priority: Vec<usize>,
}

enum Action {
    Open(usize),
    ToggleFolder(String),
    ToggleViewed(usize),
    AddFirst(String),
    RemoveFirst(String),
    Move(String, i64),
    ResetOrder,
}

pub fn file_tree(ui: &mut egui::Ui, store: &mut PRStore, state: &mut FileTreeState) {
    let p = pal(ui);
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 8, bottom: 0 }).show(ui, |ui| {
        overview_row(ui, store, &p);
    });
    egui::Frame::new().inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        filter_box(ui, store, &p);
    });

    rebuild_rows(store, state);

    let list_h = (ui.available_height() - FOOTER_H).max(ROW_H);
    let mut actions: Vec<Action> = vec![];
    egui::Frame::new().show(ui, |ui| {
        ui.set_height(list_h);
        file_list(ui, store, state, &p, list_h, &mut actions);
    });

    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, p.border));
    footer(ui, store, &p);

    for a in actions {
        match a {
            Action::Open(i) => store.scroll_to_file(i),
            Action::ToggleFolder(id) => {
                if !state.collapsed.remove(&id) {
                    state.collapsed.insert(id);
                }
                state.collapsed_version += 1;
            }
            Action::ToggleViewed(i) => store.toggle_viewed(i),
            Action::AddFirst(path) => store.add_to_review_order(&path),
            Action::RemoveFirst(path) => store.remove_from_review_order(&path),
            Action::Move(path, d) => store.move_in_review_order(&path, d),
            Action::ResetOrder => store.reset_review_order(),
        }
    }
}

fn rebuild_rows(store: &PRStore, state: &mut FileTreeState) {
    let priority: Vec<usize> = match &store.diff {
        Some(d) => store.review_order().iter().filter_map(|p| d.file_index(p)).collect(),
        None => vec![],
    };
    let key = RowsKey {
        diff: store.diff.as_ref().map(|d| Arc::as_ptr(d) as usize).unwrap_or(0),
        filter: store.file_filter.clone(),
        collapsed: state.collapsed_version,
        priority,
    };
    if state.rows_key.as_ref() == Some(&key) {
        return;
    }
    let paths: Vec<String> = store.files().iter().map(|f| f.path().to_string()).collect();
    state.rows = FileTreeBuilder::rows(&paths, &key.filter, &state.collapsed, &key.priority);
    state.rows_key = Some(key);
}

// MARK: - Summary row

/// Opens the PR summary screen (conversation, commits, merge status, AI pre-review) in
/// place of the diff.
fn overview_row(ui: &mut Ui, store: &mut PRStore, p: &Palette) {
    let conv = store.detail.as_ref().and_then(|d| d.conversation.as_ref());
    let comments = conv.map(|c| c.comments.len()).unwrap_or(0) + store.threads.len();
    let commits = conv.map(|c| c.commits.len()).unwrap_or(0);
    let findings = store.open_findings().len();
    let active = store.show_overview;

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    let painter = ui.painter();
    let radius = CornerRadius::same(6);
    painter.rect_filled(rect, radius, p.canvas);
    if active {
        painter.rect_filled(rect, radius, p.accent.gamma_multiply(0.18));
    } else if resp.hovered() {
        painter.rect_filled(rect, radius, p.subtle);
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.0, if active { p.accent } else { p.border }), StrokeKind::Inside);

    let fg = if active { p.text } else { p.muted_text };
    let font = FontId::proportional(12.0);
    let mid = rect.center().y;
    let mut x = rect.left() + 8.0;
    draw_bubbles(painter, Pos2::new(x + 8.0, mid), fg);
    x += 24.0;
    painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, "Summary", font.clone(), fg);

    // Counts, right to left.
    let mut right = rect.right() - 8.0;
    let mut count = |n: usize, color: Color32, icon: fn(&egui::Painter, Pos2, Color32)| {
        let g = painter.layout_no_wrap(n.to_string(), font.clone(), color);
        right -= g.size().x;
        painter.galley(Pos2::new(right, mid - g.size().y / 2.0), g, color);
        right -= 9.0;
        icon(painter, Pos2::new(right, mid), color);
        right -= 14.0;
    };
    if commits > 0 {
        count(commits, fg, draw_commit);
    }
    if comments > 0 {
        count(comments, fg, |pa, c, col| draw_bubble(pa, c, 11.0, col, false));
    }
    if findings > 0 {
        count(findings, p.ai_accent, |pa, c, col| draw_sparkle(pa, c, 5.5, col));
    }

    let resp = resp.on_hover_text("Conversation, commits, merge status and AI pre-review");
    if resp.clicked() {
        store.show_overview = !store.show_overview;
    }
}

// MARK: - Filter

fn filter_box(ui: &mut Ui, store: &mut PRStore, p: &Palette) {
    egui::Frame::new()
        .fill(p.canvas)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::hover());
                let c = r.center();
                for (i, w) in [10.0f32, 7.0, 4.0].into_iter().enumerate() {
                    let y = c.y - 3.5 + i as f32 * 3.5;
                    ui.painter().line_segment([Pos2::new(c.x - w / 2.0, y), Pos2::new(c.x + w / 2.0, y)], Stroke::new(1.2, p.muted_text));
                }
                ui.add_space(6.0);
                let te = egui::TextEdit::singleline(&mut store.file_filter)
                    .hint_text(RichText::new("Filter files…").color(p.muted_text))
                    .frame(egui::Frame::NONE)
                    .font(FontId::proportional(12.0))
                    .desired_width(f32::INFINITY);
                ui.add(te);
            });
        });
}

// MARK: - The list

fn file_list(ui: &mut Ui, store: &PRStore, state: &mut FileTreeState, p: &Palette, list_h: f32, actions: &mut Vec<Action>) {
    let current_row = state.rows.iter().position(|r| r.file_index == Some(store.current_file));

    // Follow the diff's current file: scroll only when its row is off-screen. Near the
    // top, show the very top so the "Review first" label stays visible.
    let mut scroll_to = None;
    if state.followed_file != Some(store.current_file) {
        if let Some(r) = current_row {
            state.followed_file = Some(store.current_file);
            let (top, bottom) = (r as f32 * ROW_H, (r + 1) as f32 * ROW_H);
            let view_h = if state.view_h > 0.0 { state.view_h } else { list_h };
            if r <= 1 {
                scroll_to = Some(0.0);
            } else if top < state.offset {
                scroll_to = Some(top);
            } else if bottom > state.offset + view_h {
                scroll_to = Some(bottom - view_h);
            }
        }
    }

    let mut area = egui::ScrollArea::vertical().id_salt("file_tree_list").auto_shrink([false, false]).max_height(list_h);
    if let Some(y) = scroll_to {
        area = area.vertical_scroll_offset(y);
    }
    let out = area.show_viewport(ui, |ui, viewport| {
        let total = state.rows.len();
        ui.set_height(ROW_H * total as f32);
        let origin = ui.max_rect().min;
        let width = ui.max_rect().width();
        let first = (viewport.min.y / ROW_H).floor().max(0.0) as usize;
        let last = ((viewport.max.y / ROW_H).ceil() as usize + 1).min(total);
        for i in first..last {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + i as f32 * ROW_H), Vec2::new(width, ROW_H));
            row(ui, store, &state.rows[i], rect, Some(i) == current_row, &state.collapsed, p, actions);
        }
    });
    state.offset = out.state.offset.y;
    state.view_h = out.inner_rect.height();
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    store: &PRStore,
    r: &FileTreeRow,
    rect: Rect,
    selected: bool,
    collapsed: &HashSet<String>,
    p: &Palette,
    actions: &mut Vec<Action>,
) {
    let mid = rect.center().y;
    let mut x = rect.left() + 4.0 + r.depth as f32 * 12.0;
    if r.is_section {
        let painter = ui.painter();
        painter.text(Pos2::new(x + 4.0, mid + 2.0), Align2::LEFT_CENTER, r.name.to_uppercase(), FontId::proportional(10.0), p.muted_text);
        return;
    }
    let id = ui.id().with(("tree_row", &r.id));
    let resp = ui.interact(rect, id, Sense::click());
    let painter = ui.painter().clone();

    // GitHub-style selection: a soft accent rounded rect with a bar on the left.
    if selected {
        painter.rect_filled(rect.shrink2(Vec2::new(6.0, 1.0)), CornerRadius::same(6), p.accent.gamma_multiply(0.14));
        let bar = Rect::from_min_size(Pos2::new(rect.left() + 6.0, rect.top() + 5.0), Vec2::new(3.0, ROW_H - 10.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect.shrink2(Vec2::new(6.0, 1.0)), CornerRadius::same(6), p.subtle);
    }

    if r.is_folder {
        let is_collapsed = collapsed.contains(&r.id);
        draw_folder(&painter, Pos2::new(x + 8.0, mid), p.muted_text, !is_collapsed);
        x += 21.0;
        let font = FontId::proportional(12.0);
        let name = truncate_middle(ui, &r.name, &font, rect.right() - 10.0 - x);
        painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, name, font, p.muted_text);
        if resp.clicked() {
            actions.push(Action::ToggleFolder(r.id.clone()));
        }
        return;
    }

    let Some(fi) = r.file_index else { return };
    let Some(file) = store.files().get(fi) else { return };
    let path = file.path().to_string();
    let viewed = store.viewed.contains(&path);

    if let Some(rank) = r.rank {
        painter.text(Pos2::new(x + 12.0, mid), Align2::CENTER_CENTER, rank.to_string(), FontId::monospace(10.0), p.ai_accent);
        x += 18.0;
    }
    draw_status(&painter, Pos2::new(x + 8.0, mid), file.status, p);
    x += 21.0;

    // Right side, right to left: viewed checkbox, Discuss marker, threads, +/- counts.
    let mut right = rect.right() - 8.0;
    let box_rect = Rect::from_center_size(Pos2::new(right - 6.0, mid), Vec2::splat(12.0));
    let box_resp = ui.interact(box_rect.expand(3.0), id.with("viewed"), Sense::click());
    draw_checkbox(&painter, box_rect, viewed, box_resp.hovered(), p);
    let box_resp = box_resp.on_hover_text(if viewed { "Viewed (click to unmark)" } else { "Mark as viewed" });
    if box_resp.clicked() {
        actions.push(Action::ToggleViewed(fi));
    }
    right -= 20.0;

    let has_discuss = store.discuss_threads.values().any(|t| t.anchor.path == path);
    if has_discuss {
        draw_sparkle(&painter, Pos2::new(right - 5.0, mid), 4.5, p.ai_accent);
        right -= 14.0;
    }
    let threads = store.open_thread_count_by_path.get(&path).copied().unwrap_or(0);
    let small = FontId::proportional(10.0);
    if threads > 0 {
        let g = painter.layout_no_wrap(threads.to_string(), small.clone(), p.muted_text);
        right -= g.size().x;
        painter.galley(Pos2::new(right, mid - g.size().y / 2.0), g, p.muted_text);
        draw_bubble(&painter, Pos2::new(right - 7.0, mid), 9.0, p.muted_text, true);
        right -= 18.0;
    }
    // +/- counts only when there is room for a readable name.
    if right - x > 110.0 && (file.additions > 0 || file.deletions > 0) {
        let mono = FontId::monospace(10.0);
        if file.deletions > 0 {
            let g = painter.layout_no_wrap(format!("−{}", file.deletions), mono.clone(), p.del_marker);
            right -= g.size().x;
            painter.galley(Pos2::new(right, mid - g.size().y / 2.0), g, p.del_marker);
            right -= 4.0;
        }
        if file.additions > 0 {
            let g = painter.layout_no_wrap(format!("+{}", file.additions), mono, p.add_marker);
            right -= g.size().x;
            painter.galley(Pos2::new(right, mid - g.size().y / 2.0), g, p.add_marker);
        }
        right -= 6.0;
    }

    let font = FontId::proportional(12.0);
    let name = truncate_middle(ui, &r.name, &font, right - x - 4.0);
    painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, name, font, if viewed { p.muted_text } else { p.text });

    if resp.clicked() {
        actions.push(Action::Open(fi));
    }
    let resp = if r.name != path { resp.on_hover_text(&path) } else { resp };
    resp.context_menu(|ui| context_menu(ui, store, fi, &path, viewed, actions));
}

fn context_menu(ui: &mut Ui, store: &PRStore, fi: usize, path: &str, viewed: bool, actions: &mut Vec<Action>) {
    let order = store.review_order();
    let mut item = |ui: &mut Ui, label: &str, a: Action| {
        if ui.button(label).clicked() {
            actions.push(a);
            ui.close();
        }
    };
    if let Some(rank) = order.iter().position(|p| p == path) {
        if rank > 0 {
            item(ui, "Move up", Action::Move(path.into(), -1));
        }
        if rank + 1 < order.len() {
            item(ui, "Move down", Action::Move(path.into(), 1));
        }
        item(ui, "Remove from Review first", Action::RemoveFirst(path.into()));
    } else {
        item(ui, "Review first", Action::AddFirst(path.into()));
    }
    let has_ai_order = store.summary.as_ref().map(|s| !s.reading_order.is_empty()).unwrap_or(false);
    if store.custom_review_order.is_some() && has_ai_order {
        item(ui, "Reset to AI reading order", Action::ResetOrder);
    }
    ui.separator();
    item(ui, if viewed { "Mark as not viewed" } else { "Mark as viewed" }, Action::ToggleViewed(fi));
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.to_string());
        ui.close();
    }
}

// MARK: - Footer

fn footer(ui: &mut Ui, store: &PRStore, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), FOOTER_H), Sense::hover());
    let painter = ui.painter();
    let total = store.files().len();
    let viewed = store.viewed_count();
    let mid = rect.center().y;
    painter.text(
        Pos2::new(rect.left() + 10.0, mid),
        Align2::LEFT_CENTER,
        format!("{viewed} / {total} files viewed"),
        FontId::proportional(11.0),
        p.muted_text,
    );
    let bar = Rect::from_min_size(Pos2::new(rect.right() - 70.0, mid - 2.0), Vec2::new(60.0, 4.0));
    painter.rect_filled(bar, CornerRadius::same(2), p.border);
    let frac = viewed as f32 / total.max(1) as f32;
    if frac > 0.0 {
        painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * frac, bar.height())), CornerRadius::same(2), p.accent);
    }
}

// MARK: - Helpers

/// `text`, with characters cut from the middle ("src/…/file.rs") until it fits `width`.
fn truncate_middle(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let measure = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x);
    if width <= 0.0 {
        return String::new();
    }
    if measure(text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len().saturating_sub(1);
    while keep > 1 {
        let head = keep.div_ceil(2);
        let tail = keep / 2;
        let s: String = chars[..head].iter().chain(std::iter::once(&'…')).chain(chars[chars.len() - tail..].iter()).collect();
        if measure(&s) <= width {
            return s;
        }
        keep -= 1;
    }
    "…".into()
}

fn status_color(s: FileStatus, p: &Palette) -> Color32 {
    match s {
        FileStatus::Added => p.add_marker,
        FileStatus::Deleted => p.del_marker,
        FileStatus::Renamed | FileStatus::Copied => p.attention,
        FileStatus::Modified => p.muted_text,
    }
}

/// GitHub's file status icons: filled squares with +, − or →; a page outline for edits.
fn draw_status(painter: &egui::Painter, c: Pos2, s: FileStatus, p: &Palette) {
    let color = status_color(s, p);
    let w = Stroke::new(1.4, Color32::WHITE);
    match s {
        FileStatus::Modified => {
            let r = Rect::from_center_size(c, Vec2::new(9.0, 11.0));
            painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.1, color), StrokeKind::Inside);
            for i in 0..2 {
                let y = r.top() + 4.0 + i as f32 * 2.5;
                painter.line_segment([Pos2::new(r.left() + 2.5, y), Pos2::new(r.right() - 2.5, y)], Stroke::new(0.9, color));
            }
        }
        _ => {
            let r = Rect::from_center_size(c, Vec2::splat(11.0));
            painter.rect_filled(r, CornerRadius::same(3), color);
            let h = 2.8;
            match s {
                FileStatus::Added => {
                    painter.line_segment([c - Vec2::new(h, 0.0), c + Vec2::new(h, 0.0)], w);
                    painter.line_segment([c - Vec2::new(0.0, h), c + Vec2::new(0.0, h)], w);
                }
                FileStatus::Deleted => {
                    painter.line_segment([c - Vec2::new(h, 0.0), c + Vec2::new(h, 0.0)], w);
                }
                _ => {
                    painter.line_segment([c - Vec2::new(h, 0.0), c + Vec2::new(h, 0.0)], w);
                    painter.line_segment([c + Vec2::new(h, 0.0), c + Vec2::new(0.5, -2.2)], w);
                    painter.line_segment([c + Vec2::new(h, 0.0), c + Vec2::new(0.5, 2.2)], w);
                }
            }
        }
    }
}

fn draw_folder(painter: &egui::Painter, c: Pos2, color: Color32, filled: bool) {
    let body = Rect::from_center_size(c + Vec2::new(0.0, 1.0), Vec2::new(13.0, 9.0));
    let tab = Rect::from_min_size(body.min - Vec2::new(0.0, 2.0), Vec2::new(5.5, 3.0));
    if filled {
        let fill = color.gamma_multiply(0.8);
        painter.rect_filled(tab, CornerRadius::same(1), fill);
        painter.rect_filled(body, CornerRadius::same(2), fill);
    } else {
        let s = Stroke::new(1.1, color);
        painter.rect_stroke(tab, CornerRadius::same(1), s, StrokeKind::Inside);
        painter.rect_stroke(body, CornerRadius::same(2), s, StrokeKind::Inside);
    }
}

fn draw_checkbox(painter: &egui::Painter, r: Rect, checked: bool, hovered: bool, p: &Palette) {
    let radius = CornerRadius::same(3);
    if checked {
        painter.rect_filled(r, radius, p.accent);
        let pts = vec![r.left_center() + Vec2::new(2.5, 0.0), r.center_bottom() + Vec2::new(-1.0, -3.0), r.right_top() + Vec2::new(-2.5, 3.0)];
        painter.add(Shape::line(pts, Stroke::new(1.6, Color32::WHITE)));
    } else {
        painter.rect_filled(r, radius, p.canvas);
        painter.rect_stroke(r, radius, Stroke::new(1.0, if hovered { p.accent } else { p.border }), StrokeKind::Inside);
    }
}

/// A speech bubble (`bubble.left`), outlined or filled.
fn draw_bubble(painter: &egui::Painter, c: Pos2, size: f32, color: Color32, filled: bool) {
    let r = Rect::from_center_size(c - Vec2::new(0.0, size * 0.08), Vec2::new(size, size * 0.72));
    let tail = vec![
        Pos2::new(r.left() + size * 0.18, r.bottom() - 1.0),
        Pos2::new(r.left() + size * 0.12, r.bottom() + size * 0.22),
        Pos2::new(r.left() + size * 0.45, r.bottom() - 1.0),
    ];
    let radius = CornerRadius::same((size * 0.25) as u8);
    if filled {
        painter.rect_filled(r, radius, color);
        painter.add(Shape::convex_polygon(tail, color, Stroke::NONE));
    } else {
        painter.rect_stroke(r, radius, Stroke::new(1.1, color), StrokeKind::Inside);
        painter.add(Shape::line(tail, Stroke::new(1.1, color)));
    }
}

/// Two overlapping bubbles (`bubble.left.and.bubble.right`).
fn draw_bubbles(painter: &egui::Painter, c: Pos2, color: Color32) {
    draw_bubble(painter, c + Vec2::new(-2.0, -1.5), 10.0, color, false);
    let r = Rect::from_center_size(c + Vec2::new(3.0, 3.0), Vec2::new(9.0, 6.5));
    painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.1, color), StrokeKind::Inside);
}

/// A commit-graph glyph: a ring on a line.
fn draw_commit(painter: &egui::Painter, c: Pos2, color: Color32) {
    let s = Stroke::new(1.2, color);
    painter.line_segment([c - Vec2::new(6.0, 0.0), c - Vec2::new(3.0, 0.0)], s);
    painter.line_segment([c + Vec2::new(3.0, 0.0), c + Vec2::new(6.0, 0.0)], s);
    painter.circle_stroke(c, 2.8, s);
}

/// A four-pointed sparkle (the AI/Discuss marker).
fn draw_sparkle(painter: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    let k = r * 0.3;
    let pts = [
        c + Vec2::new(0.0, -r),
        c + Vec2::new(k, -k),
        c + Vec2::new(r, 0.0),
        c + Vec2::new(k, k),
        c + Vec2::new(0.0, r),
        c + Vec2::new(-k, k),
        c + Vec2::new(-r, 0.0),
        c + Vec2::new(-k, -k),
    ];
    // Concave: draw as two convex diamonds' union via a filled path of triangles.
    for i in 0..4 {
        let tip = pts[i * 2];
        let a = pts[(i * 2 + 7) % 8];
        let b = pts[i * 2 + 1];
        painter.add(Shape::convex_polygon(vec![tip, b, c, a], color, Stroke::NONE));
    }
}
