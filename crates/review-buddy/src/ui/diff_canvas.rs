//! The virtualized diff: a prefix-sum row index over `DiffLayout`, painting only the rows
//! in the viewport. Inline blocks (threads, composers, Discuss markers, AI findings) are
//! real egui widgets placed at their rows; their measured heights feed back into the index.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use egui::text::LayoutJob;
use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, TextFormat, Ui, Vec2};
use review_buddy_core::diff::layout::{DiffViewMode, InlineBlockKind, InlineBlockRef, LayoutRow, PlaceholderReason, RowKind};
use review_buddy_core::diff::split::{IntralineDiff, SplitAligner};
use review_buddy_core::highlight::language::LanguageRegistry;
use review_buddy_core::highlight::lexer::{LexState, Lexer, SyntaxToken};
use review_buddy_core::*;

use super::blocks;
use super::markdown::syntax_color;
use crate::pr_store::PRStore;
use crate::theme::{DiffMetrics, Palette};

/// Per-PR canvas state kept across frames.
#[derive(Default)]
pub struct CanvasState {
    layout_version: u64,
    width: f32,
    font_size: f32,
    offsets: Vec<f32>,
    pub block_heights: HashMap<String, f32>,
    h_offset: f32,
    max_code_chars: usize,
    drag: Option<DragAnchor>,
    /// Pointer position of the last mouse-up, for placing the selection bar.
    bar_point: Option<Pos2>,
    last_press: Option<(Instant, Pos2, u32)>,
    partners: HashMap<usize, Arc<Vec<i32>>>,
    /// Scroll anchor: (file path, rows into the file, pixels into the row).
    anchor: Option<(String, usize, f32)>,
    scroll_y: f32,
    force_scroll: Option<f32>,
}

impl CanvasState {
    /// Scrolls by `dy` points on the next frame (perf runs, scripts).
    pub fn scroll_by(&mut self, dy: f32) {
        let max = (self.offsets.last().copied().unwrap_or(0.0) - 600.0).max(0.0);
        self.force_scroll = Some((self.scroll_y + dy).clamp(0.0, max));
    }
}

#[derive(Clone, Copy)]
struct DragAnchor {
    file: usize,
    side: DiffSide,
    text: bool,
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    row: usize,
    file: usize,
    side: DiffSide,
    line: i64,
    in_gutter: bool,
}

#[derive(Clone, Copy)]
struct Geo {
    mode: DiffViewMode,
    num_width: f32,
    marker_width: f32,
    width: f32,
}

impl Geo {
    fn half(&self) -> f32 {
        (self.width / 2.0).floor()
    }
    fn code_x(&self, side: DiffSide) -> f32 {
        match self.mode {
            DiffViewMode::Unified => self.num_width * 2.0 + self.marker_width,
            DiffViewMode::Split => (if side == DiffSide::Left { 0.0 } else { self.half() }) + self.num_width + self.marker_width,
        }
    }
    fn code_width(&self, _side: DiffSide) -> f32 {
        match self.mode {
            DiffViewMode::Unified => self.width - self.code_x(DiffSide::Right),
            DiffViewMode::Split => self.half() - self.num_width - self.marker_width,
        }
    }
    fn side_at(&self, x: f32) -> DiffSide {
        if self.mode == DiffViewMode::Split && x >= self.half() { DiffSide::Right } else { DiffSide::Left }
    }
    fn is_gutter(&self, x: f32) -> bool {
        if self.mode == DiffViewMode::Unified {
            return x < self.num_width * 2.0 + self.marker_width;
        }
        let local = if x >= self.half() { x - self.half() } else { x };
        local < self.num_width + self.marker_width
    }
    fn gutter_width(&self) -> f32 {
        if self.mode == DiffViewMode::Unified { self.num_width * 2.0 } else { self.num_width }
    }
}

#[derive(Clone, Copy, Debug)]
struct SideLine {
    kind: Option<LineKind>,
    number: i64,
    line_index: Option<usize>,
    expanded_new: Option<i64>,
}

pub enum CanvasAction {
    None,
    OpenDiscussPanel,
}

