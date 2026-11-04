//! Renders GitHub-flavored Markdown (parsed by the core `MarkdownParser`) with egui.

use std::ops::Range;

use egui::text::LayoutJob;
use egui::{Color32, FontId, RichText, Sense, Stroke, TextFormat, Ui};
use review_buddy_core::highlight::language::LanguageRegistry;
use review_buddy_core::highlight::lexer::{LexState, Lexer, TokenKind};
use review_buddy_core::review::markdown::{MarkdownBlock, MarkdownList, MarkdownParser, MarkdownTable, TableAlignment};

use super::common::{pal, suggestion_preview};
use crate::theme::Palette;

pub struct MdStyle {
    pub size: f32,
}

impl Default for MdStyle {
    fn default() -> Self {
        MdStyle { size: 13.0 }
    }
}

/// Renders `text`; `suggestion_context` holds the lines a ```suggestion block replaces.
pub fn markdown(ui: &mut Ui, text: &str, suggestion_context: &[String]) {
    let blocks = MarkdownParser::parse(text);
    blocks_ui(ui, &blocks, suggestion_context, &MdStyle::default());
}

pub fn markdown_sized(ui: &mut Ui, text: &str, size: f32) {
    let blocks = MarkdownParser::parse(text);
    blocks_ui(ui, &blocks, &[], &MdStyle { size });
}

fn blocks_ui(ui: &mut Ui, blocks: &[MarkdownBlock], ctx: &[String], st: &MdStyle) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        for b in blocks {
            block_ui(ui, b, ctx, st);
        }
    });
}

fn block_ui(ui: &mut Ui, b: &MarkdownBlock, ctx: &[String], st: &MdStyle) {
    let p = pal(ui);
    match b {
        MarkdownBlock::Paragraph(s) => inline(ui, s, st.size, p.text, false),
        MarkdownBlock::Heading { level, text } => {
            let size = match level {
                1 => st.size * 1.6,
                2 => st.size * 1.35,
                3 => st.size * 1.15,
                _ => st.size,
            };
            inline(ui, text, size, p.text, true);
            if *level <= 2 {
                let r = ui.available_rect_before_wrap();
                ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, p.muted_border));
            }
        }
        MarkdownBlock::Code { language, text } => code_block(ui, text, language.as_deref(), &p),
        MarkdownBlock::Suggestion(s) => {
            let new: Vec<String> = if s.is_empty() { vec![] } else { s.split('\n').map(str::to_string).collect() };
            suggestion_preview(ui, ctx, &new);
        }
        MarkdownBlock::Quote(inner) => {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 18.0), Sense::hover());
                let top = r.top();
                let resp = ui.vertical(|ui| {
                    ui.visuals_mut().override_text_color = Some(p.muted_text);
                    blocks_ui(ui, inner, ctx, st)
                });
                let h = resp.response.rect.bottom() - top;
                ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(3.0, h)), 1.5, p.border);
            });
        }
        MarkdownBlock::List(l) => list_ui(ui, l, ctx, st),
        MarkdownBlock::Table(t) => table_ui(ui, t, st),
        MarkdownBlock::Rule => {
            ui.separator();
        }
    }
}

fn list_ui(ui: &mut Ui, l: &MarkdownList, ctx: &[String], st: &MdStyle) {
    let p = pal(ui);
    for (i, item) in l.items.iter().enumerate() {
        ui.horizontal_top(|ui| {
            let marker = match item.checked {
                Some(true) => "☑".to_string(),
                Some(false) => "☐".to_string(),
                None if l.ordered => format!("{}.", l.start + i as u64),
                None => "•".to_string(),
            };
            ui.add_sized([18.0, st.size + 4.0], egui::Label::new(RichText::new(marker).size(st.size).color(p.muted_text)));
            ui.vertical(|ui| blocks_ui(ui, &item.blocks, ctx, st));
        });
    }
}

fn table_ui(ui: &mut Ui, t: &MarkdownTable, st: &MdStyle) {
    let p = pal(ui);
    egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
        egui::Grid::new(ui.next_auto_id()).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            for (c, h) in t.header.iter().enumerate() {
                cell(ui, h, t.alignments.get(c).copied(), st.size, p.text, true);
            }
            ui.end_row();
            for row in &t.rows {
                for c in 0..t.column_count() {
                    let text = row.get(c).map(String::as_str).unwrap_or("");
                    cell(ui, text, t.alignments.get(c).copied(), st.size, p.text, false);
                }
                ui.end_row();
            }
        });
    });
}

