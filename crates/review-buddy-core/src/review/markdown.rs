//! Block-level GitHub-flavored Markdown for comment bodies and Claude's replies. Inline
//! formatting (`code`, **bold**, links) is left to the UI; this handles structure:
//! headings, lists (nested, task), quotes, tables, code fences (including ```suggestion
//! blocks, which GitHub renders as a mini diff) and rules.
//!
//! It must never fail: text streams in a token at a time, so half a table or an
//! unterminated fence has to render as something sensible.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownBlock {
    Paragraph(String),
    Heading { level: usize, text: String },
    Code { language: Option<String>, text: String },
    Suggestion(String),
    Quote(Vec<MarkdownBlock>),
    List(MarkdownList),
    Table(MarkdownTable),
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownList {
    pub ordered: bool,
    /// Number of the first item (ordered lists).
    pub start: u64,
    pub items: Vec<MarkdownListItem>,
}

impl MarkdownList {
    pub fn new(ordered: bool, items: Vec<MarkdownListItem>) -> Self {
        MarkdownList { ordered, start: 1, items }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownListItem {
    /// `None` for a plain item; the checkbox state for a task item (`- [x] …`).
    pub checked: Option<bool>,
    pub blocks: Vec<MarkdownBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TableAlignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownTable {
    pub header: Vec<String>,
    pub alignments: Vec<TableAlignment>,
    /// Every row has exactly `header.len()` cells (short rows padded, long ones cut).
    pub rows: Vec<Vec<String>>,
}

impl MarkdownTable {
    pub fn column_count(&self) -> usize {
        self.header.len()
    }
}

/// Nesting deeper than this (quotes in lists in quotes…) is shown as plain text.
const MAX_DEPTH: usize = 16;

pub struct MarkdownParser;

impl MarkdownParser {
    pub fn parse(text: &str) -> Vec<MarkdownBlock> {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return Vec::new();
        }
        let lines: Vec<String> = normalized.split('\n').map(expand_leading_tabs).collect();
        parse_blocks(&lines, 0)
    }

    /// Text for a ```suggestion block replacing `lines`.
    pub fn suggestion_template<S: AsRef<str>>(lines: &[S]) -> String {
        let body: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
        format!("```suggestion\n{}\n```", body.join("\n"))
    }
}

// MARK: Blocks

fn parse_blocks(lines: &[String], depth: usize) -> Vec<MarkdownBlock> {
    if depth > MAX_DEPTH {
        let text = lines.iter().map(|l| trim_ws(l)).collect::<Vec<_>>().join("\n");
        let text = text.trim();
        return if text.is_empty() { Vec::new() } else { vec![MarkdownBlock::Paragraph(text.to_string())] };
    }
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush_para = |blocks: &mut Vec<MarkdownBlock>, para: &mut Vec<String>| {
        if !para.is_empty() {
            blocks.push(MarkdownBlock::Paragraph(para.join("\n")));
            para.clear();
        }
    };
    let n = lines.len();
    let mut i = 0;
    while i < n {
        let line = lines[i].as_str();
        let indent = leading_spaces(line);
        let t = trim_ws(line);
        if t.is_empty() {
            flush_para(&mut blocks, &mut para);
            i += 1;
            continue;
        }

        if let Some(fence) = opening_fence(t) {
            flush_para(&mut blocks, &mut para);
            let mut body: Vec<&str> = Vec::new();
            i += 1;
            while i < n && !is_closing_fence(&lines[i], &fence) {
                body.push(drop_spaces(&lines[i], indent));
                i += 1;
            }
            i += 1; // closing fence (or past the end when the fence is still streaming)
            let text = body.join("\n");
            if fence.language.as_deref() == Some("suggestion") {
                blocks.push(MarkdownBlock::Suggestion(text));
            } else {
                blocks.push(MarkdownBlock::Code { language: fence.language, text });
            }
            continue;
        }
        if t.starts_with("<!--") {
            // HTML comments (PR templates) are invisible on GitHub; an unclosed one hides the rest.
            flush_para(&mut blocks, &mut para);
            while i < n && !lines[i].contains("-->") {
                i += 1;
            }
            i += 1;
            continue;
        }
        if let Some(h) = heading(t) {
            flush_para(&mut blocks, &mut para);
            blocks.push(h);
            i += 1;
            continue;
        }
        if !para.is_empty() {
            if let Some(level) = setext_level(t) {
                blocks.push(MarkdownBlock::Heading { level, text: para.join(" ") });
                para.clear();
                i += 1;
                continue;
            }
        }
        if is_rule(t) {
            flush_para(&mut blocks, &mut para);
            blocks.push(MarkdownBlock::Rule);
            i += 1;
            continue;
        }
        if t.starts_with('>') {
            flush_para(&mut blocks, &mut para);
            let mut inner: Vec<String> = Vec::new();
            let mut last_blank = false;
            while i < n {
                let qt = trim_ws(&lines[i]);
                if let Some(rest) = qt.strip_prefix('>') {
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    inner.push(rest.to_string());
                    last_blank = trim_ws(rest).is_empty();
                } else if !qt.is_empty() && !last_blank && !starts_block(&lines[i]) {
                    inner.push(qt.to_string()); // lazy continuation of the quoted paragraph
                } else {
                    break;
                }
                i += 1;
            }
            blocks.push(MarkdownBlock::Quote(parse_blocks(&inner, depth + 1)));
            continue;
        }
        if i + 1 < n && t.contains('|') {
            if let Some(aligns) = delimiter_row(&lines[i + 1]) {
                let header = split_cells(t);
                if header.len() == aligns.len() {
                    flush_para(&mut blocks, &mut para);
                    i += 2;
                    let mut rows = Vec::new();
                    while i < n {
                        let rt = trim_ws(&lines[i]);
                        if rt.is_empty() || !rt.contains('|') || starts_block(&lines[i]) {
                            break;
                        }
                        let mut cells = split_cells(rt);
                        cells.resize(header.len(), String::new());
                        rows.push(cells);
                        i += 1;
                    }
                    blocks.push(MarkdownBlock::Table(MarkdownTable { header, alignments: aligns, rows }));
                    continue;
                }
            }
        }
        if let Some(m) = list_marker(line) {
            if para.is_empty() || m.can_interrupt_paragraph() {
                flush_para(&mut blocks, &mut para);
                blocks.push(MarkdownBlock::List(parse_list(lines, &mut i, depth)));
                continue;
            }
        }
        para.push(t.to_string());
        i += 1;
    }
    flush_para(&mut blocks, &mut para);
    blocks
}

/// Consumes one list starting at `lines[*i]`.
fn parse_list(lines: &[String], i: &mut usize, depth: usize) -> MarkdownList {
    let first = list_marker(&lines[*i]).expect("parse_list starts at a list marker");
    let mut items: Vec<MarkdownListItem> = Vec::new();
    let mut cur: Vec<String> = vec![first.content.clone()];
    let mut content_col = first.content_column;
    let mut saw_blank = false;

    fn finish_item(mut cur: Vec<String>, items: &mut Vec<MarkdownListItem>, depth: usize) {
        while cur.last().is_some_and(|l| trim_ws(l).is_empty()) {
            cur.pop();
        }
        let mut checked = None;
        if let Some(head) = cur.first().cloned() {
            for (bx, state) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
                if head == bx || head.starts_with(&format!("{bx} ")) {
                    checked = Some(state);
                    cur[0] = trim_ws(&head[bx.len()..]).to_string();
                    break;
                }
            }
        }
        items.push(MarkdownListItem { checked, blocks: parse_blocks(&cur, depth + 1) });
    }

    *i += 1;
    while *i < lines.len() {
        let line = lines[*i].as_str();
        let t = trim_ws(line);
        if t.is_empty() {
            cur.push(String::new());
            saw_blank = true;
            *i += 1;
            continue;
        }
        let ind = leading_spaces(line);
        if ind <= first.indent + 1 {
            if let Some(m) = list_marker(line).filter(|_| !is_rule(t)) {
                if m.ordered != first.ordered {
                    break;
                }
                finish_item(std::mem::replace(&mut cur, vec![m.content]), &mut items, depth);
                content_col = m.content_column;
                saw_blank = false;
                *i += 1;
                continue;
            }
            // Not indented under the item: a lazy paragraph continuation, unless a blank
            // line or another block ends the list.
            if saw_blank || starts_block(line) || is_table_start(lines, *i) {
                break;
            }
            cur.push(t.to_string());
            *i += 1;
            continue;
        }
        // Indented under the item (nested list, code, continuation paragraph). Models
        // nest with 2, 3 or 4 spaces regardless of the marker width, so be lenient.
        cur.push(drop_spaces(line, ind.min(content_col)).to_string());
        saw_blank = false;
        *i += 1;
    }
    finish_item(cur, &mut items, depth);
    MarkdownList { ordered: first.ordered, start: first.start, items }
}

// MARK: Line classifiers

struct Fence {
    ch: char,
    count: usize,
    language: Option<String>,
}

fn opening_fence(t: &str) -> Option<Fence> {
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let count = t.chars().take_while(|&x| x == c).count();
    if count < 3 {
        return None;
    }
    let info = trim_ws(&t[count..]);
    if c == '`' && info.contains('`') {
        return None; // inline code like ```x```
    }
    let language = info.split(' ').find(|s| !s.is_empty()).map(str::to_string);
    Some(Fence { ch: c, count, language })
}

fn is_closing_fence(line: &str, f: &Fence) -> bool {
    let t = trim_ws(line);
    t.chars().count() >= f.count && t.chars().all(|c| c == f.ch)
}

fn heading(s: &str) -> Option<MarkdownBlock> {
    let level = s.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &s[level..];
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let mut text = trim_ws(rest);
    // Optional closing sequence: "## Title ##".
    if text.ends_with('#') {
        let before = text.trim_end_matches('#');
        if before.is_empty() || before.ends_with(char::is_whitespace) {
            text = trim_ws(before);
        }
    }
    Some(MarkdownBlock::Heading { level, text: text.to_string() })
}

fn setext_level(t: &str) -> Option<usize> {
    if t.chars().count() < 3 {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        return Some(1);
    }
    if t.chars().all(|c| c == '-') {
        return Some(2);
    }
    None
}

fn is_rule(t: &str) -> bool {
    let mut chars = t.chars().filter(|&c| c != ' ');
    let Some(c) = chars.next() else { return false };
    if c != '-' && c != '*' && c != '_' {
        return false;
    }
    let mut count = 1;
    for x in chars {
        if x != c {
            return false;
        }
        count += 1;
    }
    count >= 3
}

struct ListMarker {
    indent: usize,
    ordered: bool,
    start: u64,
    content_column: usize,
    content: String,
}

impl ListMarker {
    /// CommonMark: an empty item, or an ordered list not starting at 1, can't interrupt a paragraph.
    fn can_interrupt_paragraph(&self) -> bool {
        !self.content.is_empty() && (!self.ordered || self.start == 1)
    }
}

fn list_marker(line: &str) -> Option<ListMarker> {
    let indent = leading_spaces(line);
    let s = &line[indent..];
    let c = s.chars().next()?;
    let marker_len;
    let mut ordered = false;
    let mut start = 1;
    if c == '-' || c == '*' || c == '+' {
        marker_len = 1;
    } else {
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) {
            return None;
        }
        let after = s[digits..].chars().next();
        if after != Some('.') && after != Some(')') {
            return None;
        }
        ordered = true;
        start = s[..digits].parse().unwrap_or(1);
        marker_len = digits + 1;
    }
    let rest = &s[marker_len..];
    if let Some(f) = rest.chars().next() {
        if f != ' ' && f != '\t' {
            return None;
        }
    }
    let spaces = if rest.starts_with('\t') { 1 } else { leading_spaces(rest) };
    let content = trim_ws(rest).to_string();
    // 1–4 spaces after the marker set the content column; more means indented content.
    let gap = if content.is_empty() || spaces > 4 { 1 } else { spaces };
    Some(ListMarker { indent, ordered, start, content_column: indent + marker_len + gap, content })
}