/// Draws the diff for `store`. Returns actions for the window (e.g. open the Discuss panel).
pub fn diff_canvas(ui: &mut Ui, store: &mut PRStore, cs: &mut CanvasState, metrics: &DiffMetrics) -> CanvasAction {
    let mut action = CanvasAction::None;
    let p = Palette::current(ui.ctx());
    let Some(layout_rows) = store.layout.as_ref().map(|l| l.row_count()) else { return action };
    let width = ui.available_width();
    let max_line = store.files().iter().flat_map(|f| f.hunks.last()).map(|h| (h.old_end().max(h.new_end())) as i64).max().unwrap_or(999);
    let geo = Geo {
        mode: store.options.mode,
        num_width: metrics.number_column_width(max_line + 20),
        marker_width: metrics.marker_width,
        width,
    };

    // Rebuild the row index when the layout or geometry changed; keep the reading position.
    if cs.layout_version != store.layout_version || cs.width != width || cs.font_size != metrics.font_size || cs.offsets.len() != layout_rows + 1 {
        let anchor = cs.anchor.clone();
        let relayout = cs.layout_version != store.layout_version;
        cs.layout_version = store.layout_version;
        cs.width = width;
        cs.font_size = metrics.font_size;
        rebuild_offsets(store, cs, metrics);
        cs.max_code_chars = max_code_chars(store);
        if relayout && store.pending_scroll.is_none() {
            if let Some((path, rows_in, px)) = anchor {
                if let (Some(l), Some(f)) = (&store.layout, store.diff.as_ref().and_then(|d| d.file_index(&path))) {
                    let r = l.row_range_for_file(f);
                    let row = (r.start + rows_in).min(r.end.saturating_sub(1)).min(layout_rows.saturating_sub(1));
                    let y = cs.offsets.get(row).copied().unwrap_or(0.0) + px;
                    if (y - cs.scroll_y).abs() > 0.5 {
                        cs.force_scroll = Some(y);
                    }
                }
            }
        }
    }
    if let Some(row) = store.pending_scroll.take() {
        cs.force_scroll = Some(cs.offsets.get(row).copied().unwrap_or(0.0));
    }
    let content_h = *cs.offsets.last().unwrap_or(&0.0);

    let mut sa = egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).id_salt(("diff", store.pr_ref.to_string())).auto_shrink([false, false]);
    if let Some(y) = cs.force_scroll.take() {
        sa = sa.vertical_scroll_offset(y.max(0.0));
    }
    let out = sa.show_viewport(ui, |ui, viewport| {
        let (content_rect, resp) = ui.allocate_exact_size(Vec2::new(width, content_h.max(1.0)), Sense::click_and_drag());
        let origin = content_rect.min;
        let painter = ui.painter_at(viewport.translate(origin.to_vec2()));
        painter.rect_filled(viewport.translate(origin.to_vec2()), 0.0, p.canvas);

        // Horizontal scrolling of the code columns.
        if resp.hovered() {
            let dx = ui.input(|i| i.smooth_scroll_delta.x);
            if dx != 0.0 {
                let max = (cs.max_code_chars as f32 * metrics.char_width + 40.0 - geo.code_width(DiffSide::Right)).max(0.0);
                cs.h_offset = (cs.h_offset - dx).clamp(0.0, max);
            }
        }

        let first = row_at(cs, viewport.top());
        let last = row_at(cs, (viewport.bottom()).min(content_h - 0.01));
        if layout_rows == 0 {
            return;
        }
        store.report_top_row(row_at(cs, viewport.top() + 1.0));
        // Anchor for the next relayout: the row at the top of the viewport.
        if let (Some(l), Some(d)) = (&store.layout, &store.diff) {
            let top = row_at(cs, viewport.top());
            let f = l.file_index_for_row(top);
            if let Some(fd) = d.files.get(f) {
                cs.anchor = Some((fd.path().to_string(), top - l.file_start_row[f], viewport.top() - cs.offsets[top]));
            }
        }

        // Pointer input over code rows.
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let local = |pos: Pos2| Pos2::new(pos.x - origin.x, pos.y - origin.y);
        let hover_hit = resp.hover_pos().and_then(|pos| hit(store, cs, &geo, local(pos)));
        if let Some(h) = hover_hit {
            ui.ctx().set_cursor_icon(if h.in_gutter { CursorIcon::PointingHand } else { CursorIcon::Text });
        }
        handle_mouse(ui, store, cs, &geo, metrics, &resp, pointer.map(local), &mut action);

        // Paint rows.
        let mut block_rows = vec![];
        for i in first..=last.max(first) {
            if i >= layout_rows {
                break;
            }
            let row = store.layout.as_ref().unwrap().rows[i];
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + cs.offsets[i]), Vec2::new(width, cs.offsets[i + 1] - cs.offsets[i]));
            match row.kind {
                RowKind::Line | RowKind::Pair | RowKind::Expanded => draw_code_row(ui, &painter, store, cs, &geo, metrics, &p, row, rect),
                RowKind::HunkHeader => draw_hunk_header(ui, &painter, store, &geo, metrics, &p, row, rect),
                RowKind::ExpandFooter => draw_expand_footer(ui, &painter, store, &geo, &p, row, rect),
                RowKind::FileHeader => file_header(ui, store, &p, metrics, row.file as usize, rect),
                RowKind::Placeholder => placeholder(ui, &painter, store, &p, row, rect),
                RowKind::Block => block_rows.push((i, row, rect)),
            }
        }
        // Hover "+" in the gutter.
        if let Some(h) = hover_hit {
            if cs.drag.is_none() {
                let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + cs.offsets[h.row]), Vec2::new(width, metrics.line_height));
                let x = if geo.mode == DiffViewMode::Split && store.layout.as_ref().unwrap().rows[h.row].kind == RowKind::Pair {
                    (if h.side == DiffSide::Left { 0.0 } else { geo.half() }) + geo.num_width + 2.0
                } else {
                    geo.num_width * 2.0 + 2.0
                };
                plus(&painter, Pos2::new(rect.left() + x, rect.center().y), &p);
            }
        }
        // Inline blocks: real widgets; report their heights.
        let mut changed = false;
        for (i, row, rect) in block_rows {
            let b = store.layout.as_ref().unwrap().blocks[row.a as usize].clone();
            let inner = Rect::from_min_size(rect.min, Vec2::new(width, 4000.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(("block", &b.id)));
            child.set_clip_rect(viewport.translate(origin.to_vec2()));
            let r = egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 6)).show(&mut child, |ui| {
                ui.set_max_width((width - 24.0).min(880.0));
                if let Some(a) = blocks::block(ui, store, &b) {
                    if matches!(a, blocks::BlockAction::OpenDiscuss) {
                        action = CanvasAction::OpenDiscussPanel;
                    }
                }
            });
            let h = r.response.rect.height().ceil().max(8.0);
            let old = cs.block_heights.get(&b.id).copied();
            if old.map(|o| (o - h).abs() > 0.5).unwrap_or(true) {
                cs.block_heights.insert(b.id.clone(), h);
                changed = true;
                // Growth above the viewport would push the content down: keep it still.
                if cs.offsets[i] < viewport.top() {
                    let delta = h - (cs.offsets[i + 1] - cs.offsets[i]);
                    cs.force_scroll = Some(viewport.top() + delta);
                }
            }
        }
        if changed {
            rebuild_offsets(store, cs, metrics);
            // The rows below were placed with the old height: lay the frame out again
            // before it's shown, so a new or resized block never flickers into place.
            ui.ctx().request_discard("inline block height changed");
        }

        // Sticky header for the file under the top edge.
        if let Some(l) = &store.layout {
            let top = row_at(cs, viewport.top());
            let f = l.file_index_for_row(top);
            let hdr_row = l.file_start_row[f];
            if cs.offsets[hdr_row] < viewport.top() - 0.5 && l.rows[hdr_row].kind == RowKind::FileHeader {
                let next_start = l.file_start_row.get(f + 1).map(|&r| cs.offsets[r]).unwrap_or(content_h);
                let y = (viewport.top()).min(next_start - metrics.file_header_height);
                let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, metrics.file_header_height));
                file_header(ui, store, &p, metrics, f, rect);
            }
        }

        selection_bar(ui, store, cs, &geo, origin, &mut action);
    });
    cs.scroll_y = out.state.offset.y;
    action
}

// MARK: - Geometry

fn rebuild_offsets(store: &PRStore, cs: &mut CanvasState, m: &DiffMetrics) {
    let Some(l) = &store.layout else {
        cs.offsets = vec![0.0];
        return;
    };
    let mut o = Vec::with_capacity(l.rows.len() + 1);
    let mut y = 0.0f32;
    for r in &l.rows {
        o.push(y);
        y += match r.kind {
            RowKind::Line | RowKind::Pair | RowKind::Expanded => m.line_height,
            RowKind::FileHeader => m.file_header_height,
            RowKind::HunkHeader | RowKind::ExpandFooter => m.hunk_header_height,
            RowKind::Placeholder => m.placeholder_height,
            RowKind::Block => {
                let b = &l.blocks[r.a as usize];
                cs.block_heights.get(&b.id).copied().unwrap_or_else(|| estimated_height(store, b))
            }
        };
    }
    o.push(y);
    cs.offsets = o;
}