fn cell(ui: &mut Ui, text: &str, align: Option<TableAlignment>, size: f32, color: Color32, bold: bool) {
    let layout = match align {
        Some(TableAlignment::Center) => egui::Layout::top_down(egui::Align::Center),
        Some(TableAlignment::Right) => egui::Layout::top_down(egui::Align::Max),
        _ => egui::Layout::top_down(egui::Align::Min),
    };
    ui.with_layout(layout, |ui| inline(ui, text, size, color, bold));
}

fn code_block(ui: &mut Ui, text: &str, lang: Option<&str>, p: &Palette) {
    egui::Frame::new().fill(p.subtle).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let font = FontId::monospace(12.0);
        let mut job = LayoutJob::default();
        let is_diff = matches!(lang, Some("diff") | Some("patch"));
        let spec = lang.map(|l| LanguageRegistry::language_for_path(&format!("x.{l}")));
        let mut state = LexState::default();
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                job.append("\n", 0.0, TextFormat::simple(font.clone(), p.text));
            }
            if is_diff {
                let c = if line.starts_with('+') {
                    p.add_marker
                } else if line.starts_with('-') {
                    p.del_marker
                } else if line.starts_with("@@") {
                    p.accent
                } else {
                    p.text
                };
                job.append(line, 0.0, TextFormat::simple(font.clone(), c));
                continue;
            }
            match spec {
                Some(spec) => {
                    let toks = Lexer::new(spec).tokenize(line, &mut state);
                    append_tokens(&mut job, line, &toks, &font, p);
                }
                None => job.append(line, 0.0, TextFormat::simple(font.clone(), p.text)),
            }
        }
        egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            ui.add(egui::Label::new(job).extend());
        });
    });
}

/// Appends `line` colored by syntax tokens (byte ranges) to `job`.
pub fn append_tokens(
    job: &mut LayoutJob,
    line: &str,
    toks: &[review_buddy_core::highlight::lexer::SyntaxToken],
    font: &FontId,
    p: &Palette,
) {
    let mut pos = 0;
    for t in toks {
        let (s, e) = (t.start as usize, (t.start + t.length) as usize);
        if s < pos || e > line.len() || !line.is_char_boundary(s) || !line.is_char_boundary(e) {
            continue;
        }
        if s > pos {
            job.append(&line[pos..s], 0.0, TextFormat::simple(font.clone(), p.text));
        }
        job.append(&line[s..e], 0.0, TextFormat::simple(font.clone(), syntax_color(t.kind, p)));
        pos = e;
    }
    if pos < line.len() {
        job.append(&line[pos..], 0.0, TextFormat::simple(font.clone(), p.text));
    }
}

pub fn syntax_color(k: TokenKind, p: &Palette) -> Color32 {
    match k {
        TokenKind::Plain => p.text,
        TokenKind::Keyword => p.syn_keyword,
        TokenKind::Type => p.syn_type,
        TokenKind::String => p.syn_string,
        TokenKind::Number | TokenKind::Constant | TokenKind::Property | TokenKind::Attribute => p.syn_number,
        TokenKind::Comment => p.syn_comment,
        TokenKind::Function => p.syn_function,
        TokenKind::Tag => p.syn_tag,
    }
}

// MARK: - Inline

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
}

