//! Side-by-side alignment of hunk lines, and intra-line change emphasis.

use crate::models::{DiffLine, FileDiff, LineKind};
use std::ops::Range;

/// One visual row of a side-by-side diff. Indices point into `FileDiff::lines`; -1 means empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SplitPair {
    pub left: i32,
    pub right: i32,
}

impl SplitPair {
    pub fn new(left: i32, right: i32) -> Self {
        SplitPair { left, right }
    }
}

/// Aligns a hunk's lines into side-by-side rows the way GitHub does: context lines
/// share a row, and inside each change block the i-th deletion is paired with the
/// i-th addition. Linear in the number of lines.
pub struct SplitAligner;

impl SplitAligner {
    pub fn align(lines: &[DiffLine], range: Range<usize>, out: &mut Vec<SplitPair>) {
        let mut dels: Vec<i32> = Vec::new();
        let mut adds: Vec<i32> = Vec::new();

        fn flush(dels: &mut Vec<i32>, adds: &mut Vec<i32>, out: &mut Vec<SplitPair>) {
            let n = dels.len().max(adds.len());
            for k in 0..n {
                out.push(SplitPair::new(
                    dels.get(k).copied().unwrap_or(-1),
                    adds.get(k).copied().unwrap_or(-1),
                ));
            }
            dels.clear();
            adds.clear();
        }

        let lower = range.start;
        for i in range {
            let idx = i as i32;
            match lines[i].kind {
                LineKind::Deletion => {
                    // A deletion after additions starts a new change block.
                    if !adds.is_empty() {
                        flush(&mut dels, &mut adds, out);
                    }
                    dels.push(idx);
                }
                LineKind::Addition => adds.push(idx),
                LineKind::Context => {
                    flush(&mut dels, &mut adds, out);
                    out.push(SplitPair::new(idx, idx));
                }
                LineKind::NoNewline => {
                    // Attach the marker to whichever side the previous line was on.
                    flush(&mut dels, &mut adds, out);
                    let prev = if i > lower { lines[i - 1].kind } else { LineKind::Context };
                    out.push(match prev {
                        LineKind::Deletion => SplitPair::new(idx, -1),
                        LineKind::Addition => SplitPair::new(-1, idx),
                        _ => SplitPair::new(idx, idx),
                    });
                }
            }
        }
        flush(&mut dels, &mut adds, out);
    }

    pub fn align_file(file: &FileDiff) -> Vec<SplitPair> {
        let mut out = Vec::with_capacity(file.lines.len());
        for h in &file.hunks {
            Self::align(&file.lines, h.line_range.clone(), &mut out);
        }
        out
    }
}

/// Byte ranges that differ between a paired deletion and addition, used to
/// emphasize the changed part of a line (GitHub's darker word highlight).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntralineResult {
    /// UTF-8 byte range of the changed part in the old line (on char boundaries).
    pub old: Range<usize>,
    /// UTF-8 byte range of the changed part in the new line (on char boundaries).
    pub new: Range<usize>,
}

pub struct IntralineDiff;

impl IntralineDiff {
    /// Common prefix/suffix trimming on UTF-8 bytes, snapped to token and char boundaries.
    /// Returns `None` when the lines are too different for emphasis to be useful.
    pub fn compute(old: &str, new: &str) -> Option<IntralineResult> {
        let a = old.as_bytes();
        let b = new.as_bytes();
        if a == b {
            return None;
        }
        let max_p = a.len().min(b.len());
        let mut p = 0;
        while p < max_p && a[p] == b[p] {
            p += 1;
        }
        let mut s = 0;
        while s < max_p - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
            s += 1;
        }
        // Snap to word boundaries so we highlight whole identifiers.
        while p > 0
            && is_word(a[p - 1])
            && ((p < a.len() && is_word(a[p])) || (p < b.len() && is_word(b[p])))
        {
            p -= 1;
        }
        while s > 0
            && is_word(a[a.len() - s])
            && ((a.len() - s > p && is_word(a[a.len() - s - 1]))
                || (b.len() - s > p && is_word(b[b.len() - s - 1])))
        {
            s -= 1;
        }
        // Never split a multi-byte character.
        while p > 0 && !(old.is_char_boundary(p) && new.is_char_boundary(p)) {
            p -= 1;
        }
        while s > 0 && !(old.is_char_boundary(a.len() - s) && new.is_char_boundary(b.len() - s)) {
            s -= 1;
        }
        let old_range = p..(a.len() - s);
        let new_range = p..(b.len() - s);
        // If almost everything changed, emphasis is noise.
        let changed = old_range.len().max(new_range.len());
        let total = a.len().max(b.len());
        if total > 0 && changed as f64 / total as f64 > 0.75 && changed > 6 {
            return None;
        }
        Some(IntralineResult { old: old_range, new: new_range })
    }
}

#[inline(always)]
fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c > 127
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parser::UnifiedDiffParser;

    #[test]
    fn pairs_deletions_with_additions() {
        let d = UnifiedDiffParser::parse("--- a/x\n+++ b/x\n@@ -1,5 +1,5 @@\n a\n-b\n-c\n+B\n d\n-e\n+E\n+F\n");
        let pairs = SplitAligner::align_file(&d.files[0]);
        let render: Vec<String> = pairs
            .iter()
            .map(|p| {
                let l = if p.left >= 0 { d.text(0, p.left as usize) } else { "_" };
                let r = if p.right >= 0 { d.text(0, p.right as usize) } else { "_" };
                format!("{l}|{r}")
            })
            .collect();
        assert_eq!(render, ["a|a", "b|B", "c|_", "d|d", "e|E", "_|F"]);
    }

    #[test]
    fn additions_then_deletions_start_new_block() {
        let d = UnifiedDiffParser::parse("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n+n\n-o\n x\n");
        let pairs = SplitAligner::align_file(&d.files[0]);
        assert_eq!(pairs.len(), 3);
        assert!(pairs[0].left == -1 && pairs[1].right == -1);
    }

    #[test]
    fn highlights_changed_word() {
        let r = IntralineDiff::compute("let value = computeOld(x)", "let value = computeNew(x)").unwrap();
        assert_eq!(r.old, 12..22); // whole identifier, snapped to word boundary
        assert_eq!(r.new, 12..22);
    }

    #[test]
    fn identical_returns_none() {
        assert_eq!(IntralineDiff::compute("same", "same"), None);
    }

    #[test]
    fn totally_different_returns_none() {
        assert_eq!(IntralineDiff::compute("import Foundation", "return value * 42"), None);
    }

    #[test]
    fn insertion_only() {
        let r = IntralineDiff::compute("foo(a)", "foo(a, b)").unwrap();
        assert!(r.old.is_empty());
        assert_eq!(r.new, 5..8);
    }

    #[test]
    fn non_ascii_ranges_are_byte_offsets_on_char_boundaries() {
        // "é" is 2 bytes; "ö" vs "ü" share their first UTF-8 byte (0xC3).
        let old = "é = \"föo\" + x";
        let new = "é = \"füo\" + x";
        let r = IntralineDiff::compute(old, new).unwrap();
        // Word snapping widens to the whole token `föo` / `füo`.
        assert_eq!(&old[r.old.clone()], "föo");
        assert_eq!(&new[r.new.clone()], "füo");
        assert_eq!(r.old, 6..10);
        // Mid-character differences never yield a split char.
        let r = IntralineDiff::compute("a ö", "a ü").unwrap();
        assert!("a ö".is_char_boundary(r.old.start) && "a ö".is_char_boundary(r.old.end));
        assert_eq!(&"a ü"[r.new.clone()], "ü");
    }
}