fn estimated_height(store: &PRStore, b: &InlineBlockRef) -> f32 {
    match b.kind {
        InlineBlockKind::Discuss => 46.0,
        InlineBlockKind::Composer => 200.0,
        InlineBlockKind::Finding => 150.0,
        InlineBlockKind::ReviewThread => match store.threads.get(&b.id) {
            Some(t) if t.is_resolved => 46.0,
            Some(t) => 80.0 + t.comments.len() as f32 * 80.0,
            None => 120.0,
        },
    }
}

fn max_code_chars(store: &PRStore) -> usize {
    let Some(d) = &store.diff else { return 0 };
    d.files.iter().flat_map(|f| f.lines.iter()).map(|l| (l.text_length as usize).min(4000)).max().unwrap_or(0)
}

/// Row containing y (binary search over the prefix sums).
fn row_at(cs: &CanvasState, y: f32) -> usize {
    let n = cs.offsets.len().saturating_sub(1);
    if n == 0 || y <= 0.0 {
        return 0;
    }
    if y >= cs.offsets[n] {
        return n - 1;
    }
    let i = cs.offsets.partition_point(|&o| o <= y);
    i.saturating_sub(1).min(n - 1)
}

fn side_lines(store: &PRStore, row: LayoutRow) -> (Option<SideLine>, Option<SideLine>, Option<SideLine>) {
    let Some(diff) = &store.diff else { return (None, None, None) };
    let f = &diff.files[row.file as usize];
    match row.kind {
        RowKind::Line => {
            let l = f.lines[row.a as usize];
            (None, None, Some(SideLine { kind: Some(l.kind), number: 0, line_index: Some(row.a as usize), expanded_new: None }))
        }
        RowKind::Pair => {
            let left = (row.a >= 0).then(|| {
                let l = f.lines[row.a as usize];
                SideLine { kind: Some(l.kind), number: l.old_line as i64, line_index: Some(row.a as usize), expanded_new: None }
            });
            let right = (row.b >= 0).then(|| {
                let l = f.lines[row.b as usize];
                SideLine { kind: Some(l.kind), number: l.new_line as i64, line_index: Some(row.b as usize), expanded_new: None }
            });
            (left, right, None)
        }
        RowKind::Expanded => (
            Some(SideLine { kind: Some(LineKind::Context), number: row.b as i64, line_index: None, expanded_new: Some(row.a as i64) }),
            Some(SideLine { kind: Some(LineKind::Context), number: row.a as i64, line_index: None, expanded_new: Some(row.a as i64) }),
            None,
        ),
        _ => (None, None, None),
    }
}

fn hit(store: &PRStore, cs: &CanvasState, geo: &Geo, p: Pos2) -> Option<Hit> {
    let l = store.layout.as_ref()?;
    let diff = store.diff.as_ref()?;
    if p.y < 0.0 || p.y >= *cs.offsets.last()? {
        return None;
    }
    let row = row_at(cs, p.y);
    let r = *l.rows.get(row)?;
    let file = r.file as usize;
    let in_gutter = geo.is_gutter(p.x);
    match r.kind {
        RowKind::Line => {
            let dl = diff.files[file].lines[r.a as usize];
            if dl.kind == LineKind::NoNewline {
                return None;
            }
            let side = if dl.kind == LineKind::Deletion { DiffSide::Left } else { DiffSide::Right };
            Some(Hit { row, file, side, line: dl.line_number(side) as i64, in_gutter })
        }
        RowKind::Pair | RowKind::Expanded => {
            let side = if geo.mode == DiffViewMode::Split { geo.side_at(p.x) } else { DiffSide::Right };
            let (left, right, _) = side_lines(store, r);
            let sl = if side == DiffSide::Left { left } else { right }?;
            if sl.number <= 0 || sl.kind == Some(LineKind::NoNewline) {
                return None;
            }
            Some(Hit { row, file, side, line: sl.number, in_gutter })
        }
        _ => None,
    }
}

// MARK: - Text

/// Display text for a line: tabs expanded to 4 spaces, very long lines clipped.
fn display(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for (n, c) in text.chars().enumerate() {
        if n >= 4000 {
            s.push('…');
            break;
        }
        if c == '\t' { s.push_str("    ") } else { s.push(c) }
    }
    s
}

/// Display column (chars, tabs = 4) of byte offset `b` in `text`.
fn col_of_byte(text: &str, b: usize) -> usize {
    text.char_indices().take_while(|(i, _)| *i < b).map(|(_, c)| if c == '\t' { 4 } else { 1 }).sum()
}

/// Byte offset in `text` for display column `col`.
fn byte_of_col(text: &str, col: usize) -> usize {
    let mut c = 0;
    for (i, ch) in text.char_indices() {
        let w = if ch == '\t' { 4 } else { 1 };
        if c + w / 2 >= col && c >= col.saturating_sub(w - 1) {
            return i;
        }
        c += w;
        if c > col {
            return i + ch.len_utf8();
        }
    }
    text.len()
}

struct LineText {
    text: String,
    tokens: Option<Vec<SyntaxToken>>,
    emphasis: Option<std::ops::Range<usize>>,
    muted: bool,
}

fn partner(store: &PRStore, cs: &mut CanvasState, file: usize, line: usize) -> Option<usize> {
    let map = cs
        .partners
        .entry(file)
        .or_insert_with(|| {
            let f = &store.files()[file];
            let mut map = vec![-1i32; f.lines.len()];
            for p in SplitAligner::align_file(f) {
                if p.left >= 0 && p.right >= 0 && p.left != p.right {
                    map[p.left as usize] = p.right;
                    map[p.right as usize] = p.left;
                }
            }
            Arc::new(map)
        })
        .clone();
    map.get(line).and_then(|&v| (v >= 0).then_some(v as usize))
}

