//! Parses `git diff` / GitHub `.diff` output in a single pass over raw bytes.
//!
//! The parser never allocates per diff line: lines are recorded as byte ranges
//! into the source buffer. Hunk bodies are consumed by their declared counts, so
//! a deleted line whose text starts with `-- ` is never mistaken for a file header.

use crate::models::{DiffLine, DiffSet, FileDiff, FileStatus, Hunk, LineKind};

pub struct UnifiedDiffParser;

impl UnifiedDiffParser {
    pub fn parse(text: &str) -> DiffSet {
        Self::parse_bytes(text.as_bytes().to_vec())
    }

    pub fn parse_bytes(bytes: Vec<u8>) -> DiffSet {
        let files = {
            let mut state = State::new(&bytes);
            state.run()
        };
        DiffSet::new(bytes, files)
    }
}

struct State<'a> {
    buf: &'a [u8],
    pos: usize,
    files: Vec<FileDiff>,
    current: Option<FileDiff>,
    hunk_start: usize,
    old_remaining: i32,
    new_remaining: i32,
    old_line: i32,
    new_line: i32,
    in_hunk: bool,
    pending_hunk: Option<Hunk>,
}

impl<'a> State<'a> {
    fn new(buf: &'a [u8]) -> Self {
        State {
            buf,
            pos: 0,
            files: Vec::new(),
            current: None,
            hunk_start: 0,
            old_remaining: 0,
            new_remaining: 0,
            old_line: 0,
            new_line: 0,
            in_hunk: false,
            pending_hunk: None,
        }
    }

    /// Returns the next line as (start, end excluding newline / CR) and advances.
    fn next_line(&mut self) -> Option<(usize, usize)> {
        let n = self.buf.len();
        if self.pos >= n {
            return None;
        }
        let start = self.pos;
        let i = match self.buf[start..].iter().position(|&b| b == b'\n') {
            Some(off) => {
                self.pos = start + off + 1;
                start + off
            }
            None => {
                self.pos = n;
                n
            }
        };
        let mut end = i;
        if end > start && self.buf[end - 1] == b'\r' {
            end -= 1;
        }
        Some((start, end))
    }

    fn has_prefix(&self, s: usize, e: usize, prefix: &str) -> bool {
        self.buf[s..e].starts_with(prefix.as_bytes())
    }

    fn string(&self, s: usize, e: usize) -> String {
        if e <= s {
            return String::new();
        }
        String::from_utf8_lossy(&self.buf[s..e]).into_owned()
    }

    fn finish_hunk(&mut self) {
        if let (Some(mut h), Some(f)) = (self.pending_hunk.take(), self.current.as_mut()) {
            h.line_range = self.hunk_start..f.lines.len();
            f.hunks.push(h);
        }
        self.pending_hunk = None;
        self.in_hunk = false;
    }

    fn finish_file(&mut self) {
        self.finish_hunk();
        if let Some(f) = self.current.take() {
            self.files.push(f);
        }
    }

    fn run(&mut self) -> Vec<FileDiff> {
        while let Some((s, e)) = self.next_line() {
            if self.in_hunk {
                if self.consume_hunk_line(s, e) {
                    continue;
                }
                self.finish_hunk();
            }
            if self.has_prefix(s, e, "diff --git ") {
                self.finish_file();
                let (a, b) = parse_git_header_paths(&self.string(s + 11, e));
                self.current = Some(FileDiff::new(a, b, FileStatus::Modified));
            } else if self.has_prefix(s, e, "@@ ") {
                if self.current.is_none() {
                    self.current = Some(FileDiff::new("", "", FileStatus::Modified));
                }
                self.start_hunk(s, e);
            } else if self.current.is_some() {
                self.parse_file_header_line(s, e);
            } else if self.has_prefix(s, e, "--- ") {
                // Plain unified diff without a `diff --git` line.
                self.current = Some(FileDiff::new("", "", FileStatus::Modified));
                self.parse_file_header_line(s, e);
            }
        }
        self.finish_file();
        std::mem::take(&mut self.files)
    }

