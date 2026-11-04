//! A fast single-pass, line-at-a-time lexer. It is not a parser — it only needs
//! to be right about comments, strings, keywords, numbers and types, which is what
//! matters visually in a diff.
//!
//! Works on UTF-8 bytes; token offsets are byte offsets into the line. All
//! delimiters are ASCII and every byte >= 0x80 counts as an identifier byte, so
//! tokens always start and end on char boundaries.

use super::language::{LanguageRegistry, LanguageSpec};
use crate::models::{DiffSet, FileDiff, LineKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenKind {
    Plain,
    Keyword,
    Type,
    String,
    Number,
    Comment,
    Function,
    Attribute,
    Constant,
    Tag,
    Property,
}

impl TokenKind {
    pub const ALL: [TokenKind; 11] = [
        TokenKind::Plain,
        TokenKind::Keyword,
        TokenKind::Type,
        TokenKind::String,
        TokenKind::Number,
        TokenKind::Comment,
        TokenKind::Function,
        TokenKind::Attribute,
        TokenKind::Constant,
        TokenKind::Tag,
        TokenKind::Property,
    ];
}

/// A highlighted span, in UTF-8 bytes of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxToken {
    pub start: u32,
    pub length: u32,
    pub kind: TokenKind,
}

impl SyntaxToken {
    #[inline]
    pub fn new(start: usize, length: usize, kind: TokenKind) -> Self {
        SyntaxToken { start: start as u32, length: length as u32, kind }
    }
    /// Byte range of the token within its line.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.length) as usize
    }
}

/// Lexer state carried between consecutive lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LexState {
    #[default]
    Normal,
    /// Index into `LanguageSpec::block_comments`.
    BlockComment(usize),
    /// Index into `LanguageSpec::multiline_strings`.
    MultilineString(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Lexer<'a> {
    pub spec: &'a LanguageSpec,
    is_css: bool,
    is_ruby: bool,
}

#[inline(always)]
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

#[inline(always)]
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c > 127
}

#[inline(always)]
fn matches_at(u: &[u8], pat: &[u8], p: usize) -> bool {
    u.len() >= p + pat.len() && &u[p..p + pat.len()] == pat
}

fn find(u: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    let n = u.len();
    let mut q = from;
    while q + pat.len() <= n {
        if u[q] == pat[0] && matches_at(u, pat, q) {
            return Some(q);
        }
        q += 1;
    }
    None
}

/// Finds the closing delimiter, honoring backslash escapes.
fn find_string_end(u: &[u8], close: &[u8], from: usize) -> Option<usize> {
    let n = u.len();
    let mut q = from;
    while q < n {
        if u[q] == b'\\' {
            q += 2;
            continue;
        }
        if u[q] == close[0] && matches_at(u, close, q) {
            return Some(q);
        }
        q += 1;
    }
    None
}

fn is_leading_key(u: &[u8], s: usize) -> bool {
    u[..s].iter().all(|&c| c == b' ' || c == b'\t' || c == b'-')
}

impl<'a> Lexer<'a> {
    pub fn new(spec: &'a LanguageSpec) -> Self {
        Lexer { spec, is_css: spec.name == "CSS", is_ruby: spec.name == "Ruby" }
    }

    #[inline(always)]
    fn is_ident(&self, c: u8) -> bool {
        is_ident_start(c) || is_digit(c) || (c == b'?' && self.is_ruby)
    }

    pub fn tokenize(&self, line: &str, state: &mut LexState) -> Vec<SyntaxToken> {
        self.tokenize_bytes(line.as_bytes(), state)
    }