fn line_text(store: &mut PRStore, cs: &mut CanvasState, file: usize, sl: &SideLine) -> Option<LineText> {
    if let Some(n) = sl.expanded_new {
        let path = store.files()[file].path().to_string();
        let text = store.file_content_cache.get(&path)?.get((n - 1).max(0) as usize)?.clone();
        let mut st = LexState::default();
        let toks = Lexer::new(LanguageRegistry::language_for_path(&path)).tokenize(&text, &mut st);
        return Some(LineText { text, tokens: Some(toks), emphasis: None, muted: false });
    }
    let li = sl.line_index?;
    let diff = store.diff.clone()?;
    let f = &diff.files[file];
    let dl = f.lines[li];
    let text = diff.text_of(&dl).to_string();
    if dl.kind == LineKind::NoNewline {
        return Some(LineText { text, tokens: None, emphasis: None, muted: true });
    }
    let mut emphasis = None;
    if dl.kind != LineKind::Context && text.len() < 1000 {
        if let Some(p) = partner(store, cs, file, li) {
            let other = diff.text_of(&f.lines[p]);
            emphasis = if dl.kind == LineKind::Deletion {
                IntralineDiff::compute(&text, other).map(|r| r.old)
            } else {
                IntralineDiff::compute(other, &text).map(|r| r.new)
            };
        }
    }
    let tokens = store.tokens(file).and_then(|t| t.get(li).cloned());
    Some(LineText { text, tokens, emphasis, muted: false })
}

fn line_job(lt: &LineText, font: &FontId, p: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let color = if lt.muted { p.muted_text } else { p.text };
    let mut pos = 0;
    let push = |job: &mut LayoutJob, s: &str, c: Color32| {
        if !s.is_empty() {
            job.append(&display(s), 0.0, TextFormat::simple(font.clone(), c));
        }
    };
    if let Some(toks) = &lt.tokens {
        for t in toks {
            let (s, e) = (t.start as usize, (t.start + t.length) as usize);
            if s < pos || e > lt.text.len() || !lt.text.is_char_boundary(s) || !lt.text.is_char_boundary(e) {
                continue;
            }
            push(&mut job, &lt.text[pos..s], color);
            push(&mut job, &lt.text[s..e], syntax_color(t.kind, p));
            pos = e;
        }
    }
    push(&mut job, &lt.text[pos..], color);
    job
}

// MARK: - Drawing

#[allow(clippy::too_many_arguments)]
fn draw_code_row(
    ui: &Ui,
    painter: &egui::Painter,
    store: &mut PRStore,
    cs: &mut CanvasState,
    geo: &Geo,
    m: &DiffMetrics,
    p: &Palette,
    row: LayoutRow,
    rect: Rect,
) {
    let file = row.file as usize;
    let sel = store.selection.clone();
    let font = m.font();
    let h = rect.height();
    let colors = |kind: Option<LineKind>, selected: bool| -> (Color32, Color32, Option<Color32>) {
        if selected {
            return (p.selected_bg, p.selected_num_bg, None);
        }
        match kind {
            Some(LineKind::Addition) => (p.add_bg, p.add_num_bg, Some(p.add_word)),
            Some(LineKind::Deletion) => (p.del_bg, p.del_num_bg, Some(p.del_word)),
            None => (p.empty_bg, p.empty_bg, None),
            _ => (p.canvas, p.canvas, None),
        }
    };
    let number = |n: i64, right: f32| {
        if n > 0 {
            painter.text(Pos2::new(rect.left() + right - 10.0, rect.center().y), Align2::RIGHT_CENTER, n.to_string(), font.clone(), p.muted_text);
        }
    };
    let marker = |kind: Option<LineKind>, x: f32| {
        let (s, c) = match kind {
            Some(LineKind::Addition) => ("+", p.add_marker),
            Some(LineKind::Deletion) => ("-", p.del_marker),
            _ => return,
        };
        painter.text(Pos2::new(rect.left() + x + 7.0, rect.center().y), Align2::LEFT_CENTER, s, font.clone(), c);
    };
    let h_off = cs.h_offset;
    let draw_text = |lt: &LineText, x: f32, clip_w: f32, emph: Option<Color32>, highlight: Option<std::ops::Range<usize>>| {
        let clip = Rect::from_min_size(Pos2::new(rect.left() + x, rect.top()), Vec2::new(clip_w.max(0.0), h)).intersect(painter.clip_rect());
        let pt = painter.with_clip_rect(clip);
        let ox = rect.left() + x + 8.0 - h_off;
        let cw = m.char_width;
        if let Some(hl) = highlight {
            let len = lt.text.len();
            let x0 = col_of_byte(&lt.text, hl.start.min(len)) as f32 * cw;
            let mut x1 = col_of_byte(&lt.text, hl.end.min(len)) as f32 * cw;
            if hl.end > len {
                x1 += cw / 2.0;
            }
            pt.rect_filled(Rect::from_min_size(Pos2::new(ox + x0, rect.top()), Vec2::new((x1 - x0).max(0.0), h)), 0.0, p.text_selection);
        }
        if let (Some(c), Some(e)) = (emph, &lt.emphasis) {
            let x0 = col_of_byte(&lt.text, e.start) as f32 * cw;
            let x1 = col_of_byte(&lt.text, e.end) as f32 * cw;
            pt.rect_filled(Rect::from_min_size(Pos2::new(ox + x0, rect.top() + 1.0), Vec2::new(x1 - x0, h - 2.0)), 2.0, c);
        }
        let galley = ui.fonts_mut(|f| f.layout_job(line_job(lt, &font, p)));
        pt.galley(Pos2::new(ox, rect.center().y - galley.size().y / 2.0), galley, p.text);
    };

    let (left, right, unified) = side_lines(store, row);
    if let Some(u) = unified {
        let li = u.line_index.unwrap();
        let dl = store.files()[file].lines[li];
        let side = if dl.kind == LineKind::Deletion { DiffSide::Left } else { DiffSide::Right };
        let n = dl.line_number(side) as i64;
        let selected = sel.as_ref().map(|s| s.file == file && s.contains(side, n)).unwrap_or(false)
            || (dl.kind == LineKind::Context && sel.as_ref().map(|s| s.file == file && s.contains(DiffSide::Left, dl.old_line as i64)).unwrap_or(false));
        // A text selection highlights characters; the gutter still marks the lines.
        let partial = selected && sel.as_ref().map(|s| s.is_partial()).unwrap_or(false);
        let (bg, num, word) = colors(Some(dl.kind), selected && !partial);
        painter.rect_filled(rect, 0.0, bg);
        painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(geo.num_width * 2.0, h)), 0.0, if partial { p.selected_num_bg } else { num });
        number(dl.old_line as i64, geo.num_width);
        number(dl.new_line as i64, geo.num_width * 2.0);
        marker(Some(dl.kind), geo.num_width * 2.0);
        let sel_line = if sel.as_ref().map(|s| s.side) == Some(DiffSide::Left) { dl.old_line as i64 } else { dl.new_line as i64 };
        if let Some(lt) = line_text(store, cs, file, &u) {
            let hl = if partial { sel.as_ref().and_then(|s| s.column_range(sel_line)) } else { None };
            draw_text(&lt, geo.code_x(DiffSide::Right), geo.code_width(DiffSide::Right), word, hl);
        }
        return;
    }
    for side in [DiffSide::Left, DiffSide::Right] {
        let sl = if side == DiffSide::Left { left } else { right };
        let (x0, half_w) = if geo.mode == DiffViewMode::Split {
            if side == DiffSide::Left { (0.0, geo.half()) } else { (geo.half(), geo.width - geo.half()) }
        } else {
            // Expanded row in unified mode: one combined line.
            if side == DiffSide::Left {
                continue;
            }
            (0.0, geo.width)
        };
        let n = sl.map(|s| s.number).unwrap_or(0);
        let selected = sl.is_some() && sel.as_ref().map(|s| s.file == file && s.contains(side, n)).unwrap_or(false);
        let partial = selected && sel.as_ref().map(|s| s.is_partial()).unwrap_or(false);
        let (bg, num, word) = colors(sl.and_then(|s| s.kind), selected && !partial);
        let num_c = if partial { p.selected_num_bg } else { num };
        painter.rect_filled(Rect::from_min_size(Pos2::new(rect.left() + x0, rect.top()), Vec2::new(half_w, h)), 0.0, bg);
        if geo.mode == DiffViewMode::Unified {
            painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(geo.num_width * 2.0, h)), 0.0, num_c);
            number(left.map(|s| s.number).unwrap_or(0), geo.num_width);
            number(right.map(|s| s.number).unwrap_or(0), geo.num_width * 2.0);
        } else {
            painter.rect_filled(Rect::from_min_size(Pos2::new(rect.left() + x0, rect.top()), Vec2::new(geo.num_width, h)), 0.0, num_c);
            number(n, x0 + geo.num_width);
            marker(sl.and_then(|s| s.kind), x0 + geo.num_width);
        }
        let Some(sl) = sl else { continue };
        if let Some(lt) = line_text(store, cs, file, &sl) {
            let hl = if partial { sel.as_ref().and_then(|s| s.column_range(n)) } else { None };
            draw_text(&lt, geo.code_x(side), geo.code_width(side), word, hl);
        }
    }
    if geo.mode == DiffViewMode::Split {
        painter.rect_filled(Rect::from_min_size(Pos2::new(rect.left() + geo.half() - 0.5, rect.top()), Vec2::new(1.0, h)), 0.0, p.muted_border);
    }
}