/// Whether `line` opens a block that ends a paragraph, list item or table.
fn starts_block(line: &str) -> bool {
    let t = trim_ws(line);
    opening_fence(t).is_some()
        || heading(t).is_some()
        || is_rule(t)
        || t.starts_with('>')
        || t.starts_with("<!--")
        || list_marker(line).is_some_and(|m| m.can_interrupt_paragraph())
}

fn is_table_start(lines: &[String], i: usize) -> bool {
    if i + 1 >= lines.len() || !lines[i].contains('|') {
        return false;
    }
    match delimiter_row(&lines[i + 1]) {
        Some(a) => split_cells(trim_ws(&lines[i])).len() == a.len(),
        None => false,
    }
}

// MARK: Tables

/// `| :--- | :---: | ---: |` → alignments, or `None` if the line isn't a delimiter row.
fn delimiter_row(line: &str) -> Option<Vec<TableAlignment>> {
    let t = trim_ws(line);
    if !(t.contains('|') || t.contains(':')) || !t.contains('-') {
        return None;
    }
    let mut cells: Vec<&str> = t.split('|').map(trim_ws).collect();
    if t.starts_with('|') {
        cells.remove(0);
    }
    if t.ends_with('|') && !cells.is_empty() {
        cells.pop();
    }
    if cells.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(cells.len());
    for c in cells {
        let left = c.starts_with(':');
        let right = c.ends_with(':') && c.chars().count() > 1;
        let dashes = &c[usize::from(left)..c.len() - usize::from(right)];
        if dashes.is_empty() || !dashes.chars().all(|x| x == '-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => TableAlignment::Center,
            (true, false) => TableAlignment::Left,
            (false, true) => TableAlignment::Right,
            (false, false) => TableAlignment::None,
        });
    }
    // A lone `---` is a rule or a setext underline, not a one-column table.
    if !t.contains('|') {
        return None;
    }
    Some(out)
}