    fn parse_file_header_line(&mut self, s: usize, e: usize) {
        let Some(mut f) = self.current.take() else { return };
        if self.has_prefix(s, e, "new file mode") {
            f.status = FileStatus::Added;
        } else if self.has_prefix(s, e, "deleted file mode") {
            f.status = FileStatus::Deleted;
        } else if self.has_prefix(s, e, "rename from ") {
            f.old_path = unquote(&self.string(s + 12, e));
            f.status = FileStatus::Renamed;
        } else if self.has_prefix(s, e, "rename to ") {
            f.new_path = unquote(&self.string(s + 10, e));
            f.status = FileStatus::Renamed;
        } else if self.has_prefix(s, e, "copy from ") {
            f.old_path = unquote(&self.string(s + 10, e));
            f.status = FileStatus::Copied;
        } else if self.has_prefix(s, e, "copy to ") {
            f.new_path = unquote(&self.string(s + 8, e));
            f.status = FileStatus::Copied;
        } else if self.has_prefix(s, e, "Binary files ") || self.has_prefix(s, e, "GIT binary patch") {
            f.is_binary = true;
        } else if self.has_prefix(s, e, "--- ") {
            let p = strip_prefix(unquote(&self.string(s + 4, e)), "a/");
            if p == "/dev/null" {
                f.status = FileStatus::Added;
            } else {
                f.old_path = p;
            }
        } else if self.has_prefix(s, e, "+++ ") {
            let p = strip_prefix(unquote(&self.string(s + 4, e)), "b/");
            if p == "/dev/null" {
                f.status = FileStatus::Deleted;
            } else {
                f.new_path = p;
            }
        }
        if f.status == FileStatus::Added && f.old_path.is_empty() {
            f.old_path = f.new_path.clone();
        }
        if f.status == FileStatus::Deleted && f.new_path.is_empty() {
            f.new_path = f.old_path.clone();
        }
        if f.old_path.is_empty() {
            f.old_path = f.new_path.clone();
        }
        if f.new_path.is_empty() {
            f.new_path = f.old_path.clone();
        }
        self.current = Some(f);
    }

    fn start_hunk(&mut self, s: usize, e: usize) {
        // @@ -a[,b] +c[,d] @@ heading
        let buf = self.buf;
        let mut i = s + 3;
        let read_int = |i: &mut usize| -> i32 {
            let mut v: i32 = 0;
            while *i < e && buf[*i].is_ascii_digit() {
                v = v.wrapping_mul(10).wrapping_add((buf[*i] - b'0') as i32);
                *i += 1;
            }
            v
        };
        let mut old_count = 1;
        let mut new_count = 1;
        if i < e && buf[i] == b'-' {
            i += 1;
        }
        let old_start = read_int(&mut i);
        if i < e && buf[i] == b',' {
            i += 1;
            old_count = read_int(&mut i);
        }
        while i < e && buf[i] == b' ' {
            i += 1;
        }
        if i < e && buf[i] == b'+' {
            i += 1;
        }
        let new_start = read_int(&mut i);
        if i < e && buf[i] == b',' {
            i += 1;
            new_count = read_int(&mut i);
        }
        // Skip to closing "@@".
        while i + 1 < e && !(buf[i] == b'@' && buf[i + 1] == b'@') {
            i += 1;
        }
        i = e.min(i + 2);
        while i < e && buf[i] == b' ' {
            i += 1;
        }
        let heading = self.string(i, e);

        self.hunk_start = self.current.as_ref().map_or(0, |f| f.lines.len());
        self.pending_hunk = Some(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            heading,
            line_range: self.hunk_start..self.hunk_start,
        });
        self.old_remaining = old_count;
        self.new_remaining = new_count;
        self.old_line = old_start;
        self.new_line = new_start;
        self.in_hunk = true;
    }

    /// Returns false when the line is not part of the current hunk.
    fn consume_hunk_line(&mut self, s: usize, e: usize) -> bool {
        let first = if s < e { self.buf[s] } else { b' ' }; // empty line == empty context
        if first == b'\\' {
            self.append(LineKind::NoNewline, 0, 0, s, e);
            return true;
        }
        if self.old_remaining <= 0 && self.new_remaining <= 0 {
            return false;
        }
        let text_start = if s < e { s + 1 } else { s };
        match first {
            b' ' => {
                self.append(LineKind::Context, self.old_line, self.new_line, text_start, e);
                self.old_line += 1;
                self.new_line += 1;
                self.old_remaining -= 1;
                self.new_remaining -= 1;
            }
            b'-' => {
                self.append(LineKind::Deletion, self.old_line, 0, text_start, e);
                self.old_line += 1;
                self.old_remaining -= 1;
                if let Some(f) = self.current.as_mut() {
                    f.deletions += 1;
                }
            }
            b'+' => {
                self.append(LineKind::Addition, 0, self.new_line, text_start, e);
                self.new_line += 1;
                self.new_remaining -= 1;
                if let Some(f) = self.current.as_mut() {
                    f.additions += 1;
                }
            }
            _ => return false,
        }
        true
    }

    #[inline(always)]
    fn append(&mut self, kind: LineKind, o: i32, n: i32, s: usize, e: usize) {
        if let Some(f) = self.current.as_mut() {
            f.lines.push(DiffLine {
                kind,
                old_line: o,
                new_line: n,
                text_start: s as i32,
                text_length: e.saturating_sub(s) as i32,
            });
        }
    }
}