fn arrow(painter: &egui::Painter, center: Pos2, up: bool, color: Color32) {
    // ⤒ / ⤓: a bar and a chevron (the bundled fonts don't have the glyphs).
    let s = 4.0;
    let (bar_y, tip_y, base_y) = if up { (center.y - 5.0, center.y - 2.0, center.y + 2.5) } else { (center.y + 5.0, center.y + 2.0, center.y - 2.5) };
    let stroke = Stroke::new(1.6, color);
    painter.line_segment([Pos2::new(center.x - s, bar_y), Pos2::new(center.x + s, bar_y)], stroke);
    painter.line_segment([Pos2::new(center.x - s, base_y), Pos2::new(center.x, tip_y)], stroke);
    painter.line_segment([Pos2::new(center.x, tip_y), Pos2::new(center.x + s, base_y)], stroke);
    painter.line_segment([Pos2::new(center.x, tip_y), Pos2::new(center.x, base_y + if up { 3.0 } else { -3.0 })], stroke);
}

#[allow(clippy::too_many_arguments)]
fn draw_hunk_header(ui: &Ui, painter: &egui::Painter, store: &mut PRStore, geo: &Geo, m: &DiffMetrics, p: &Palette, row: LayoutRow, rect: Rect) {
    let (file, hunk) = (row.file as usize, row.a as usize);
    painter.rect_filled(rect, 0.0, p.hunk_bg);
    let gw = geo.gutter_width();
    let gutter = Rect::from_min_size(rect.min, Vec2::new(gw, rect.height()));
    painter.rect_filled(gutter, 0.0, p.hunk_num_bg);
    if store.can_expand_above(file, hunk) {
        let resp = ui.interact(Rect::from_min_size(rect.min, Vec2::new(gw + geo.marker_width, rect.height())), ui.id().with(("expand_above", file, hunk)), Sense::click());
        let c = if resp.hovered() { p.accent } else { p.muted_text };
        if resp.hovered() {
            painter.rect_filled(gutter, 0.0, p.accent.gamma_multiply(0.2));
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        arrow(painter, gutter.center(), true, c);
        if resp.clicked() {
            store.expand_above(file, hunk, 20);
        }
        resp.on_hover_text("Expand up");
    }
    let text = store.files()[file].hunks[hunk].header_text();
    painter.text(Pos2::new(rect.left() + gw + geo.marker_width + 8.0, rect.center().y), Align2::LEFT_CENTER, text, m.font(), p.muted_text);
}

fn draw_expand_footer(ui: &Ui, painter: &egui::Painter, store: &mut PRStore, geo: &Geo, p: &Palette, row: LayoutRow, rect: Rect) {
    let file = row.file as usize;
    painter.rect_filled(rect, 0.0, p.hunk_bg);
    let gw = geo.gutter_width();
    let gutter = Rect::from_min_size(rect.min, Vec2::new(gw, rect.height()));
    painter.rect_filled(gutter, 0.0, p.hunk_num_bg);
    if !store.can_expand_below(file) {
        return;
    }
    let resp = ui.interact(Rect::from_min_size(rect.min, Vec2::new(gw + geo.marker_width, rect.height())), ui.id().with(("expand_below", file)), Sense::click());
    if resp.hovered() {
        painter.rect_filled(gutter, 0.0, p.accent.gamma_multiply(0.2));
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    arrow(painter, gutter.center(), false, if resp.hovered() { p.accent } else { p.muted_text });
    if resp.clicked() {
        store.expand_below(file, 20);
    }
    resp.on_hover_text("Expand down");
}

fn placeholder(ui: &Ui, painter: &egui::Painter, store: &mut PRStore, p: &Palette, row: LayoutRow, rect: Rect) {
    let file = row.file as usize;
    let n = store.files().get(file).map(|f| f.changed_line_count()).unwrap_or(0);
    let reason = PlaceholderReason::from_raw(row.a).unwrap_or(PlaceholderReason::Large);
    let msg = match reason {
        PlaceholderReason::Binary => "Binary file not shown.".to_string(),
        PlaceholderReason::Large => format!("Large diffs are not rendered by default ({n} lines changed)."),
        PlaceholderReason::Generated => "Some generated files are not rendered by default.".into(),
        PlaceholderReason::DeletedOnly => "This file was deleted.".into(),
        PlaceholderReason::Empty => "File mode changed or empty file.".into(),
    };
    painter.rect_filled(rect, 0.0, p.canvas);
    let can_load = matches!(reason, PlaceholderReason::Large | PlaceholderReason::Generated);
    let y = if can_load { rect.center().y - 9.0 } else { rect.center().y };
    painter.text(Pos2::new(rect.center().x, y), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), p.muted_text);
    if can_load {
        let link = Rect::from_center_size(Pos2::new(rect.center().x, rect.center().y + 12.0), Vec2::new(80.0, 20.0));
        let resp = ui.interact(link, ui.id().with(("load", file)), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        painter.text(link.center(), Align2::CENTER_CENTER, "Load diff", FontId::proportional(14.0), p.accent);
        if resp.clicked() {
            store.force_load(file);
        }
    }
}

fn plus(painter: &egui::Painter, c: Pos2, p: &Palette) {
    let r = Rect::from_center_size(c, Vec2::splat(20.0));
    painter.rect_filled(r, CornerRadius::same(5), p.accent);
    let s = Stroke::new(1.6, Color32::WHITE);
    painter.line_segment([Pos2::new(c.x - 4.5, c.y), Pos2::new(c.x + 4.5, c.y)], s);
    painter.line_segment([Pos2::new(c.x, c.y - 4.5), Pos2::new(c.x, c.y + 4.5)], s);
}

/// File header bar: chevron, status, path (copy), diffstat, open threads, Viewed toggle.
pub fn file_header(ui: &mut Ui, store: &mut PRStore, p: &Palette, m: &DiffMetrics, file: usize, rect: Rect) {
    let Some(f) = store.files().get(file).cloned() else { return };
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, p.subtle);
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    // Block clicks from reaching the code underneath (sticky header).
    let bg = ui.interact(rect, ui.id().with(("hdr", file, rect.top() as i32)), Sense::click());
    let mid = rect.center().y;
    let collapsed = store.options.collapsed.contains(&file);
    let mut x = rect.left() + 12.0;
    // Chevron.
    let chev = Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.top()), Vec2::new(30.0, rect.height()));
    let cr = ui.interact(chev, ui.id().with(("chev", file, rect.top() as i32)), Sense::click());
    let cc = if cr.hovered() { p.text } else { p.muted_text };
    let s = Stroke::new(1.6, cc);
    let cx = x + 4.0;
    if collapsed {
        painter.line_segment([Pos2::new(cx - 2.0, mid - 4.0), Pos2::new(cx + 2.0, mid)], s);
        painter.line_segment([Pos2::new(cx + 2.0, mid), Pos2::new(cx - 2.0, mid + 4.0)], s);
    } else {
        painter.line_segment([Pos2::new(cx - 4.0, mid - 2.0), Pos2::new(cx, mid + 2.0)], s);
        painter.line_segment([Pos2::new(cx, mid + 2.0), Pos2::new(cx + 4.0, mid - 2.0)], s);
    }
    if cr.clicked() || bg.double_clicked() {
        store.toggle_collapsed(file);
    }
    x += 22.0;
    let status = match f.status {
        FileStatus::Added => Some(("+", p.add_marker)),
        FileStatus::Deleted => Some(("−", p.del_marker)),
        FileStatus::Renamed | FileStatus::Copied => Some(("→", p.attention)),
        FileStatus::Modified => None,
    };
    if let Some((s, c)) = status {
        let r = Rect::from_center_size(Pos2::new(x + 6.0, mid), Vec2::splat(12.0));
        painter.rect_stroke(r, 2.0, Stroke::new(1.2, c), egui::StrokeKind::Inside);
        painter.text(r.center(), Align2::CENTER_CENTER, s, FontId::proportional(11.0), c);
        x += 20.0;
    }
    // Right side: Viewed toggle, then the open-thread count.
    let viewed = store.is_viewed(file);
    let vw = 86.0;
    let vrect = Rect::from_min_size(Pos2::new(rect.right() - vw - 12.0, mid - 12.0), Vec2::new(vw, 24.0));
    let vr = ui.interact(vrect, ui.id().with(("viewed", file, rect.top() as i32)), Sense::click());
    let vfill = if viewed { p.accent.gamma_multiply(0.12) } else if vr.hovered() { p.border.gamma_multiply(0.35) } else { Color32::TRANSPARENT };
    painter.rect(vrect, CornerRadius::same(6), vfill, Stroke::new(1.0, if viewed { p.accent.gamma_multiply(0.6) } else { p.border }), egui::StrokeKind::Inside);
    let bx = Rect::from_center_size(Pos2::new(vrect.left() + 14.0, mid), Vec2::splat(12.0));
    if viewed {
        painter.rect_filled(bx, 2.5, p.accent);
        let cs = Stroke::new(1.6, Color32::WHITE);
        painter.line_segment([Pos2::new(bx.left() + 2.5, mid), Pos2::new(bx.left() + 5.0, mid + 2.5)], cs);
        painter.line_segment([Pos2::new(bx.left() + 5.0, mid + 2.5), Pos2::new(bx.right() - 2.0, mid - 3.0)], cs);
    } else {
        painter.rect_stroke(bx, 2.5, Stroke::new(1.2, p.muted_text), egui::StrokeKind::Inside);
    }
    painter.text(Pos2::new(vrect.left() + 26.0, mid), Align2::LEFT_CENTER, "Viewed", FontId::proportional(13.0), p.text);
    if vr.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if vr.clicked() {
        store.toggle_viewed(file);
    }
    let mut right = vrect.left() - 14.0;
    if let Some(&n) = store.open_thread_count_by_path.get(f.path()) {
        if n > 0 {
            let g = painter.layout_no_wrap(n.to_string(), FontId::proportional(12.0), p.muted_text);
            right -= g.size().x;
            painter.galley(Pos2::new(right, mid - g.size().y / 2.0), g, p.muted_text);
            right -= 16.0;
            super::icons::paint(&painter, Rect::from_min_size(Pos2::new(right, mid - 6.0), Vec2::splat(12.0)), super::icons::Icon::Comment, p.muted_text);
            right -= 10.0;
        }
    }
    // Path: dimmed directory + file name, truncated from the head.
    let mono = m.font();
    let path = f.path();
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let mut prefix = String::new();
    if matches!(f.status, FileStatus::Renamed | FileStatus::Copied) && f.old_path != f.new_path {
        prefix = format!("{} → ", f.old_path);
    }
    let stat_w = 140.0;
    let max_w = (right - x - stat_w - 30.0).max(60.0);
    let max_chars = (max_w / m.char_width) as usize;
    let full: String = format!("{prefix}{dir}{name}");
    let shown_chars = full.chars().count();
    let (muted_part, name_part) = if shown_chars > max_chars && max_chars > 4 {
        let keep = max_chars - 1;
        let tail: String = full.chars().skip(shown_chars - keep).collect();
        let name_len = name.chars().count().min(keep);
        let split = tail.chars().count() - name_len;
        (format!("…{}", tail.chars().take(split).collect::<String>()), tail.chars().skip(split).collect::<String>())
    } else {
        (format!("{prefix}{dir}"), name.to_string())
    };
    let mut job = LayoutJob::default();
    job.append(&muted_part, 0.0, TextFormat::simple(mono.clone(), p.muted_text));
    job.append(&name_part, 0.0, TextFormat::simple(mono.clone(), p.text));
    let g = ui.fonts_mut(|fo| fo.layout_job(job));
    let gw = g.size().x;
    painter.galley(Pos2::new(x, mid - g.size().y / 2.0), g, p.text);
    x += gw + 8.0;
    // Copy path.
    let copy = Rect::from_center_size(Pos2::new(x + 6.0, mid), Vec2::splat(18.0));
    let cpr = ui.interact(copy, ui.id().with(("copy", file, rect.top() as i32)), Sense::click());
    let cc = if cpr.hovered() { p.accent } else { p.muted_text };
    painter.rect_stroke(Rect::from_center_size(copy.center() + Vec2::new(1.5, 1.5), Vec2::splat(8.0)), 1.5, Stroke::new(1.1, cc), egui::StrokeKind::Inside);
    painter.rect_stroke(Rect::from_center_size(copy.center() - Vec2::new(1.5, 1.5), Vec2::splat(8.0)), 1.5, Stroke::new(1.1, cc), egui::StrokeKind::Inside);
    if cpr.clicked() {
        ui.ctx().copy_text(path.to_string());
        store.banner = Some(format!("Copied {path}"));
    }
    cpr.on_hover_text("Copy path");
    x += 22.0;
    // Diffstat and the five squares.
    let mut job = LayoutJob::default();
    job.append(&format!("+{}", f.additions), 0.0, TextFormat::simple(FontId::proportional(13.0), p.add_marker));
    job.append(&format!(" −{}", f.deletions), 0.0, TextFormat::simple(FontId::proportional(13.0), p.del_marker));
    let g = ui.fonts_mut(|fo| fo.layout_job(job));
    let sw = g.size().x;
    painter.galley(Pos2::new(x, mid - g.size().y / 2.0), g, p.text);
    x += sw + 6.0;
    let total = (f.additions + f.deletions).max(1) as f32;
    let greens = (f.additions as f32 / total * 5.0).round() as usize;
    for i in 0..5 {
        let r = Rect::from_min_size(Pos2::new(x + i as f32 * 9.0, mid - 4.0), Vec2::splat(8.0));
        painter.rect_filled(r, 1.5, if i < greens { p.add_marker } else if f.additions + f.deletions == 0 { p.border } else { p.del_marker });
    }
}