/// Inline Markdown (`code`, **bold**, *italic*, ~~strike~~, [links](url), bare URLs) as one
/// wrapped label; clicking a link opens it.
pub fn inline(ui: &mut Ui, s: &str, size: f32, color: Color32, bold: bool) {
    let p = pal(ui);
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let mut links: Vec<(Range<usize>, String)> = vec![];
    let mut chars = 0usize;
    let base = Style { bold, ..Default::default() };
    let mut push = |job: &mut LayoutJob, text: &str, st: Style, link: Option<&str>| {
        if text.is_empty() {
            return;
        }
        let font = if st.code { FontId::monospace(size * 0.92) } else { FontId::proportional(size) };
        let mut f = TextFormat::simple(font, if link.is_some() { p.accent } else { color });
        f.italics = st.italic;
        if st.bold && !st.code {
            f.color = if link.is_some() { p.accent } else { strong(color) };
            f.extra_letter_spacing = 0.15;
        }
        if st.code {
            f.background = p.subtle;
        }
        if st.strike {
            f.strikethrough = Stroke::new(1.0, color);
        }
        if link.is_some() {
            f.underline = Stroke::new(1.0, p.accent.gamma_multiply(0.5));
        }
        let n = text.chars().count();
        if let Some(url) = link {
            links.push((chars..chars + n, url.to_string()));
        }
        chars += n;
        job.append(text, 0.0, f);
    };
    parse_inline(s, base, &mut |text, st, link| push(&mut job, text, st, link));
    let (pos, galley, resp) = egui::Label::new(job).sense(Sense::click()).layout_in_ui(ui);
    let hovered_link = resp.hover_pos().and_then(|hp| {
        let c = galley.cursor_from_pos(hp - pos).index.0;
        links.iter().find(|(r, _)| r.contains(&c)).map(|(_, u)| u.clone())
    });
    if hovered_link.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        if let Some(url) = hovered_link {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
    ui.painter().galley(pos, galley, color);
}

fn strong(c: Color32) -> Color32 {
    c
}

/// Splits inline Markdown into styled runs.
fn parse_inline(s: &str, st: Style, out: &mut dyn FnMut(&str, Style, Option<&str>)) {
    let b = s.as_bytes();
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        let rest = &s[i..];
        // `code`
        if b[i] == b'`' {
            let ticks = rest.bytes().take_while(|&c| c == b'`').count();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                out(&s[start..i], st, None);
                let inner = &rest[ticks..ticks + end];
                out(inner.trim(), Style { code: true, ..st }, None);
                i += ticks + end + ticks;
                start = i;
                continue;
            }
        }
        // **bold** / __bold__
        if rest.starts_with("**") || rest.starts_with("__") {
            let m = &rest[..2];
            if let Some(end) = rest[2..].find(m) {
                if end > 0 {
                    out(&s[start..i], st, None);
                    parse_inline(&rest[2..2 + end], Style { bold: true, ..st }, out);
                    i += 4 + end;
                    start = i;
                    continue;
                }
            }
        }
        // ~~strike~~
        if rest.starts_with("~~") {
            if let Some(end) = rest[2..].find("~~") {
                out(&s[start..i], st, None);
                parse_inline(&rest[2..2 + end], Style { strike: true, ..st }, out);
                i += 4 + end;
                start = i;
                continue;
            }
        }
        // *italic* / _italic_ (not inside words for _)
        if (b[i] == b'*' || (b[i] == b'_' && (i == 0 || !b[i - 1].is_ascii_alphanumeric()))) && i + 1 < b.len() && b[i + 1] != b' ' {
            let m = b[i] as char;
            if let Some(end) = rest[1..].find(m) {
                let after = i + 1 + end + 1;
                let ok = m == '*' || after >= b.len() || !b[after].is_ascii_alphanumeric();
                if end > 0 && ok {
                    out(&s[start..i], st, None);
                    parse_inline(&rest[1..1 + end], Style { italic: true, ..st }, out);
                    i += end + 2;
                    start = i;
                    continue;
                }
            }
        }
        // [text](url) and ![alt](url)
        if b[i] == b'[' || (b[i] == b'!' && rest.starts_with("![")) {
            let off = if b[i] == b'!' { 1 } else { 0 };
            if let Some(close) = rest[off..].find("](") {
                let label = &rest[off + 1..off + close];
                if let Some(paren) = rest[off + close + 2..].find(')') {
                    let url = &rest[off + close + 2..off + close + 2 + paren];
                    if !label.contains(']') && !url.contains(' ') {
                        out(&s[start..i], st, None);
                        let text = if off == 1 { format!("🖼 {label}") } else { label.to_string() };
                        out(&text, st, Some(url));
                        i += off + close + 3 + paren;
                        start = i;
                        continue;
                    }
                }
            }
        }
        // Bare URLs
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let end = rest.find(|c: char| c.is_whitespace() || c == ')' || c == '>').unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':']);
            out(&s[start..i], st, None);
            out(url, st, Some(url));
            i += url.len();
            start = i;
            continue;
        }
        // Hard line breaks inside a paragraph stay line breaks.
        i += rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    out(&s[start..], st, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(s: &str) -> Vec<(String, bool, bool, bool, Option<String>)> {
        let mut v = vec![];
        parse_inline(s, Style::default(), &mut |t, st, l| {
            if !t.is_empty() {
                v.push((t.to_string(), st.bold, st.italic, st.code, l.map(str::to_string)));
            }
        });
        v
    }

    #[test]
    fn inline_runs() {
        let r = runs("a **b** `c` *d* [e](http://x) snake_case_name");
        assert_eq!(r[1], ("b".into(), true, false, false, None));
        assert_eq!(r[3], ("c".into(), false, false, true, None));
        assert_eq!(r[5], ("d".into(), false, true, false, None));
        assert_eq!(r[7], ("e".into(), false, false, false, Some("http://x".into())));
        assert_eq!(r.last().unwrap().0, " snake_case_name");
        let r = runs("see https://github.com/o/r/pull/1.");
        assert_eq!(r[1].4.as_deref(), Some("https://github.com/o/r/pull/1"));
    }
}
