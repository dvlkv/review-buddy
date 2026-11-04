//! Flattens a `DiffSet` plus inline blocks into the rows of a virtualized diff table.

use crate::diff::split::{SplitAligner, SplitPair};
use crate::models::{DiffLine, DiffSet, DiffSide, FileDiff, FileStatus, LineKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiffViewMode {
    #[default]
    Unified,
    Split,
}

/// Heuristics matching GitHub's linguist "generated" defaults for common files.
pub struct GeneratedFileDetector;

impl GeneratedFileDetector {
    const EXACT_NAMES: [&'static str; 12] = [
        "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "Gemfile.lock",
        "Podfile.lock", "composer.lock", "poetry.lock", "go.sum", "Package.resolved", "bun.lockb", "uv.lock",
    ];
    const SUFFIXES: [&'static str; 9] = [
        ".min.js", ".min.css", ".pb.go", "_pb2.py", ".pbxproj", ".snap", ".generated.swift", ".g.dart", ".map",
    ];

    pub fn is_generated(path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        if Self::EXACT_NAMES.contains(&name) {
            return true;
        }
        if Self::SUFFIXES.iter().any(|s| name.ends_with(s)) {
            return true;
        }
        path.contains("/__generated__/")
            || path.starts_with("dist/")
            || path.contains("/dist/")
            || path.starts_with("vendor/")
            || path.contains("/node_modules/")
    }
}

// MARK: - Layout model

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InlineBlockKind {
    ReviewThread,
    Composer,
    Discuss,
    Finding,
}

/// Something rendered between diff rows: a review thread, a comment composer,
/// a Discuss marker, or an AI finding. Placed after the row showing `line` on `side`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InlineBlockRef {
    pub id: String,
    pub kind: InlineBlockKind,
    pub side: DiffSide,
    pub line: i64,
}