// MARK: - Mouse

#[allow(clippy::too_many_arguments)]
fn handle_mouse(
    ui: &Ui,
    store: &mut PRStore,
    cs: &mut CanvasState,
    geo: &Geo,
    m: &DiffMetrics,
    resp: &egui::Response,
    pointer: Option<Pos2>,
    _action: &mut CanvasAction,
) {
    let (pressed, released, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.modifiers.shift));
    let Some(pos) = pointer else { return };
    if pressed && resp.hovered() {
        let now = Instant::now();
        let count = match cs.last_press {
            Some((t, p0, n)) if now.duration_since(t).as_millis() < 400 && p0.distance(pos) < 5.0 => n + 1,
            _ => 1,
        };
        cs.last_press = Some((now, pos, count));
        mouse_down(store, cs, geo, m, pos, shift, count);
        store.wants_focus = true;
    } else if resp.dragged() && cs.drag.is_some() {
        mouse_drag(store, cs, geo, m, pos);
        // Autoscroll near the edges.
        let clip = ui.clip_rect();
        let screen = ui.input(|i| i.pointer.interact_pos()).unwrap_or_default();
        if screen.y < clip.top() + 20.0 {
            ui.scroll_with_delta(Vec2::new(0.0, 12.0));
        } else if screen.y > clip.bottom() - 20.0 {
            ui.scroll_with_delta(Vec2::new(0.0, -12.0));
        }
    }
    if released && cs.drag.is_some() {
        let plain_click = cs.drag.map(|d| d.text).unwrap_or(false) && store.is_empty_text_selection();
        cs.drag = None;
        if plain_click {
            store.clear_selection();
            cs.bar_point = None;
        } else {
            cs.bar_point = Some(pos);
        }
    }
}