/// Cells of a table row. `\|` is a literal pipe; pipes inside a closed `code span` don't
/// split (GitHub would, but models write `a | b` in code and mean one cell).
fn split_cells(row: &str) -> Vec<String> {
    let mut t = trim_ws(row);
    if let Some(r) = t.strip_prefix('|') {
        t = r;
    }
    if t.ends_with('|') && !t.ends_with("\\|") {
        t = &t[..t.len() - 1];
    }
    let chars: Vec<char> = t.chars().collect();
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut k = 0;
    while k < chars.len() {
        let c = chars[k];
        if c == '\\' && k + 1 < chars.len() && chars[k + 1] == '|' {
            cur.push('|');
            k += 2;
            continue;
        }
        if c == '`' {
            let mut run = 0;
            while k + run < chars.len() && chars[k + run] == '`' {
                run += 1;
            }
            match closing_backticks(&chars, k + run, run) {
                Some(close) => {
                    cur.extend(&chars[k..close + run]);
                    k = close + run;
                }
                None => {
                    cur.extend(&chars[k..k + run]);
                    k += run;
                }
            }
            continue;
        }
        if c == '|' {
            cells.push(std::mem::take(&mut cur));
            k += 1;
            continue;
        }
        cur.push(c);
        k += 1;
    }
    cells.push(cur);
    cells.iter().map(|c| clean_cell(c)).collect()
}