impl InlineBlockRef {
    pub fn new(id: impl Into<String>, kind: InlineBlockKind, side: DiffSide, line: i64) -> Self {
        InlineBlockRef { id: id.into(), kind, side, line }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RowKind {
    /// File header. `a` unused.
    FileHeader,
    /// `a` = hunk index, `b` = file lines still hidden above it (0 = none). The header
    /// comes before any context expanded above the hunk, so its "expand" arrow stays
    /// at the edge of what's still hidden.
    HunkHeader,
    /// Unified line. `a` = index into `FileDiff::lines`.
    Line,
    /// Split row. `a` = left line index, `b` = right line index (-1 = empty).
    Pair,
    /// Context line loaded from the full file. `a` = new line number, `b` = old line number.
    Expanded,
    /// Inline block. `a` = index into `DiffLayout::blocks`.
    Block,
    /// Placeholder for a collapsed large/generated/binary file. `a` = `PlaceholderReason as i32`.
    Placeholder,
    /// "Expand below" affordance after the last hunk (only with `show_expand_footer`).
    /// `b` = lines already expanded below, so the row changes (and is redrawn) on expand.
    ExpandFooter,
}

/// A row of the flattened, virtualized diff table. 16 bytes; a 200k-line PR is ~3 MB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayoutRow {
    pub kind: RowKind,
    pub file: i32,
    pub a: i32,
    pub b: i32,
}

impl LayoutRow {
    /// A row with `a` and `b` unset (-1).
    pub fn new(kind: RowKind, file: i32) -> Self {
        LayoutRow { kind, file, a: -1, b: -1 }
    }
    pub fn with(kind: RowKind, file: i32, a: i32, b: i32) -> Self {
        LayoutRow { kind, file, a, b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum PlaceholderReason {
    Binary,
    Large,
    Generated,
    DeletedOnly,
    Empty,
}

impl PlaceholderReason {
    pub fn raw(self) -> i32 {
        self as i32
    }
    pub fn from_raw(raw: i32) -> Option<Self> {
        use PlaceholderReason::*;
        [Binary, Large, Generated, DeletedOnly, Empty].into_iter().find(|r| r.raw() == raw)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLayoutOptions {
    pub mode: DiffViewMode,
    /// Files whose body is hidden (user collapsed, or marked viewed).
    pub collapsed: HashSet<usize>,
    /// Large/generated files the user asked to render anyway.
    pub force_loaded: HashSet<usize>,
    /// Context lines expanded above hunk `h` of file `f`: key = `DiffLayoutOptions::key(f, h)`.
    pub expanded_above: HashMap<i64, i64>,
    /// Lines expanded below the last hunk of file `f`.
    pub expanded_below: HashMap<usize, i64>,
    /// Files whose tail has been expanded to the end: their `ExpandFooter` is dropped.
    pub below_complete: HashSet<usize>,
    pub large_file_threshold: usize,
    /// Emit an `ExpandFooter` row after each file's last hunk.
    pub show_expand_footer: bool,
}

impl Default for DiffLayoutOptions {
    fn default() -> Self {
        DiffLayoutOptions {
            mode: DiffViewMode::Unified,
            collapsed: HashSet::new(),
            force_loaded: HashSet::new(),
            expanded_above: HashMap::new(),
            expanded_below: HashMap::new(),
            below_complete: HashSet::new(),
            large_file_threshold: 2000,
            show_expand_footer: false,
        }
    }
}

impl DiffLayoutOptions {
    pub fn with_mode(mode: DiffViewMode) -> Self {
        DiffLayoutOptions { mode, ..Default::default() }
    }

    /// `file << 32 | hunk`.
    pub fn key(file: usize, hunk: usize) -> i64 {
        ((file as i64) << 32) | hunk as i64
    }
}

/// Per-file inline block lookup, only built for files that have blocks.
struct FileBlocks {
    left: HashMap<i64, Vec<i32>>,
    right: HashMap<i64, Vec<i32>>,
    first: i32,
    end: i32,
}

impl FileBlocks {
    #[inline(always)]
    fn emit(&self, line: &DiffLine, file: i32, rows: &mut Vec<LayoutRow>) {
        if line.new_line > 0 && line.kind != LineKind::Deletion {
            push_blocks(self.right.get(&i64::from(line.new_line)), file, rows);
        }
        if line.old_line > 0 && line.kind != LineKind::Addition {
            push_blocks(self.left.get(&i64::from(line.old_line)), file, rows);
        }
    }
}

#[inline(always)]
fn push_blocks(ids: Option<&Vec<i32>>, file: i32, rows: &mut Vec<LayoutRow>) {
    if let Some(ids) = ids {
        rows.extend(ids.iter().map(|&id| LayoutRow::with(RowKind::Block, file, id, -1)));
    }
}

/// Flattens a `DiffSet` + inline blocks into table rows. Pure and O(rows), so it
/// can be rebuilt on every structural change (mode switch, collapse, new comment).
#[derive(Debug)]
pub struct DiffLayout {
    pub diff: Arc<DiffSet>,
    pub options: DiffLayoutOptions,
    pub rows: Vec<LayoutRow>,
    pub blocks: Vec<InlineBlockRef>,
    /// Row index of each file header.
    pub file_start_row: Vec<usize>,
    /// Split pairs per file (only populated in split mode), shared so a rebuild can reuse them.
    pub split_pairs: Arc<Vec<Vec<SplitPair>>>,
}

impl DiffLayout {
    pub fn new(diff: Arc<DiffSet>, options: DiffLayoutOptions) -> Self {
        Self::with_blocks(diff, options, &HashMap::new(), None)
    }

    /// `blocks_by_file` maps file index to its inline blocks; `split_cache` is a previous
    /// layout's `split_pairs` for the same diff (ignored if its file count differs).
    pub fn with_blocks(
        diff: Arc<DiffSet>,
        options: DiffLayoutOptions,
        blocks_by_file: &HashMap<usize, Vec<InlineBlockRef>>,
        split_cache: Option<Arc<Vec<Vec<SplitPair>>>>,
    ) -> Self {
        let files = &diff.files;
        let mut rows: Vec<LayoutRow> = Vec::with_capacity(diff.total_lines() + files.len() * 4);
        let mut starts: Vec<usize> = Vec::with_capacity(files.len());
        let mut all_blocks: Vec<InlineBlockRef> = Vec::new();
        let split = options.mode == DiffViewMode::Split;
        let pairs_cache: Arc<Vec<Vec<SplitPair>>> = match split_cache {
            _ if !split => Arc::new(Vec::new()),
            Some(c) if c.len() == files.len() => c,
            _ => Arc::new(files.iter().map(SplitAligner::align_file).collect()),
        };
        let has_expansions = !options.expanded_above.is_empty() || !options.expanded_below.is_empty();

        for (fi, file) in files.iter().enumerate() {
            let f = fi as i32;
            starts.push(rows.len());
            rows.push(LayoutRow::new(RowKind::FileHeader, f));
            if options.collapsed.contains(&fi) {
                continue;
            }
            if let Some(reason) = Self::placeholder_reason(file, fi, &options, Some(diff.is_generated[fi])) {
                rows.push(LayoutRow::with(RowKind::Placeholder, f, reason.raw(), -1));
                continue;
            }

            let fb = match blocks_by_file.get(&fi) {
                Some(bs) if !bs.is_empty() => {
                    let mut x = FileBlocks {
                        left: HashMap::new(),
                        right: HashMap::new(),
                        first: all_blocks.len() as i32,
                        end: 0,
                    };
                    for b in bs {
                        let idx = all_blocks.len() as i32;
                        all_blocks.push(b.clone());
                        let map = if b.side == DiffSide::Left { &mut x.left } else { &mut x.right };
                        map.entry(b.line).or_default().push(idx);
                    }
                    x.end = all_blocks.len() as i32;
                    Some(x)
                }
                _ => None,
            };
            let file_row_start = rows.len();
            let pairs: &[SplitPair] = if split { &pairs_cache[fi] } else { &[] };
            let mut pair_cursor = 0;
            let mut prev_new_end: i64 = 1; // first new line not yet shown
            let lines = &file.lines;

            for (hi, hunk) in file.hunks.iter().enumerate() {
                let n = if has_expansions {
                    options.expanded_above.get(&DiffLayoutOptions::key(fi, hi)).copied().unwrap_or(0)
                } else {
                    0
                };
                let new_first = i64::from(hunk.new_first());
                let lo = prev_new_end.max(new_first - n.max(0));
                // Once the gap above is fully revealed the header has nothing left to
                // expand: drop it so the hunk joins what's above (as on GitHub).
                let closed = lo <= prev_new_end && (n > 0 || hi > 0);
                if !closed {
                    rows.push(LayoutRow::with(RowKind::HunkHeader, f, hi as i32, (lo - prev_new_end).max(0) as i32));
                }
                if lo < new_first {
                    let old_offset = new_first - i64::from(hunk.old_first());
                    emit_expanded(lo..new_first, old_offset, f, fb.as_ref(), &mut rows);
                }
                if split {
                    // Pairs for this hunk are contiguous in `pairs`.
                    while pair_cursor < pairs.len() {
                        let p = pairs[pair_cursor];
                        let any_idx = if p.left >= 0 { p.left } else { p.right } as usize;
                        if any_idx >= hunk.line_range.end {
                            break;
                        }
                        pair_cursor += 1;
                        rows.push(LayoutRow::with(RowKind::Pair, f, p.left, p.right));
                        if let Some(fb) = &fb {
                            if p.right >= 0 {
                                fb.emit(&lines[p.right as usize], f, &mut rows);
                            }
                            if p.left >= 0 && p.left != p.right {
                                let l = &lines[p.left as usize];
                                if l.kind == LineKind::Deletion {
                                    push_blocks(fb.left.get(&i64::from(l.old_line)), f, &mut rows);
                                }
                            }
                        }
                    }
                } else if let Some(fb) = &fb {
                    for i in hunk.line_range.clone() {
                        rows.push(LayoutRow::with(RowKind::Line, f, i as i32, -1));
                        fb.emit(&lines[i], f, &mut rows);
                    }
                } else {
                    rows.extend(hunk.line_range.clone().map(|i| LayoutRow::with(RowKind::Line, f, i as i32, -1)));
                }
                prev_new_end = prev_new_end.max(i64::from(hunk.new_end()));
            }
            let below = if has_expansions {
                options.expanded_below.get(&fi).copied().unwrap_or(0).max(0)
            } else {
                0
            };
            if below > 0 {
                if let Some(last) = file.hunks.last() {
                    let old_offset = i64::from(last.new_end()) - i64::from(last.old_end());
                    emit_expanded(prev_new_end..prev_new_end + below, old_offset, f, fb.as_ref(), &mut rows);
                }
            }
            if options.show_expand_footer
                && !file.hunks.is_empty()
                && file.status != FileStatus::Deleted
                && !options.below_complete.contains(&fi)
            {
                rows.push(LayoutRow::with(RowKind::ExpandFooter, f, -1, below as i32));
            }
            // Blocks whose line is not visible (outdated threads, lines outside hunks)
            // go at the end of the file so nothing is silently hidden.
            if let Some(fb) = &fb {
                let shown: HashSet<i32> =
                    rows[file_row_start..].iter().filter(|r| r.kind == RowKind::Block).map(|r| r.a).collect();
                for id in fb.first..fb.end {
                    if !shown.contains(&id) {
                        rows.push(LayoutRow::with(RowKind::Block, f, id, -1));
                    }
                }
            }
        }

        DiffLayout {
            diff,
            options,
            rows,
            blocks: all_blocks,
            file_start_row: starts,
            split_pairs: pairs_cache,
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Index of the file containing `row` (binary search over file starts; 0 when empty).
    pub fn file_index_for_row(&self, row: usize) -> usize {
        self.file_start_row.partition_point(|&s| s <= row).saturating_sub(1)
    }

    pub fn row_range_for_file(&self, f: usize) -> Range<usize> {
        let start = self.file_start_row[f];
        let end = self.file_start_row.get(f + 1).copied().unwrap_or(self.rows.len());
        start..end
    }

    /// The row showing `line` on `side` in file `f`, if visible.
    pub fn row(&self, f: usize, side: DiffSide, line: i64) -> Option<usize> {
        if f >= self.file_start_row.len() {
            return None;
        }
        let file = &self.diff.files[f];
        let line_at = |idx: i32| idx >= 0 && i64::from(file.lines[idx as usize].line_number(side)) == line;
        self.row_range_for_file(f).find(|&r| {
            let row = &self.rows[r];
            match row.kind {
                RowKind::Line => line_at(row.a),
                RowKind::Pair => line_at(if side == DiffSide::Left { row.a } else { row.b }),
                RowKind::Expanded => i64::from(if side == DiffSide::Left { row.b } else { row.a }) == line,
                _ => false,
            }
        })
    }

    /// Why file `index` is shown as a placeholder instead of its lines, if it is.
    /// `generated` overrides the path heuristic (pass `DiffSet::is_generated[index]`).
    pub fn placeholder_reason(
        file: &FileDiff,
        index: usize,
        options: &DiffLayoutOptions,
        generated: Option<bool>,
    ) -> Option<PlaceholderReason> {
        if file.is_binary {
            return Some(PlaceholderReason::Binary);
        }
        if options.force_loaded.contains(&index) {
            return None;
        }
        if file.lines.is_empty() {
            return match file.status {
                FileStatus::Renamed | FileStatus::Copied => None,
                _ => Some(PlaceholderReason::Empty),
            };
        }
        if generated.unwrap_or_else(|| GeneratedFileDetector::is_generated(file.path())) {
            return Some(PlaceholderReason::Generated);
        }
        if file.changed_line_count() > options.large_file_threshold {
            return Some(PlaceholderReason::Large);
        }
        None
    }
}

fn emit_expanded(new_range: Range<i64>, old_offset: i64, file: i32, blocks: Option<&FileBlocks>, rows: &mut Vec<LayoutRow>) {
    for n in new_range {
        rows.push(LayoutRow::with(RowKind::Expanded, file, n as i32, (n - old_offset) as i32));
        if let Some(fb) = blocks {
            push_blocks(fb.right.get(&n), file, rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parser::UnifiedDiffParser;
    use crate::models::Hunk;
    use std::time::Instant;

    const DIFF: &str = concat!(
        "diff --git a/a.swift b/a.swift\n",
        "--- a/a.swift\n",
        "+++ b/a.swift\n",
        "@@ -10,3 +10,3 @@\n",
        " one\n",
        "-two\n",
        "+TWO\n",
        " three\n",
        "@@ -30,2 +30,3 @@\n",
        " x\n",
        "+y\n",
        " z\n",
        "diff --git a/package-lock.json b/package-lock.json\n",
        "--- a/package-lock.json\n",
        "+++ b/package-lock.json\n",
        "@@ -1 +1 @@\n",
        "-{}\n",
        "+{ }\n",
        "diff --git a/b.swift b/b.swift\n",
        "--- a/b.swift\n",
        "+++ b/b.swift\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    );

    fn diff() -> Arc<DiffSet> {
        Arc::new(UnifiedDiffParser::parse(DIFF))
    }

    fn layout(options: DiffLayoutOptions) -> DiffLayout {
        DiffLayout::new(diff(), options)
    }

    fn above(file: usize, hunk: usize, n: i64) -> DiffLayoutOptions {
        DiffLayoutOptions {
            expanded_above: HashMap::from([(DiffLayoutOptions::key(file, hunk), n)]),
            ..Default::default()
        }
    }

    fn expanded(l: &DiffLayout) -> Vec<LayoutRow> {
        l.rows.iter().copied().filter(|r| r.kind == RowKind::Expanded).collect()
    }

    #[test]
    fn layout_row_is_16_bytes() {
        assert_eq!(std::mem::size_of::<LayoutRow>(), 16);
    }

    #[test]
    fn unified_rows() {
        let l = layout(DiffLayoutOptions::with_mode(DiffViewMode::Unified));
        assert_eq!(l.file_start_row, [0, 10, 12]);
        assert_eq!(l.rows[0].kind, RowKind::FileHeader);
        assert_eq!(l.rows[1].kind, RowKind::HunkHeader);
        assert!(l.rows[2..=5].iter().all(|r| r.kind == RowKind::Line));
        assert!(l.rows[11].kind == RowKind::Placeholder && l.rows[11].a == PlaceholderReason::Generated.raw());
        assert_eq!(l.row_count(), 16);
    }

    #[test]
    fn split_rows() {
        let l = layout(DiffLayoutOptions::with_mode(DiffViewMode::Split));
        // file0: header, hunk, 3 pairs, hunk, 3 pairs
        assert_eq!(l.row_range_for_file(0).len(), 1 + 1 + 3 + 1 + 3);
        let pair = l.rows[3];
        assert_eq!(pair.kind, RowKind::Pair);
        assert_eq!(l.diff.text(0, pair.a as usize), "two");
        assert_eq!(l.diff.text(0, pair.b as usize), "TWO");
    }

    #[test]
    fn collapsed_and_force_loaded() {
        let l = layout(DiffLayoutOptions {
            collapsed: HashSet::from([0]),
            force_loaded: HashSet::from([1]),
            ..Default::default()
        });
        assert_eq!(l.row_range_for_file(0).len(), 1);
        assert_eq!(l.rows[l.file_start_row[1] + 1].kind, RowKind::HunkHeader);
    }

    #[test]
    fn large_file_becomes_placeholder() {
        let l = layout(DiffLayoutOptions { large_file_threshold: 1, ..Default::default() });
        assert!(l.rows[1].kind == RowKind::Placeholder && l.rows[1].a == PlaceholderReason::Large.raw());
    }

    #[test]
    fn blocks_placed_after_their_line() {
        let blocks = HashMap::from([(
            0usize,
            vec![
                InlineBlockRef::new("t1", InlineBlockKind::ReviewThread, DiffSide::Right, 11),
                InlineBlockRef::new("t2", InlineBlockKind::Discuss, DiffSide::Left, 11),
                InlineBlockRef::new("gone", InlineBlockKind::ReviewThread, DiffSide::Right, 999),
            ],
        )]);
        for mode in [DiffViewMode::Unified, DiffViewMode::Split] {
            let l = DiffLayout::with_blocks(diff(), DiffLayoutOptions::with_mode(mode), &blocks, None);
            let ids: Vec<&str> = l
                .rows
                .iter()
                .filter(|r| r.kind == RowKind::Block)
                .map(|r| l.blocks[r.a as usize].id.as_str())
                .collect();
            // Unified: "-two" (old 11) precedes "+TWO" (new 11). Split: both on one row, right first.
            let expected = if mode == DiffViewMode::Unified { ["t2", "t1", "gone"] } else { ["t1", "t2", "gone"] };
            assert_eq!(ids, expected, "mode {mode:?}");
            // t1 follows the row containing new line 11 ("TWO")
            let r = l.row(0, DiffSide::Right, 11).unwrap();
            assert_eq!(l.rows[r + 1].kind, RowKind::Block);
        }
    }

    #[test]
    fn file_index_for_row_binary_search() {
        let l = layout(DiffLayoutOptions::default());
        assert_eq!(l.file_index_for_row(0), 0);
        assert_eq!(l.file_index_for_row(9), 0);
        assert_eq!(l.file_index_for_row(10), 1);
        assert_eq!(l.file_index_for_row(14), 2);
        let empty = DiffLayout::new(Arc::new(DiffSet::empty()), DiffLayoutOptions::default());
        assert_eq!(empty.file_index_for_row(5), 0);
    }

    #[test]
    fn expanded_context_above_and_below() {
        let mut opts = above(0, 1, 5);
        opts.expanded_below = HashMap::from([(0, 2)]);
        let l = layout(opts);
        let exp = expanded(&l);
        assert_eq!(exp.iter().map(|r| r.a).collect::<Vec<_>>(), [25, 26, 27, 28, 29, 33, 34]);
        // Old numbering offset after hunk 0 is 0, after hunk 1 is +1.
        assert_eq!(exp.first().unwrap().b, 25);
        assert_eq!(exp.last().unwrap().b, 33);
        assert!(l.row(0, DiffSide::Right, 27).is_some());
    }

    #[test]
    fn expanded_above_clamps_to_previous_hunk() {
        let l = layout(above(0, 1, 100));
        let exp = expanded(&l);
        assert_eq!(exp.first().unwrap().a, 13);
        assert_eq!(exp.last().unwrap().a, 29);
    }

    /// Regression: the header used to come *after* lines expanded above it, so each click
    /// pushed the arrow 20 rows down and the next click landed on code.
    #[test]
    fn hunk_header_stays_above_expanded_context() {
        let mut header_rows = HashSet::new();
        let mut hidden = Vec::new();
        for n in [0i64, 5, 10, 16] {
            let l = layout(above(0, 1, n));
            let h = l
                .rows
                .iter()
                .position(|r| r.kind == RowKind::HunkHeader && r.file == 0 && r.a == 1)
                .unwrap();
            header_rows.insert(h);
            hidden.push(l.rows[h].b);
            // Expanded lines follow the header, contiguous and ending right before the hunk.
            let exp: Vec<i64> = l.rows[h + 1..]
                .iter()
                .take_while(|r| r.kind == RowKind::Expanded)
                .map(|r| i64::from(r.a))
                .collect();
            assert_eq!(exp, ((30 - n).max(13)..30).collect::<Vec<_>>());
        }
        assert_eq!(header_rows.len(), 1);
        // 13..<30 is hidden at first: 17 lines.
        assert_eq!(hidden, [17, 12, 7, 1]);
        // Fully revealed: the header goes and the gap's lines take its place.
        let h = *header_rows.iter().next().unwrap();
        for n in [17i64, 100] {
            let l = layout(above(0, 1, n));
            assert!(!l.rows.iter().any(|r| r.kind == RowKind::HunkHeader && r.file == 0 && r.a == 1));
            assert!(l.rows[h].kind == RowKind::Expanded && l.rows[h].a == 13);
        }
    }

    #[test]
    fn expanding_the_first_hunk_reaches_line_one() {
        // Reaching line 1 leaves nothing to expand, so the header is dropped.
        let l = layout(above(0, 0, 9));
        assert_eq!(l.rows[1..=9].iter().map(|r| r.a).collect::<Vec<_>>(), (1..=9).collect::<Vec<i32>>());
        assert!(l.rows[1..=9].iter().all(|r| r.kind == RowKind::Expanded));
        assert!(!l.rows.iter().any(|r| r.kind == RowKind::HunkHeader && r.file == 0 && r.a == 0));
        let partial = layout(above(0, 0, 4));
        assert_eq!(partial.rows[1].b, 5);
        // Old numbers follow the hunk's offset (0 here).
        assert!(partial.rows[2].a == 6 && partial.rows[2].b == 6);
    }

    #[test]
    fn expanded_rows_are_found_by_line() {
        let mut opts = above(0, 1, 3);
        opts.expanded_below = HashMap::from([(0, 2)]);
        let l = layout(opts);
        for line in [27, 28, 29, 33, 34] {
            let r = l.row(0, DiffSide::Right, line);
            assert!(r.is_some() && l.rows[r.unwrap()].kind == RowKind::Expanded, "line {line}");
        }
        assert_eq!(l.row(0, DiffSide::Right, 26), None);
    }

    #[test]
    fn split_mode_expands_the_same_way() {
        let mut opts = above(0, 1, 5);
        opts.mode = DiffViewMode::Split;
        let l = layout(opts);
        let h = l.rows.iter().position(|r| r.kind == RowKind::HunkHeader && r.a == 1).unwrap();
        assert_eq!(l.rows[h + 1..=h + 5].iter().map(|r| r.a).collect::<Vec<_>>(), [25, 26, 27, 28, 29]);
        assert_eq!(l.rows[h + 6].kind, RowKind::Pair);
    }

    #[test]
    fn footer_row_changes_when_expanded_below() {
        let mut o = DiffLayoutOptions { show_expand_footer: true, ..Default::default() };
        let footer = |o: &DiffLayoutOptions| *layout(o.clone()).rows.iter().find(|r| r.kind == RowKind::ExpandFooter).unwrap();
        let before = footer(&o);
        o.expanded_below = HashMap::from([(0, 20)]);
        let after = footer(&o);
        assert!(before != after && after.b == 20);
    }

    #[test]
    fn hunk_bounds_with_empty_sides() {
        let hunk = |old_start, old_count, new_start, new_count, n: usize| Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            heading: String::new(),
            line_range: 0..n,
        };
        // "+4,0": nothing added; the hunk sits after new line 4.
        let del = hunk(5, 3, 4, 0, 3);
        assert!(del.new_first() == 5 && del.new_end() == 5 && del.old_first() == 5 && del.old_end() == 8);
        let add = hunk(4, 0, 5, 2, 2);
        assert!(add.old_first() == 5 && add.old_end() == 5 && add.new_first() == 5 && add.new_end() == 7);
        let whole_file_deleted = hunk(1, 3, 0, 0, 3);
        assert_eq!(whole_file_deleted.new_first(), 1);
    }

    #[test]
    fn zero_context_diff_expands_without_gaps_or_overlap() {
        // -U0 style: a pure deletion then a pure addition.
        let d = Arc::new(UnifiedDiffParser::parse(concat!(
            "diff --git a/z.swift b/z.swift\n",
            "--- a/z.swift\n",
            "+++ b/z.swift\n",
            "@@ -5,2 +4,0 @@\n",
            "-gone1\n",
            "-gone2\n",
            "@@ -20,0 +19,2 @@\n",
            "+new1\n",
            "+new2\n",
        )));
        let opts = DiffLayoutOptions {
            expanded_above: HashMap::from([(DiffLayoutOptions::key(0, 0), 100), (DiffLayoutOptions::key(0, 1), 100)]),
            ..Default::default()
        };
        let l = DiffLayout::new(d, opts);
        let rows = expanded(&l);
        // New lines 1-4 sit above the deletion; 5-18 between the hunks; 19-20 are added.
        assert_eq!(rows.iter().map(|r| r.a).collect::<Vec<_>>(), (1..=18).collect::<Vec<i32>>());
        // Old numbering: before the deletion old == new; after it, old = new + 2.
        assert_eq!(rows.iter().find(|r| r.a == 4).unwrap().b, 4);
        assert_eq!(rows.iter().find(|r| r.a == 5).unwrap().b, 7);
        // Both gaps are closed, so neither hunk keeps a header.
        assert!(!l.rows.iter().any(|r| r.kind == RowKind::HunkHeader));
    }

    #[test]
    fn expand_footer_optional() {
        let mut o = DiffLayoutOptions::default();
        assert!(!layout(o.clone()).rows.iter().any(|r| r.kind == RowKind::ExpandFooter));
        o.show_expand_footer = true;
        let l = layout(o);
        // One per rendered (non-placeholder) file, after its last hunk line.
        let files: Vec<i32> = l.rows.iter().filter(|r| r.kind == RowKind::ExpandFooter).map(|r| r.file).collect();
        assert_eq!(files, [0, 2]);
        assert_eq!(l.rows[l.row_range_for_file(0).end - 1].kind, RowKind::ExpandFooter);
    }

    #[test]
    fn generated_detector() {
        assert!(GeneratedFileDetector::is_generated("web/package-lock.json"));
        assert!(GeneratedFileDetector::is_generated("a/b.min.js"));
        assert!(!GeneratedFileDetector::is_generated("src/main.swift"));
    }

    #[test]
    fn split_cache_is_reused() {
        let d = diff();
        let s = DiffLayout::new(d.clone(), DiffLayoutOptions::with_mode(DiffViewMode::Split));
        let s2 = DiffLayout::with_blocks(
            d.clone(),
            DiffLayoutOptions::with_mode(DiffViewMode::Split),
            &HashMap::new(),
            Some(s.split_pairs.clone()),
        );
        assert!(Arc::ptr_eq(&s.split_pairs, &s2.split_pairs));
        assert_eq!(s.rows, s2.rows);
        let u = DiffLayout::new(d, DiffLayoutOptions::default());
        assert!(u.split_pairs.is_empty());
    }

    // MARK: - Performance (budgets ~10x the Swift debug budgets)

    fn huge() -> Arc<DiffSet> {
        static D: std::sync::OnceLock<Arc<DiffSet>> = std::sync::OnceLock::new();
        D.get_or_init(|| Arc::new(UnifiedDiffParser::parse(crate::support::synthetic::huge_fixture())))
            .clone()
    }

    fn time<T>(body: impl FnOnce() -> T) -> (T, f64) {
        let s = Instant::now();
        let r = body();
        (r, s.elapsed().as_secs_f64() * 1000.0)
    }

    #[test]
    fn layout_huge_diff_both_modes() {
        let d = huge();
        let (u, ms_u) = time(|| DiffLayout::new(d.clone(), DiffLayoutOptions::with_mode(DiffViewMode::Unified)));
        let (s, ms_s) = time(|| DiffLayout::new(d.clone(), DiffLayoutOptions::with_mode(DiffViewMode::Split)));
        let (_, ms_s2) = time(|| {
            DiffLayout::with_blocks(
                d.clone(),
                DiffLayoutOptions::with_mode(DiffViewMode::Split),
                &HashMap::new(),
                Some(s.split_pairs.clone()),
            )
        });
        println!(
            "layout unified {} rows: {ms_u:.1}ms; split {} rows: {ms_s:.1}ms; split cached: {ms_s2:.1}ms",
            u.row_count(),
            s.row_count()
        );
        assert!(ms_u < 8000.0 && ms_s < 15000.0 && ms_s2 < 8000.0);
    }

    #[test]
    fn row_lookup_is_fast() {
        let l = DiffLayout::new(huge(), DiffLayoutOptions::default());
        let (_, ms) = time(|| {
            for r in (0..l.row_count()).step_by(7) {
                std::hint::black_box(l.file_index_for_row(r));
            }
        });
        assert!(ms < 2000.0, "{ms}ms");
    }
}