fn column_at(store: &mut PRStore, cs: &mut CanvasState, geo: &Geo, m: &DiffMetrics, p: Pos2, row: usize, side: DiffSide) -> Option<usize> {
    let r = store.layout.as_ref()?.rows[row];
    let (left, right, unified) = side_lines(store, r);
    let sl = unified.or(if geo.mode == DiffViewMode::Unified { right } else if side == DiffSide::Left { left } else { right })?;
    let text = line_text(store, cs, r.file as usize, &sl)?.text;
    let code_side = if unified.is_some() || geo.mode == DiffViewMode::Unified { DiffSide::Right } else { side };
    let x = p.x - (geo.code_x(code_side) + 8.0 - cs.h_offset);
    if x <= 0.0 {
        return Some(0);
    }
    let col = (x / m.char_width).round() as usize;
    Some(byte_of_col(&text, col))
}

fn mouse_down(store: &mut PRStore, cs: &mut CanvasState, geo: &Geo, m: &DiffMetrics, p: Pos2, extend: bool, clicks: u32) {
    let Some(h) = hit(store, cs, geo, p) else {
        if store.selection.is_some() {
            store.clear_selection();
            cs.bar_point = None;
        }
        return;
    };
    // Gutter clicks select lines; clicks in the code start a text selection.
    // A triple click selects the line, like a text editor.
    let column = if h.in_gutter || clicks >= 3 { None } else { column_at(store, cs, geo, m, p, h.row, h.side) };
    let path = store.files()[h.file].path().to_string();
    if clicks == 2 && !extend {
        if let (Some(col), Some(text)) = (column, store.lines_for(&CodeAnchor::new(path, h.side, h.line, h.line)).first().cloned()) {
            // Double click selects the token under the pointer.
            let w = word_range(&text, col);
            store.select(h.file, h.side, h.line, false, Some(w.start));
            store.select(h.file, h.side, h.line, true, Some(w.end));
            cs.drag = Some(DragAnchor { file: h.file, side: h.side, text: true });
            cs.bar_point = None;
            return;
        }
    }
    store.select(h.file, h.side, h.line, extend, column);
    cs.drag = Some(DragAnchor { file: h.file, side: h.side, text: column.is_some() });
    cs.bar_point = None;
}