fn strip_prefix(s: String, p: &str) -> String {
    match s.strip_prefix(p) {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// Undo git's C-style quoting: `"a/f\303\266o bar"`. Also drops a trailing
/// `\t<timestamp>` (`--- a/x\t2024-...`).
fn unquote(raw: &str) -> String {
    let s = match raw.find('\t') {
        Some(tab) => &raw[..tab],
        None => raw,
    };
    if s.chars().count() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return s.to_string();
    }
    let inner = &s.as_bytes()[1..s.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut it = inner.iter().copied();
    while let Some(c) = it.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let Some(n) = it.next() else {
            out.push(c);
            continue;
        };
        match n {
            b'n' => out.push(0x0A),
            b't' => out.push(0x09),
            b'"' => out.push(0x22),
            b'\\' => out.push(0x5C),
            b'0'..=b'7' => {
                let mut v = (n - b'0') as u32;
                for _ in 0..2 {
                    let Some(d) = it.next() else { break };
                    v = v.wrapping_mul(8).wrapping_add(d.wrapping_sub(b'0') as u32);
                }
                out.push(v as u8);
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a/foo b/foo` -> ("foo", "foo"). Ambiguous when paths contain spaces;
/// `---`/`+++`/`rename` lines override this later.
fn parse_git_header_paths(rest: &str) -> (String, String) {
    if rest.starts_with('"') {
        // Quoted form: "a/x y" "b/x y"
        let parts: Vec<&str> = rest.split('"').filter(|p| !p.is_empty() && *p != " ").collect();
        if parts.len() >= 2 {
            return (
                strip_prefix(unquote(&format!("\"{}\"", parts[0])), "a/"),
                strip_prefix(unquote(&format!("\"{}\"", parts[1])), "b/"),
            );
        }
    }
    // Split at the midpoint " b/" where both halves are equal (common case).
    let chars: Vec<char> = rest.chars().collect();
    if chars.len() > 4 {
        let mid = (chars.len() - 1) / 2;
        let left: String = chars[..mid].iter().collect();
        let right: String = chars[mid + 1..].iter().collect();
        if let (Some(l), Some(r)) = (left.strip_prefix("a/"), right.strip_prefix("b/")) {
            if l == r {
                return (l.to_string(), r.to_string());
            }
        }
    }
    if let Some(idx) = rest.find(" b/") {
        return (strip_prefix(rest[..idx].to_string(), "a/"), rest[idx + 3..].to_string());
    }
    (rest.to_string(), rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::split::SplitAligner;
    use crate::support::synthetic::{Shape, SyntheticDiff};

    const SAMPLE: &str = concat!(
        "diff --git a/src/app.swift b/src/app.swift\n",
        "index 83db48f..bf269f4 100644\n",
        "--- a/src/app.swift\n",
        "+++ b/src/app.swift\n",
        "@@ -1,5 +1,6 @@ struct App {\n",
        " import Foundation\n",
        "-let a = 1\n",
        "+let a = 2\n",
        "+let b = 3\n",
        "\n",
        " func main() {\n",
        "--- not a header, a deleted line starting with --\n",
        "@@ -20,2 +21,2 @@\n",
        " x\n",
        "-y\n",
        "+z\n",
        "\\ No newline at end of file\n",
        "diff --git a/new.txt b/new.txt\n",
        "new file mode 100644\n",
        "index 0000000..e69de29\n",
        "--- /dev/null\n",
        "+++ b/new.txt\n",
        "@@ -0,0 +1,2 @@\n",
        "+hello\n",
        "+world\n",
        "diff --git a/old.txt b/old.txt\n",
        "deleted file mode 100644\n",
        "--- a/old.txt\n",
        "+++ /dev/null\n",
        "@@ -1 +0,0 @@\n",
        "-bye\n",
        "diff --git a/from.txt b/to.txt\n",
        "similarity index 100%\n",
        "rename from from.txt\n",
        "rename to to.txt\n",
        "diff --git a/img.png b/img.png\n",
        "Binary files a/img.png and b/img.png differ\n",
    );

    #[test]
    fn parses_files_and_statuses() {
        let d = UnifiedDiffParser::parse(SAMPLE);
        let paths: Vec<&str> = d.files.iter().map(|f| f.path()).collect();
        assert_eq!(paths, ["src/app.swift", "new.txt", "old.txt", "to.txt", "img.png"]);
        let statuses: Vec<FileStatus> = d.files.iter().map(|f| f.status).collect();
        use FileStatus::*;
        assert_eq!(statuses, [Modified, Added, Deleted, Renamed, Modified]);
        assert_eq!(d.files[3].old_path, "from.txt");
        assert!(d.files[4].is_binary);
    }

    #[test]
    fn parses_hunks_and_line_numbers() {
        let d = UnifiedDiffParser::parse(SAMPLE);
        let f = &d.files[0];
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert!(h.old_start == 1 && h.old_count == 5 && h.new_start == 1 && h.new_count == 6);
        assert_eq!(h.heading, "struct App {");
        assert!(f.additions == 3 && f.deletions == 3);
        let first: Vec<DiffLine> = h.line_range.clone().map(|i| f.lines[i]).collect();
        use LineKind::*;
        let kinds: Vec<LineKind> = first.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [Context, Deletion, Addition, Addition, Context, Context, Deletion]);
        assert!(first[1].old_line == 2 && first[1].new_line == 0);
        assert_eq!(first[3].new_line, 3);
        assert!(first[4].old_line == 3 && first[4].new_line == 4);
        assert_eq!(d.text_of(&first[6]), "-- not a header, a deleted line starting with --");
        assert_eq!(d.text_of(&first[4]), "");
    }

    #[test]
    fn no_newline_marker() {
        let d = UnifiedDiffParser::parse(SAMPLE);
        let f = &d.files[0];
        let last = f.hunks[1].line_range.end - 1;
        assert_eq!(f.lines[last].kind, LineKind::NoNewline);
    }

    #[test]
    fn hunk_header_without_counts() {
        let d = UnifiedDiffParser::parse(SAMPLE);
        let h = &d.files[2].hunks[0];
        assert!(h.old_start == 1 && h.old_count == 1 && h.new_start == 0 && h.new_count == 0);
    }

    #[test]
    fn quoted_paths_and_crlf() {
        let text = "diff --git \"a/sp ace/f\\303\\266.txt\" \"b/sp ace/f\\303\\266.txt\"\r\n--- \"a/sp ace/f\\303\\266.txt\"\r\n+++ \"b/sp ace/f\\303\\266.txt\"\r\n@@ -1 +1 @@\r\n-a\r\n+b\r\n";
        let d = UnifiedDiffParser::parse(text);
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path(), "sp ace/fö.txt");
        assert_eq!(d.text(0, 1), "b");
    }

    #[test]
    fn path_with_spaces_in_git_header() {
        let d = UnifiedDiffParser::parse("diff --git a/my file.txt b/my file.txt\nindex 1..2\n");
        assert_eq!(d.files[0].path(), "my file.txt");
    }

    #[test]
    fn plain_unified_diff_without_git_header() {
        let d = UnifiedDiffParser::parse("--- a/x.c\n+++ b/x.c\n@@ -1 +1 @@\n-1\n+2\n");
        assert!(d.files.len() == 1 && d.files[0].path() == "x.c" && d.files[0].lines.len() == 2);
    }

    #[test]
    fn empty_input() {
        assert!(UnifiedDiffParser::parse("").files.is_empty());
    }

    #[test]
    fn file_index_lookup() {
        let d = UnifiedDiffParser::parse(SAMPLE);
        assert_eq!(d.file_index("to.txt"), Some(3));
        assert_eq!(d.file_index("from.txt"), Some(3));
        assert_eq!(d.file_index("nope"), None);
    }

    #[test]
    fn synthetic_diff_round_trip() {
        let d = UnifiedDiffParser::parse(&SyntheticDiff::make(Shape::new(10, 3, 20)));
        assert_eq!(d.files.len(), 10);
        assert!(d.files.iter().all(|f| f.hunks.len() == 3));
        for f in &d.files {
            for h in &f.hunks {
                let lines = &f.lines[h.line_range.clone()];
                assert_eq!(lines.iter().filter(|l| l.kind != LineKind::Addition).count() as i32, h.old_count);
                assert_eq!(lines.iter().filter(|l| l.kind != LineKind::Deletion).count() as i32, h.new_count);
            }
        }
    }

    #[test]
    fn invalid_utf8_and_truncated_input_do_not_panic() {
        let mut bytes = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-\xff\xfe\n+ok\n".to_vec();
        bytes.extend_from_slice(b"@@ -");
        let d = UnifiedDiffParser::parse_bytes(bytes);
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.text(0, 1), "ok");
        assert_eq!(d.text(0, 0), "");
        assert_eq!(SplitAligner::align_file(&d.files[0]).len(), 1);
    }

    // MARK: - Performance (budgets ~10x the Swift debug budgets)

    #[test]
    fn parse_huge_diff() {
        let bytes = crate::support::synthetic::huge_fixture().as_bytes().to_vec();
        let n = bytes.len();
        let start = std::time::Instant::now();
        let d = UnifiedDiffParser::parse_bytes(bytes);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        println!("parse {}MB, {} lines: {ms:.1}ms", n / 1_000_000, d.total_lines());
        assert_eq!(d.files.len(), 2000);
        assert!(d.total_lines() >= 190_000);
        assert!(ms < 15_000.0, "parse took {ms}ms");
    }
}