    pub fn tokenize_bytes(&self, u: &[u8], state: &mut LexState) -> Vec<SyntaxToken> {
        let spec = self.spec;
        let mut tokens: Vec<SyntaxToken> = Vec::new();
        let n = u.len();
        let mut i = 0usize;

        // Continue a construct from the previous line.
        match *state {
            LexState::BlockComment(idx) => {
                let close = spec.block_comments[idx].1.as_bytes();
                if let Some(e) = find(u, close, 0) {
                    tokens.push(SyntaxToken::new(0, e + close.len(), TokenKind::Comment));
                    i = e + close.len();
                    *state = LexState::Normal;
                } else {
                    if n > 0 {
                        tokens.push(SyntaxToken::new(0, n, TokenKind::Comment));
                    }
                    return tokens;
                }
            }
            LexState::MultilineString(idx) => {
                let close = spec.multiline_strings[idx].1.as_bytes();
                if let Some(e) = find_string_end(u, close, 0) {
                    tokens.push(SyntaxToken::new(0, e + close.len(), TokenKind::String));
                    i = e + close.len();
                    *state = LexState::Normal;
                } else {
                    if n > 0 {
                        tokens.push(SyntaxToken::new(0, n, TokenKind::String));
                    }
                    return tokens;
                }
            }
            LexState::Normal => {}
        }

        if spec.is_markup {
            self.tokenize_markup(u, i, state, &mut tokens);
            return tokens;
        }

        let mut last_was_dot = false;
        'line: while i < n {
            let c = u[i];
            // Whitespace
            if c == b' ' || c == b'\t' {
                i += 1;
                continue;
            }

            // Line comments
            for lc in spec.line_comments {
                let lc = lc.as_bytes();
                if c == lc[0] && matches_at(u, lc, i) {
                    tokens.push(SyntaxToken::new(i, n - i, TokenKind::Comment));
                    break 'line;
                }
            }
            // Block comments
            for (bi, (open, close)) in spec.block_comments.iter().enumerate() {
                let (open, close) = (open.as_bytes(), close.as_bytes());
                if c == open[0] && matches_at(u, open, i) {
                    if let Some(e) = find(u, close, i + open.len()) {
                        tokens.push(SyntaxToken::new(i, e + close.len() - i, TokenKind::Comment));
                        i = e + close.len();
                    } else {
                        tokens.push(SyntaxToken::new(i, n - i, TokenKind::Comment));
                        *state = LexState::BlockComment(bi);
                        break 'line;
                    }
                    continue 'line;
                }
            }
            // Multiline strings (checked before single-line so `"""` wins over `"`).
            for (mi, (open, close)) in spec.multiline_strings.iter().enumerate() {
                let (open, close) = (open.as_bytes(), close.as_bytes());
                if c == open[0] && matches_at(u, open, i) {
                    if let Some(e) = find_string_end(u, close, i + open.len()) {
                        tokens.push(SyntaxToken::new(i, e + close.len() - i, TokenKind::String));
                        i = e + close.len();
                    } else {
                        tokens.push(SyntaxToken::new(i, n - i, TokenKind::String));
                        *state = LexState::MultilineString(mi);
                        break 'line;
                    }
                    continue 'line;
                }
            }
            // Strings
            if spec.string_delimiters.contains(&c) {
                let end = find_string_end(u, &[c], i + 1).unwrap_or(n - 1);
                let len = end + 1 - i;
                // Key-value languages: `"key":` is a property.
                let mut kind = TokenKind::String;
                if spec.key_value {
                    let mut j = end + 1;
                    while j < n && u[j] == b' ' {
                        j += 1;
                    }
                    if j < n && u[j] == b':' {
                        kind = TokenKind::Property;
                    }
                }
                tokens.push(SyntaxToken::new(i, len.min(n - i), kind));
                i = end + 1;
                last_was_dot = false;
                continue;
            }
            // Numbers
            if is_digit(c) || (c == b'.' && i + 1 < n && is_digit(u[i + 1]) && !last_was_dot) {
                let s = i;
                i += 1;
                while i < n && (self.is_ident(u[i]) || (u[i] == b'.' && i + 1 < n && is_digit(u[i + 1]))) {
                    i += 1;
                }
                tokens.push(SyntaxToken::new(s, i - s, TokenKind::Number));
                last_was_dot = false;
                continue;
            }
            // Attributes / decorators / directives / variables
            if spec.attribute_prefixes.contains(&c) && i + 1 < n && (is_ident_start(u[i + 1]) || u[i + 1] == b'{') {
                let s = i;
                i += 1;
                if u[i] == b'{' {
                    while i < n && u[i] != b'}' {
                        i += 1;
                    }
                    i = n.min(i + 1);
                } else {
                    while i < n && (self.is_ident(u[i]) || (u[i] == b'-' && self.is_css)) {
                        i += 1;
                    }
                }
                tokens.push(SyntaxToken::new(s, i - s, TokenKind::Attribute));
                last_was_dot = false;
                continue;
            }
            // Identifiers
            if is_ident_start(c) {
                let s = i;
                i += 1;
                while i < n && self.is_ident(u[i]) {
                    i += 1;
                }
                let word = &u[s..i];
                let mut kind = TokenKind::Plain;
                if !last_was_dot && spec.keywords.contains(word) {
                    kind = TokenKind::Keyword;
                } else if spec.constants.contains(word) {
                    kind = TokenKind::Constant;
                } else if spec.builtin_types.contains(word) || (spec.capitalized_are_types && c.is_ascii_uppercase()) {
                    kind = TokenKind::Type;
                } else {
                    let mut j = i;
                    while j < n && u[j] == b' ' {
                        j += 1;
                    }
                    if j < n && u[j] == b'(' && !spec.key_value {
                        kind = TokenKind::Function;
                    } else if spec.key_value
                        && j < n
                        && (u[j] == b':' || u[j] == b'=')
                        && (tokens.is_empty() || is_leading_key(u, s))
                    {
                        kind = TokenKind::Property;
                    }
                }
                if kind != TokenKind::Plain {
                    tokens.push(SyntaxToken::new(s, i - s, kind));
                }
                last_was_dot = false;
                continue;
            }
            last_was_dot = c == b'.';
            i += 1;
        }
        tokens
    }

    fn tokenize_markup(&self, u: &[u8], start: usize, state: &mut LexState, tokens: &mut Vec<SyntaxToken>) {
        let n = u.len();
        let mut i = start;
        let (open, close): (&[u8], &[u8]) = match self.spec.block_comments.first() {
            Some((o, c)) => (o.as_bytes(), c.as_bytes()),
            None => (&[], &[]),
        };
        while i < n {
            if !open.is_empty() && u[i] == b'<' && matches_at(u, open, i) {
                match find(u, close, i + open.len()) {
                    Some(q) => {
                        tokens.push(SyntaxToken::new(i, q + close.len() - i, TokenKind::Comment));
                        i = q + close.len();
                    }
                    None => {
                        tokens.push(SyntaxToken::new(i, n - i, TokenKind::Comment));
                        *state = LexState::BlockComment(0);
                        return;
                    }
                }
                continue;
            }
            if u[i] == b'<' {
                let mut j = i + 1;
                if j < n && (u[j] == b'/' || u[j] == b'!' || u[j] == b'?') {
                    j += 1;
                }
                let s = j;
                while j < n && (self.is_ident(u[j]) || u[j] == b'-' || u[j] == b':' || u[j] == b'.') {
                    j += 1;
                }
                if j > s {
                    tokens.push(SyntaxToken::new(s, j - s, TokenKind::Tag));
                }
                i = j;
                // Attributes until '>'
                while i < n && u[i] != b'>' {
                    if u[i] == b'"' || u[i] == b'\'' {
                        let q = u[i];
                        let mut e = i + 1;
                        while e < n && u[e] != q {
                            e += 1;
                        }
                        tokens.push(SyntaxToken::new(i, n.min(e + 1) - i, TokenKind::String));
                        i = e + 1;
                    } else if is_ident_start(u[i]) {
                        let a = i;
                        while i < n && (self.is_ident(u[i]) || u[i] == b'-' || u[i] == b':') {
                            i += 1;
                        }
                        tokens.push(SyntaxToken::new(a, i - a, TokenKind::Attribute));
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            i += 1;
        }
    }
}

/// Highlights all lines of a file diff, tracking lexer state separately for the
/// old and new sides so block comments that span lines render correctly.
pub struct DiffHighlighter;

impl DiffHighlighter {
    /// Returns tokens for each line in `file.lines` (same indexing).
    pub fn highlight(file: &FileDiff, diff: &DiffSet) -> Vec<Vec<SyntaxToken>> {
        let lexer = Lexer::new(LanguageRegistry::language_for_path(file.path()));
        let mut result: Vec<Vec<SyntaxToken>> = vec![Vec::new(); file.lines.len()];
        if lexer.spec.name == "Text" {
            return result;
        }
        for hunk in &file.hunks {
            let mut old_state = LexState::Normal;
            let mut new_state = LexState::Normal;
            for i in hunk.line_range.clone() {
                let line = &file.lines[i];
                let text = diff.text_of(line);
                match line.kind {
                    LineKind::Context => {
                        let mut s = new_state;
                        result[i] = lexer.tokenize(text, &mut s);
                        new_state = s;
                        old_state = s;
                    }
                    LineKind::Addition => result[i] = lexer.tokenize(text, &mut new_state),
                    LineKind::Deletion => result[i] = lexer.tokenize(text, &mut old_state),
                    LineKind::NoNewline => {}
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::language::*;
    use crate::models::{DiffLine, FileStatus, Hunk};
    use std::time::Instant;

    fn kinds(text: &str, lang: &LanguageSpec) -> Vec<(String, TokenKind)> {
        let mut s = LexState::Normal;
        Lexer::new(lang).tokenize(text, &mut s).into_iter().map(|t| (text[t.range()].to_string(), t.kind)).collect()
    }

    fn texts(t: &[(String, TokenKind)]) -> Vec<&str> {
        t.iter().map(|x| x.0.as_str()).collect()
    }

    fn kinds_only(t: &[(String, TokenKind)]) -> Vec<TokenKind> {
        t.iter().map(|x| x.1).collect()
    }

    fn pair(s: &str, k: TokenKind) -> (String, TokenKind) {
        (s.to_string(), k)
    }

    /// Builds a single-file, single-hunk diff without the parser.
    fn make_diff(path: &str, lines: &[(LineKind, &str)]) -> DiffSet {
        let mut file = FileDiff::new(path, path, FileStatus::Modified);
        let mut buffer = Vec::new();
        let (mut old, mut new) = (1, 1);
        for &(kind, text) in lines {
            let start = buffer.len() as i32;
            buffer.extend_from_slice(text.as_bytes());
            buffer.push(b'\n');
            let (o, nw) = match kind {
                LineKind::Context => (old, new),
                LineKind::Addition => (0, new),
                LineKind::Deletion => (old, 0),
                LineKind::NoNewline => (0, 0),
            };
            if o > 0 {
                old += 1;
            }
            if nw > 0 {
                new += 1;
            }
            file.lines.push(DiffLine {
                kind,
                old_line: o,
                new_line: nw,
                text_start: start,
                text_length: text.len() as i32,
            });
        }
        file.hunks.push(Hunk {
            old_start: 1,
            old_count: old - 1,
            new_start: 1,
            new_count: new - 1,
            heading: String::new(),
            line_range: 0..lines.len(),
        });
        DiffSet::new(buffer, vec![file])
    }

    #[test]
    fn swift_basics() {
        let t = kinds("let x: Int = foo(42) // hi", &SWIFT);
        assert_eq!(texts(&t), ["let", "Int", "foo", "42", "// hi"]);
        use TokenKind::*;
        assert_eq!(kinds_only(&t), [Keyword, Type, Function, Number, Comment]);
    }

    #[test]
    fn strings_with_escapes() {
        let t = kinds(r#"print("a \"q\" b", 'c')"#, &TYPESCRIPT);
        assert!(t.contains(&pair(r#""a \"q\" b""#, TokenKind::String)));
        assert!(t.contains(&pair("'c'", TokenKind::String)));
    }

    #[test]
    fn block_comment_spans_lines() {
        let mut s = LexState::Normal;
        let lx = Lexer::new(&SWIFT);
        let _ = lx.tokenize("let a = 1 /* start", &mut s);
        assert_eq!(s, LexState::BlockComment(0));
        let t2 = lx.tokenize("still comment", &mut s);
        assert_eq!(t2, [SyntaxToken::new(0, 13, TokenKind::Comment)]);
        let t3 = lx.tokenize("end */ let b", &mut s);
        assert_eq!(s, LexState::Normal);
        assert_eq!(t3.iter().map(|t| t.kind).collect::<Vec<_>>(), [TokenKind::Comment, TokenKind::Keyword]);
    }

    #[test]
    fn multiline_strings() {
        let mut s = LexState::Normal;
        let lx = Lexer::new(&PYTHON);
        let _ = lx.tokenize("x = \"\"\"doc", &mut s);
        assert_eq!(s, LexState::MultilineString(0));
        let t = lx.tokenize("done\"\"\" + y", &mut s);
        assert_eq!(t.first().map(|t| t.kind), Some(TokenKind::String));
        assert_eq!(s, LexState::Normal);
    }

    #[test]
    fn member_access_is_not_keyword() {
        let t = kinds("obj.default = 1", &TYPESCRIPT);
        assert!(!t.iter().any(|x| x.0 == "default" && x.1 == TokenKind::Keyword));
    }

    #[test]
    fn json_keys_are_properties() {
        let t = kinds(r#"  "name": "x", "n": 1, "ok": true"#, &JSON);
        use TokenKind::*;
        assert_eq!(kinds_only(&t), [Property, String, Property, Number, Property, Constant]);
    }

    #[test]
    fn yaml_keys() {
        let t = kinds("  - name: build # comment", &YAML);
        assert_eq!(kinds_only(&t), [TokenKind::Property, TokenKind::Comment]);
    }

    #[test]
    fn markup_tags() {
        let t = kinds(r#"<div class="a">hi</div>"#, &MARKUP);
        use TokenKind::*;
        assert_eq!(kinds_only(&t), [Tag, Attribute, String, Tag]);
    }

    #[test]
    fn markup_comment_spans_lines() {
        let mut s = LexState::Normal;
        let lx = Lexer::new(&MARKUP);
        let t = lx.tokenize("<p> <!-- open", &mut s);
        assert_eq!(s, LexState::BlockComment(0));
        assert_eq!(t.last(), Some(&SyntaxToken::new(4, 9, TokenKind::Comment)));
        let t = lx.tokenize("done --> <b>", &mut s);
        assert_eq!(s, LexState::Normal);
        assert_eq!(t, [SyntaxToken::new(0, 8, TokenKind::Comment), SyntaxToken::new(10, 1, TokenKind::Tag)]);
    }

    #[test]
    fn attributes_and_decorators() {
        assert_eq!(kinds("@MainActor final class A {}", &SWIFT)[0], pair("@MainActor", TokenKind::Attribute));
        assert_eq!(kinds("@property", &PYTHON)[0], pair("@property", TokenKind::Attribute));
    }

    #[test]
    fn unicode_offsets_are_utf8_bytes() {
        let text = "let 🎉 = \"é\"";
        let t = kinds(text, &SWIFT);
        assert_eq!(*t.last().unwrap(), pair("\"é\"", TokenKind::String));
        let mut s = LexState::Normal;
        let toks = Lexer::new(&SWIFT).tokenize(text, &mut s);
        // "let " (4) + 🎉 (4 bytes) + " = " (3) → string starts at byte 11, `"é"` is 4 bytes.
        assert_eq!(*toks.last().unwrap(), SyntaxToken::new(11, 4, TokenKind::String));
    }

    #[test]
    fn never_splits_utf8_sequences() {
        let samples = [
            "let ключ = \"значение\" // комментарий",
            "日本語(1.5) + @属性 #指令 {ü}",
            "x = 'é\\",
            "<ä b=\"ö\">ü</ä>",
            "  ключ: \"v\" # ü",
        ];
        for spec in [&*SWIFT, &*RUST, &*YAML, &*MARKUP, &*CSS, &*RUBY, &*SHELL] {
            for text in samples {
                let mut s = LexState::Normal;
                for t in Lexer::new(spec).tokenize(text, &mut s) {
                    let r = t.range();
                    assert!(r.end <= text.len(), "{text:?} {t:?}");
                    assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end), "{text:?} {t:?}");
                }
            }
        }
    }

    #[test]
    fn unterminated_string_stops_at_line_end() {
        let t = kinds("let s = \"oops", &SWIFT);
        assert_eq!(*t.last().unwrap(), pair("\"oops", TokenKind::String));
    }

    #[test]
    fn diff_highlighter_tracks_sides_separately() {
        let d = make_diff(
            "x.swift",
            &[(LineKind::Deletion, "/* old"), (LineKind::Addition, "let a = 1"), (LineKind::Context, "x */ let")],
        );
        let toks = DiffHighlighter::highlight(&d.files[0], &d);
        assert_eq!(toks[0], [SyntaxToken::new(0, 6, TokenKind::Comment)]);
        assert_eq!(toks[1].first().map(|t| t.kind), Some(TokenKind::Keyword));
        // Context continues from the NEW side state (no open comment) → "let" keyword.
        assert!(toks[2].iter().any(|t| t.kind == TokenKind::Keyword));
    }

    #[test]
    fn diff_highlighter_carries_new_state_into_context() {
        let d = make_diff(
            "x.rs",
            &[(LineKind::Addition, "/* open"), (LineKind::Context, "still"), (LineKind::Deletion, "let x")],
        );
        let toks = DiffHighlighter::highlight(&d.files[0], &d);
        assert_eq!(toks[1], [SyntaxToken::new(0, 5, TokenKind::Comment)]);
        // Context syncs the old side to the new side's state.
        assert_eq!(toks[2], [SyntaxToken::new(0, 5, TokenKind::Comment)]);
    }

    #[test]
    fn plain_text_has_no_tokens() {
        let d = make_diff("x.txt", &[(LineKind::Deletion, "a"), (LineKind::Addition, "b")]);
        let toks = DiffHighlighter::highlight(&d.files[0], &d);
        assert_eq!(toks.len(), 2);
        assert!(toks.iter().all(|t| t.is_empty()));
    }

    #[test]
    fn highlight_largest_file() {
        // 200 hunks x 50 lines of Swift-ish source (mirrors PerformanceTests.highlightLargestFile).
        let samples = [
            "    let value = compute(index: 42, name: \"item\") // trailing comment",
            "    func render(_ row: Int) -> String { return \"\\(row)\" }",
            "    /* block comment that spans */ var total: Double = 0.5",
            "    @MainActor static let shared = Store()",
            "    if case .some(let x) = optional, x > 10 { print(x) }",
        ];
        let mut file = FileDiff::new("Big.swift", "Big.swift", FileStatus::Modified);
        let mut buffer = Vec::new();
        for h in 0..200 {
            let first = file.lines.len();
            for l in 0..50 {
                let text = samples[(h + l) % samples.len()];
                let kind = match l % 5 {
                    0 => LineKind::Deletion,
                    1 => LineKind::Addition,
                    _ => LineKind::Context,
                };
                let start = buffer.len() as i32;
                buffer.extend_from_slice(text.as_bytes());
                buffer.push(b'\n');
                file.lines.push(DiffLine {
                    kind,
                    old_line: 1,
                    new_line: 1,
                    text_start: start,
                    text_length: text.len() as i32,
                });
            }
            file.hunks.push(Hunk {
                old_start: 1,
                old_count: 40,
                new_start: 1,
                new_count: 40,
                heading: String::new(),
                line_range: first..file.lines.len(),
            });
        }
        let d = DiffSet::new(buffer, vec![file]);
        let t0 = Instant::now();
        let toks = DiffHighlighter::highlight(&d.files[0], &d);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("highlight {} lines: {ms:.1}ms", toks.len());
        assert_eq!(toks.len(), d.files[0].lines.len());
        assert!(toks.iter().all(|t| !t.is_empty()));
        assert!(ms < 1500.0);
    }
}