fn closing_backticks(chars: &[char], from: usize, run: usize) -> Option<usize> {
    let mut k = from;
    while k < chars.len() {
        if chars[k] == '`' {
            let mut r = 0;
            while k + r < chars.len() && chars[k + r] == '`' {
                r += 1;
            }
            if r == run {
                return Some(k);
            }
            k += r;
        } else {
            k += 1;
        }
    }
    None
}

/// Replaces `<br>`, `<br/>`, `<BR />`… with a newline, then trims.
fn clean_cell(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == '<'
            && chars.get(k + 1).is_some_and(|c| c.eq_ignore_ascii_case(&'b'))
            && chars.get(k + 2).is_some_and(|c| c.eq_ignore_ascii_case(&'r'))
        {
            let mut j = k + 3;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if chars.get(j) == Some(&'/') {
                j += 1;
            }
            if chars.get(j) == Some(&'>') {
                out.push('\n');
                k = j + 1;
                continue;
            }
        }
        out.push(chars[k]);
        k += 1;
    }
    trim_ws(&out).to_string()
}

// MARK: Helpers

/// Swift's `CharacterSet.whitespaces`: whitespace other than line breaks.
fn is_ws(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn trim_ws(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn drop_spaces(s: &str, n: usize) -> &str {
    &s[n.min(leading_spaces(s))..]
}

fn expand_leading_tabs(s: &str) -> String {
    if !(s.starts_with('\t') || (s.starts_with(' ') && s.contains('\t'))) {
        return s.to_string();
    }
    let mut out = String::new();
    let mut idx = 0;
    for b in s.bytes() {
        match b {
            b'\t' => out.push_str(&" ".repeat(4 - out.len() % 4)),
            b' ' => out.push(' '),
            _ => break,
        }
        idx += 1;
    }
    out.push_str(&s[idx..]);
    out
}

#[cfg(test)]
mod tests {
    use super::MarkdownBlock::*;
    use super::*;

    fn parse(s: &str) -> Vec<MarkdownBlock> {
        MarkdownParser::parse(s)
    }
    fn p(s: &str) -> MarkdownBlock {
        Paragraph(s.into())
    }
    fn h(level: usize, s: &str) -> MarkdownBlock {
        Heading { level, text: s.into() }
    }
    fn code(lang: Option<&str>, s: &str) -> MarkdownBlock {
        Code { language: lang.map(Into::into), text: s.into() }
    }
    fn item(text: &str, checked: Option<bool>) -> MarkdownListItem {
        MarkdownListItem { checked, blocks: vec![p(text)] }
    }
    fn table(b: &[MarkdownBlock], i: usize) -> Option<&MarkdownTable> {
        match b.get(i) {
            Some(Table(t)) => Some(t),
            _ => None,
        }
    }
    fn list(b: &[MarkdownBlock], i: usize) -> Option<&MarkdownList> {
        match b.get(i) {
            Some(List(l)) => Some(l),
            _ => None,
        }
    }
    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter().map(|row| row.iter().map(|s| s.to_string()).collect()).collect()
    }
    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // MARK: MarkdownAndKeysTests

    #[test]
    fn mixed_document() {
        let md = "# Title\nFirst line\nsecond line\n\n```swift\nlet x = 1\n```\n\n```suggestion\nlet y = 2\n```\n> quoted\n> more\n- a\n- b\n1. one\n2) two\n- [ ] todo\n- [x] done\n---\n### Small";
        assert_eq!(
            parse(md),
            vec![
                h(1, "Title"),
                p("First line\nsecond line"),
                code(Some("swift"), "let x = 1"),
                Suggestion("let y = 2".into()),
                Quote(vec![p("quoted\nmore")]),
                List(MarkdownList::new(false, vec![item("a", None), item("b", None)])),
                List(MarkdownList::new(true, vec![item("one", None), item("two", None)])),
                List(MarkdownList::new(false, vec![item("todo", Some(false)), item("done", Some(true))])),
                Rule,
                h(3, "Small"),
            ]
        );
    }

    #[test]
    fn edge_cases() {
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("#nospace"), vec![p("#nospace")]);
        assert_eq!(parse("a\r\nb\r\n\r\nc"), vec![p("a\nb"), p("c")]);
        assert_eq!(parse("```\nunterminated\ncode"), vec![code(None, "unterminated\ncode")]);
        assert_eq!(parse("~~~\nx\n~~~"), vec![code(None, "x")]);
        assert_eq!(parse("```suggestion\n```"), vec![Suggestion(String::new())]);
        assert_eq!(
            parse("- [ ] a\n- b"),
            vec![List(MarkdownList::new(false, vec![item("a", Some(false)), item("b", None)]))]
        );
    }

    #[test]
    fn suggestion_template_round_trips() {
        let t = MarkdownParser::suggestion_template(&["a", "  b"]);
        assert_eq!(t, "```suggestion\na\n  b\n```");
        assert_eq!(parse(&t), vec![Suggestion("a\n  b".into())]);
    }

    // MARK: Tables

    #[test]
    fn basic_table_with_alignment() {
        let b = parse("| Left | Center | Right | None |\n|:-----|:------:|------:|------|\n| a | **b** | `c` | d |");
        assert_eq!(b.len(), 1);
        let t = table(&b, 0).unwrap();
        assert_eq!(t.header, strs(&["Left", "Center", "Right", "None"]));
        assert_eq!(
            t.alignments,
            vec![TableAlignment::Left, TableAlignment::Center, TableAlignment::Right, TableAlignment::None]
        );
        assert_eq!(t.rows, rows(&[&["a", "**b**", "`c`", "d"]]));
        assert_eq!(t.column_count(), 4);
    }

    #[test]
    fn missing_outer_pipes_and_ragged_rows() {
        let b = parse("a | b | c\n--|---|--\n1 | 2\n1 | 2 | 3 | 4\n| only |");
        let t = table(&b, 0).unwrap();
        assert_eq!(t.header, strs(&["a", "b", "c"]));
        assert_eq!(t.rows, rows(&[&["1", "2", ""], &["1", "2", "3"], &["only", "", ""]]));
    }

    #[test]
    fn escaped_pipes_and_pipes_in_code() {
        let b = parse(
            "| Expr | Meaning |\n|---|---|\n| `a | b` | bitwise or |\n| a \\| b | escaped |\n| `x` | unclosed ` tick | extra |",
        );
        let t = table(&b, 0).unwrap();
        assert_eq!(t.rows[0], strs(&["`a | b`", "bitwise or"]));
        assert_eq!(t.rows[1], strs(&["a | b", "escaped"]));
        assert_eq!(t.rows[2], strs(&["`x`", "unclosed ` tick"]));
    }

    #[test]
    fn header_and_delimiter_counts_must_match() {
        let b = parse("| a | b | c |\n|---|---|");
        assert!(table(&b, 0).is_none());
        assert!(b.iter().all(|x| matches!(x, Paragraph(_))));
    }

    #[test]
    fn table_ends_at_blank_line_and_before_list() {
        let b = parse("| k | v |\n|---|---|\n| 1 | 2 |\n- after\n- list\n\ntail");
        assert_eq!(table(&b, 0).unwrap().rows, rows(&[&["1", "2"]]));
        assert_eq!(list(&b, 1).unwrap().items.len(), 2);
        assert_eq!(b.last(), Some(&p("tail")));
    }

    #[test]
    fn table_interrupts_paragraph() {
        let b = parse("Values:\n| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(b.first(), Some(&p("Values:")));
        assert_eq!(table(&b, 1).unwrap().header, strs(&["a", "b"]));
    }

    #[test]
    fn br_tags_become_line_breaks_in_cells() {
        let b = parse("| a |\n|---|\n| one<br>two<br/>three<BR />four |");
        assert_eq!(table(&b, 0).unwrap().rows, rows(&[&["one\ntwo\nthree\nfour"]]));
    }

    #[test]
    fn lone_dashes_are_not_a_table() {
        assert_eq!(parse("a\n---"), vec![h(2, "a")]);
        assert_eq!(parse("---"), vec![Rule]);
    }

    #[test]
    fn every_streaming_prefix_parses() {
        let full = "| Значение | Где пишется | Когда |\n|---|---|---|\n| KICKED | src/domain/affiliates/kickAffiliate.ts:33 | Оператор кикает аффилиата |\n| REINSTATED | src/domain/affiliates/reinstateAffiliate.ts:18 | Оператор восстанавливает |";
        for (idx, _) in full.char_indices() {
            let _ = parse(&full[..idx]);
        }
        assert_eq!(table(&parse(full), 0).unwrap().rows.len(), 2);
        // Half a delimiter row is a (body-less) table as soon as every column has a dash.
        assert_eq!(table(&parse("| a | b |\n|---|-"), 0).unwrap().rows, Vec::<Vec<String>>::new());
        // Half a body row is padded.
        assert_eq!(table(&parse("| a | b |\n|---|---|\n| x"), 0).unwrap().rows, rows(&[&["x", ""]]));
    }

    // MARK: Code fences

    #[test]
    fn language_and_info_string() {
        assert_eq!(parse("```swift title=x\nlet a = 1\n```"), vec![code(Some("swift"), "let a = 1")]);
        assert_eq!(parse("````\n```\ninner\n```\n````"), vec![code(None, "```\ninner\n```")]);
    }

    #[test]
    fn closing_fence_must_be_bare_and_long_enough() {
        assert_eq!(parse("```\na\n```swift\nb\n```"), vec![code(None, "a\n```swift\nb")]);
        assert_eq!(parse("~~~~\na\n~~~\n~~~~"), vec![code(None, "a\n~~~")]);
        assert_eq!(parse("~~~\na\n```\n~~~"), vec![code(None, "a\n```")]);
    }

    #[test]
    fn unterminated_fence_keeps_everything() {
        assert_eq!(
            parse("text\n```ts\nconst a = 1\n\n| not | a table |"),
            vec![p("text"), code(Some("ts"), "const a = 1\n\n| not | a table |")]
        );
        assert_eq!(parse("```"), vec![code(None, "")]);
    }

    #[test]
    fn markdown_inside_code_is_literal() {
        assert_eq!(
            parse("```md\n# not heading\n- not list\n<!-- kept -->\n```"),
            vec![code(Some("md"), "# not heading\n- not list\n<!-- kept -->")]
        );
    }

    #[test]
    fn indented_fence_strips_its_indent() {
        assert_eq!(parse("  ```\n  a\n    b\n  ```"), vec![code(None, "a\n  b")]);
    }

    #[test]
    fn inline_triple_backticks_are_not_a_fence() {
        assert_eq!(parse("```x``` is inline"), vec![p("```x``` is inline")]);
    }

    // MARK: Lists

    #[test]
    fn nested_lists_with_two_three_and_four_spaces() {
        let b = parse("- a\n  - a1\n    - a1x\n- b\n1. one\n   - one-a\n2. two\n    1. two-1");
        let outer = list(&b, 0).unwrap();
        assert!(!outer.ordered && outer.items.len() == 2);
        let a = &outer.items[0].blocks;
        assert_eq!(a.first(), Some(&p("a")));
        let a1 = list(a, 1).unwrap();
        assert_eq!(a1.items[0].blocks.first(), Some(&p("a1")));
        assert_eq!(list(&a1.items[0].blocks, 1).unwrap().items[0].blocks, vec![p("a1x")]);
        let ordered = list(&b, 1).unwrap();
        assert!(ordered.ordered && ordered.items.len() == 2);
        assert_eq!(list(&ordered.items[0].blocks, 1).unwrap().items[0].blocks, vec![p("one-a")]);
        let two1 = list(&ordered.items[1].blocks, 1).unwrap();
        assert!(two1.ordered);
        assert_eq!(two1.items[0].blocks, vec![p("two-1")]);
    }

    #[test]
    fn two_space_nesting_under_ordered_item() {
        let b = parse("1. top\n  - child\n2. next");
        let l = list(&b, 0).unwrap();
        assert_eq!(l.items.len(), 2);
        assert_eq!(list(&l.items[0].blocks, 1).unwrap().items[0].blocks, vec![p("child")]);
    }

    #[test]
    fn start_number_and_lazy_continuation() {
        let b = parse("3. three\ncontinued\n4. four");
        let l = list(&b, 0).unwrap();
        assert_eq!(l.start, 3);
        let blocks: Vec<_> = l.items.iter().map(|i| i.blocks.clone()).collect();
        assert_eq!(blocks, vec![vec![p("three\ncontinued")], vec![p("four")]]);
    }

    #[test]
    fn loose_list_and_paragraph_after_blank() {
        let b = parse("- a\n\n- b\n\n  more b\n\nafter");
        let l = list(&b, 0).unwrap();
        let blocks: Vec<_> = l.items.iter().map(|i| i.blocks.clone()).collect();
        assert_eq!(blocks, vec![vec![p("a")], vec![p("b"), p("more b")]]);
        assert_eq!(b.last(), Some(&p("after")));
    }

    #[test]
    fn code_and_table_inside_items() {
        let b = parse("- step:\n  ```sh\n  make\n  ```\n- table:\n  | a | b |\n  |---|---|\n  | 1 | 2 |");
        let l = list(&b, 0).unwrap();
        assert_eq!(l.items[0].blocks, vec![p("step:"), code(Some("sh"), "make")]);
        assert_eq!(table(&l.items[1].blocks, 1).unwrap().rows, rows(&[&["1", "2"]]));
    }

    #[test]
    fn task_items_mix_with_plain_items() {
        let b = parse("- [x] done\n- [ ] todo\n- plain\n- [X]");
        let l = list(&b, 0).unwrap();
        let checked: Vec<_> = l.items.iter().map(|i| i.checked).collect();
        assert_eq!(checked, vec![Some(true), Some(false), None, Some(true)]);
        assert_eq!(l.items[3].blocks, vec![]);
    }

    #[test]
    fn markers_that_are_not_lists() {
        assert_eq!(parse("**bold** start"), vec![p("**bold** start")]);
        assert_eq!(parse("-1 is negative"), vec![p("-1 is negative")]);
        assert_eq!(parse("* * *"), vec![Rule]);
        // An ordered item not starting at 1 doesn't interrupt a paragraph.
        assert_eq!(parse("in 2024\n2. not a list"), vec![p("in 2024\n2. not a list")]);
        assert!(list(&parse("text\n- interrupts"), 1).is_some());
    }

    #[test]
    fn different_list_types_split() {
        let b = parse("- a\n1. b");
        assert!(!list(&b, 0).unwrap().ordered);
        assert!(list(&b, 1).unwrap().ordered);
    }

    // MARK: Misc

    #[test]
    fn quotes_nest_and_contain_blocks() {
        let b = parse("> # Title\n> - a\n> > inner\nlazy");
        let Some(Quote(q)) = b.first() else { panic!("no quote") };
        assert_eq!(q.first(), Some(&h(1, "Title")));
        assert!(list(q, 1).is_some());
        let Some(Quote(inner)) = q.last() else { panic!("no inner quote") };
        assert_eq!(inner, &vec![p("inner\nlazy")]);
    }

    #[test]
    fn headings_variants() {
        assert_eq!(parse("## Title ##"), vec![h(2, "Title")]);
        assert_eq!(parse("#"), vec![h(1, "")]);
        assert_eq!(parse("####### seven"), vec![p("####### seven")]);
        assert_eq!(parse("Big\n==="), vec![h(1, "Big")]);
        assert_eq!(parse("C# is a language"), vec![p("C# is a language")]);
        assert_eq!(parse("# C#"), vec![h(1, "C#")]);
    }

    #[test]
    fn html_comments_are_hidden() {
        assert_eq!(parse("<!-- template\nhint -->\nBody"), vec![p("Body")]);
        assert_eq!(parse("Body\n<!-- never closed\nhidden"), vec![p("Body")]);
    }

    #[test]
    fn tabs_and_cr_lines() {
        assert_eq!(parse("-\ta\r\n\t- b"), parse("- a\n    - b"));
        assert_eq!(parse("a\rb"), vec![p("a\nb")]);
    }

    #[test]
    fn deep_nesting_does_not_blow_the_stack() {
        let deep = "> ".repeat(500) + "x";
        assert!(!parse(&deep).is_empty());
        let deep_list: Vec<String> = (0..300).map(|i| format!("{}- l{}", "  ".repeat(i), i)).collect();
        assert!(list(&parse(&deep_list.join("\n")), 0).is_some());
    }

    /// The reply from the bug report: a paragraph, a table, a list, paragraphs.
    #[test]
    fn claude_reply_from_screenshot() {
        let md = "В AffiliateEventType (prisma/schema.prisma:202) четыре значения:

| Значение | Где пишется | Когда |
|---|---|---|
| KICKED | src/domain/affiliates/kickAffiliate.ts:33 | Оператор кикает аффилиата (status → KICKED) |
| REINSTATED | src/domain/affiliates/reinstateAffiliate.ts:18 | Оператор восстанавливает кикнутого аффилиата |
| TIER_CHANGED | src/domain/affiliates/updateAffiliateTier.ts:29 | Оператор меняет currentTierIndex |
| REFERRAL_BOUND | src/domain/graph/bindReferral.ts:29 | **Новое в этом PR.** Оператор привязал юзера. В payload лежит `{ referredUserId, reason }` |

Откуда они в базе:

- Первые три появились в init-миграции: `prisma/migrations/20260721150000_init/migration.sql:20`.
- REFERRAL_BOUND добавляет миграция `prisma/migrations/20261103120000_referral_bound_event/migration.sql:2` через `ALTER TYPE … ADD VALUE`.

В OpenAPI-схеме админки enum дописан вручную, в `src/routes/admin.ts:131`.

Не путать с HqEventType.";
        let b = parse(md);
        assert_eq!(b.len(), 6);
        assert_eq!(b[0], p("В AffiliateEventType (prisma/schema.prisma:202) четыре значения:"));
        let t = table(&b, 1).unwrap();
        assert_eq!(t.header, strs(&["Значение", "Где пишется", "Когда"]));
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[3][0], "REFERRAL_BOUND");
        assert!(t.rows[3][2].ends_with("`{ referredUserId, reason }`"));
        assert_eq!(b[2], p("Откуда они в базе:"));
        assert_eq!(list(&b, 3).unwrap().items.len(), 2);
        assert_eq!(b[5], p("Не путать с HqEventType."));
    }

    #[test]
    fn random_input_never_traps() {
        let pieces = [
            "|", "-", ":", "`", "```", "~~~", ">", "#", " ", "  ", "\n", "\t", "1.", "*", "[x]", "a", "Ж", "<!--",
            "-->", "\\", "<br>", "\r", "\u{3000}",
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let len = (next() % 60) as usize;
            let s: String = (0..len).map(|_| pieces[(next() % pieces.len() as u64) as usize]).collect();
            let _ = parse(&s);
        }
    }
}