fn mouse_drag(store: &mut PRStore, cs: &mut CanvasState, geo: &Geo, m: &DiffMetrics, p: Pos2) {
    let Some(a) = cs.drag else { return };
    let Some(h) = hit(store, cs, geo, p) else { return };
    if h.file != a.file {
        return;
    }
    let line = if h.side == a.side {
        h.line
    } else {
        let r = store.layout.as_ref().unwrap().rows[h.row];
        let (l, rt, u) = side_lines(store, r);
        let sl = if a.side == DiffSide::Left { l.or(u) } else { rt.or(u) };
        match sl {
            Some(s) if s.number > 0 => s.number,
            _ => return,
        }
    };
    let column = if a.text { column_at(store, cs, geo, m, p, h.row, a.side) } else { None };
    store.select(a.file, a.side, line, true, column);
}

/// Byte range of the identifier at `col` (letters, digits, `_`, `$`); a single
/// character when the click lands on anything else.
pub fn word_range(text: &str, col: usize) -> std::ops::Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // A click past the end of the line belongs to the last character.
    let i = chars.iter().rposition(|(b, _)| *b <= col.min(text.len().saturating_sub(1))).unwrap_or(0);
    let end_of = |k: usize| chars.get(k + 1).map(|c| c.0).unwrap_or(text.len());
    if !is_word(chars[i].1) {
        return chars[i].0..end_of(i);
    }
    let (mut a, mut b) = (i, i);
    while a > 0 && is_word(chars[a - 1].1) {
        a -= 1;
    }
    while b + 1 < chars.len() && is_word(chars[b + 1].1) {
        b += 1;
    }
    chars[a].0..end_of(b)
}

/// Floating Comment / Discuss bar under a selection.
fn selection_bar(ui: &mut Ui, store: &mut PRStore, cs: &CanvasState, geo: &Geo, origin: Pos2, action: &mut CanvasAction) {
    if cs.drag.is_some() || store.is_empty_text_selection() {
        return;
    }
    let Some(sel) = store.selection.clone() else { return };
    let Some(l) = &store.layout else { return };
    let Some(row) = l.row(sel.file, sel.side, sel.start.max(sel.end)) else { return };
    let mut pos = Pos2::new(origin.x + geo.code_x(sel.side) + 8.0, origin.y + cs.offsets[row + 1] + 2.0);
    if let Some(bp) = cs.bar_point {
        let r = row_at(cs, bp.y);
        pos = Pos2::new(origin.x + (bp.x - 12.0).clamp(4.0, (geo.width - 260.0).max(4.0)), origin.y + cs.offsets[r + 1] + 2.0);
    }
    let count = (sel.end - sel.start).abs() + 1;
    let p = Palette::current(ui.ctx());
    egui::Area::new(ui.id().with("selection_bar")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).corner_radius(8).inner_margin(egui::Margin::same(5)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if super::common::gh_button(ui, "💬 Comment", super::common::BtnKind::Primary, true).on_hover_text("Comment (C)").clicked() {
                    store.start_comment(None, "", None);
                }
                if super::common::gh_button(ui, "✦ Discuss", super::common::BtnKind::Ai, true).on_hover_text("Ask Claude about this (D)").clicked() {
                    store.start_discuss(None, None);
                    *action = CanvasAction::OpenDiscussPanel;
                }
                ui.label(egui::RichText::new(if count == 1 { "1 line".to_string() } else { format!("{count} lines") }).size(12.0).color(p.muted_text));
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_ranges() {
        assert_eq!(word_range("let foo_bar = 1", 6), 4..11);
        assert_eq!(word_range("let foo_bar = 1", 12), 12..13);
        assert_eq!(word_range("abc", 99), 0..3);
        assert_eq!(word_range("", 0), 0..0);
        assert_eq!(word_range("é_x y", 0), 0..4);
    }

    #[test]
    fn geometry_columns() {
        let u = Geo { mode: DiffViewMode::Unified, num_width: 50.0, marker_width: 22.0, width: 1000.0 };
        assert_eq!(u.code_x(DiffSide::Right), 122.0);
        assert_eq!(u.code_width(DiffSide::Left), 878.0);
        assert!(u.is_gutter(121.0) && !u.is_gutter(123.0));
        assert_eq!(u.side_at(900.0), DiffSide::Left);
        let s = Geo { mode: DiffViewMode::Split, ..u };
        assert_eq!(s.half(), 500.0);
        assert_eq!(s.code_x(DiffSide::Left), 72.0);
        assert_eq!(s.code_x(DiffSide::Right), 572.0);
        assert_eq!(s.code_width(DiffSide::Right), 428.0);
        assert_eq!(s.side_at(499.0), DiffSide::Left);
        assert_eq!(s.side_at(500.0), DiffSide::Right);
        assert!(s.is_gutter(560.0) && !s.is_gutter(580.0));
        assert_eq!(s.gutter_width(), 50.0);
        assert_eq!(u.gutter_width(), 100.0);
    }

    #[test]
    fn row_lookup_by_y() {
        let cs = CanvasState { offsets: vec![0.0, 38.0, 62.0, 82.0, 282.0], ..Default::default() };
        assert_eq!(row_at(&cs, -5.0), 0);
        assert_eq!(row_at(&cs, 0.0), 0);
        assert_eq!(row_at(&cs, 37.9), 0);
        assert_eq!(row_at(&cs, 38.0), 1);
        assert_eq!(row_at(&cs, 100.0), 3);
        assert_eq!(row_at(&cs, 5000.0), 3);
        let empty = CanvasState { offsets: vec![0.0], ..Default::default() };
        assert_eq!(row_at(&empty, 10.0), 0);
    }

    #[test]
    fn columns_and_tabs() {
        assert_eq!(col_of_byte("\tab", 1), 4);
        assert_eq!(col_of_byte("\tab", 2), 5);
        assert_eq!(byte_of_col("\tab", 4), 1);
        assert_eq!(byte_of_col("abc", 2), 2);
        assert_eq!(byte_of_col("abc", 10), 3);
        assert_eq!(display("\tx"), "    x");
    }
}
