//! Maps line numbers between file versions: through a diff's hunks (`LineMapper`)
//! or by diffing both versions directly (`MyersDiff`).

use crate::models::{FileDiff, FileStatus, LineKind};
use std::ops::RangeInclusive;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    /// The line is unchanged and now lives at this line number.
    Moved(i64),
    /// The line was modified or deleted.
    Changed,
}

/// Maps line numbers from an old version of a file to a newer version using the
/// hunks of a diff between them. Used to move Discuss anchors when the PR head moves.
#[derive(Clone, Copy, Debug)]
pub struct LineMapper<'a> {
    file: Option<&'a FileDiff>,
}

impl<'a> LineMapper<'a> {
    /// `file` is the diff from the anchor's commit to the new commit for this path,
    /// or `None` when the file did not change between them.
    pub fn new(file: Option<&'a FileDiff>) -> Self {
        LineMapper { file }
    }

    pub fn map(&self, old_line: i64) -> Mapping {
        let Some(file) = self.file else { return Mapping::Moved(old_line) };
        if file.status == FileStatus::Deleted {
            return Mapping::Changed;
        }
        let mut offset: i64 = 0;
        for h in &file.hunks {
            let old_start = i64::from(h.old_start);
            let old_count = i64::from(h.old_count);
            let new_count = i64::from(h.new_count);
            if old_count == 0 {
                // Pure insertion after line `old_start`.
                if old_line > old_start {
                    offset += new_count;
                    continue;
                }
                break;
            }
            let h_old_end = old_start + old_count; // exclusive
            if old_line < old_start {
                break;
            }
            if old_line >= h_old_end {
                offset += new_count - old_count;
                continue;
            }
            // Inside the hunk: walk its lines.
            for l in &file.lines[h.line_range.clone()] {
                if i64::from(l.old_line) == old_line {
                    return if l.kind == LineKind::Context {
                        Mapping::Moved(i64::from(l.new_line))
                    } else {
                        Mapping::Changed
                    };
                }
            }
            return Mapping::Changed;
        }
        Mapping::Moved(old_line + offset)
    }

    /// Maps a whole range; any changed line inside makes the range outdated (`None`).
    /// An empty range maps to `None`.
    pub fn map_range(&self, range: RangeInclusive<i64>) -> Option<RangeInclusive<i64>> {
        let (start, end) = (*range.start(), *range.end());
        if start > end {
            return None;
        }
        let mut first = None;
        let mut last = 0;
        for l in range {
            match self.map(l) {
                Mapping::Moved(n) => {
                    first.get_or_insert(n);
                    last = n;
                }
                Mapping::Changed => return None,
            }
        }
        let lo = first?;
        (last - lo == end - start).then_some(lo..=last)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `a[i]` equals `b[j]`.
    Equal(usize, usize),
    /// `b[j]` inserted.
    Insert(usize),
    /// `a[i]` deleted.
    Delete(usize),
}

/// Classic Myers O(ND) line diff. Used to remap anchors when we hold both file
/// versions but no diff between them, and in tests as an oracle.
pub struct MyersDiff;

impl MyersDiff {
    pub fn diff<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Op> {
        let n = a.len() as isize;
        let m = b.len() as isize;
        let max_d = n + m;
        if max_d == 0 {
            return vec![];
        }
        let off = max_d + 1;
        let idx = |k: isize| (off + k) as usize;
        let mut v = vec![0isize; (2 * max_d + 2) as usize];
        let mut trace: Vec<Vec<isize>> = Vec::new();
        'outer: for d in 0..=max_d {
            trace.push(v.clone());
            let mut k = -d;
            while k <= d {
                let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                    v[idx(k + 1)]
                } else {
                    v[idx(k - 1)] + 1
                };
                let mut y = x - k;
                while x < n && y < m && a[x as usize] == b[y as usize] {
                    x += 1;
                    y += 1;
                }
                v[idx(k)] = x;
                if x >= n && y >= m {
                    trace.push(v.clone());
                    break 'outer;
                }
                k += 2;
            }
        }
        // Backtrack.
        let mut ops = Vec::new();
        let (mut x, mut y) = (n, m);
        for d in (0..=(trace.len() as isize - 2)).rev() {
            let vd = &trace[d as usize];
            let k = x - y;
            let prev_k = if k == -d || (k != d && vd[idx(k - 1)] < vd[idx(k + 1)]) { k + 1 } else { k - 1 };
            let prev_x = vd[idx(prev_k)];
            let prev_y = prev_x - prev_k;
            while x > prev_x && y > prev_y {
                x -= 1;
                y -= 1;
                ops.push(Op::Equal(x as usize, y as usize));
            }
            if d > 0 {
                if x == prev_x {
                    y -= 1;
                    ops.push(Op::Insert(y as usize));
                } else {
                    x -= 1;
                    ops.push(Op::Delete(x as usize));
                }
            }
        }
        while x > 0 && y > 0 {
            x -= 1;
            y -= 1;
            ops.push(Op::Equal(x as usize, y as usize));
        }
        ops.reverse();
        ops
    }

    /// For each 0-based line of `a`, the 0-based line in `b` it survived as (or `None`).
    pub fn line_map<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Option<usize>> {
        let mut map = vec![None; a.len()];
        for op in Self::diff(a, b) {
            if let Op::Equal(i, j) = op {
                map[i] = Some(j);
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parser::UnifiedDiffParser;
    use crate::models::DiffSet;

    // old: 1..10; diff inserts 2 lines after 3, changes line 6.
    fn diff() -> DiffSet {
        UnifiedDiffParser::parse(concat!(
            "--- a/f\n",
            "+++ b/f\n",
            "@@ -3,0 +4,2 @@\n",
            "+n1\n",
            "+n2\n",
            "@@ -6 +8 @@\n",
            "-six\n",
            "+SIX\n",
        ))
    }

    #[test]
    fn lines_before_any_hunk_stay() {
        let d = diff();
        let m = LineMapper::new(Some(&d.files[0]));
        assert_eq!(m.map(1), Mapping::Moved(1));
        assert_eq!(m.map(3), Mapping::Moved(3));
    }

    #[test]
    fn lines_after_insertion_shift() {
        let d = diff();
        let m = LineMapper::new(Some(&d.files[0]));
        assert_eq!(m.map(4), Mapping::Moved(6));
        assert_eq!(m.map(5), Mapping::Moved(7));
        assert_eq!(m.map(7), Mapping::Moved(9));
    }

    #[test]
    fn changed_line_is_outdated() {
        let d = diff();
        let m = LineMapper::new(Some(&d.files[0]));
        assert_eq!(m.map(6), Mapping::Changed);
        assert_eq!(m.map_range(5..=6), None);
        assert_eq!(m.map_range(4..=5), Some(6..=7));
    }

    #[test]
    fn unchanged_file() {
        assert_eq!(LineMapper::new(None).map(42), Mapping::Moved(42));
    }

    #[test]
    fn context_inside_hunk_moves() {
        let d = UnifiedDiffParser::parse("--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n a\n+x\n b\n c\n");
        let m = LineMapper::new(Some(&d.files[0]));
        assert_eq!(m.map(2), Mapping::Moved(3));
    }

    #[test]
    fn deleted_file_and_empty_range() {
        let d = UnifiedDiffParser::parse("--- a/f\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n");
        assert_eq!(LineMapper::new(Some(&d.files[0])).map(1), Mapping::Changed);
        #[allow(clippy::reversed_empty_ranges)]
        let empty = 5..=4;
        assert_eq!(LineMapper::new(None).map_range(empty), None);
    }

    #[test]
    fn myers_agrees_with_mapper() {
        let old: Vec<String> = (1..=10).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new.splice(3..3, ["n1".to_string(), "n2".to_string()]);
        new[7] = "SIX".into();
        let map = MyersDiff::line_map(&old, &new);
        assert_eq!(map[3], Some(5)); // old line 4 -> new line 6 (0-based 3 -> 5)
        assert_eq!(map[5], None); // line 6 changed
        assert_eq!(map[0], Some(0));
    }

    #[test]
    fn myers_edge_cases() {
        let empty: [i32; 0] = [];
        assert_eq!(MyersDiff::diff(&empty, &empty), vec![]);
        assert_eq!(MyersDiff::diff(&[1], &empty), vec![Op::Delete(0)]);
        assert_eq!(MyersDiff::diff(&empty, &[1]), vec![Op::Insert(0)]);
        assert_eq!(MyersDiff::diff(&[1, 2], &[1, 2]), vec![Op::Equal(0, 0), Op::Equal(1, 1)]);
    }

    #[test]
    fn myers_ops_reconstruct_b() {
        let a: Vec<char> = "ABCABBA".chars().collect();
        let b: Vec<char> = "CBABAC".chars().collect();
        let ops = MyersDiff::diff(&a, &b);
        let rebuilt: Vec<char> = ops
            .iter()
            .filter_map(|op| match *op {
                Op::Equal(_, j) | Op::Insert(j) => Some(b[j]),
                Op::Delete(_) => None,
            })
            .collect();
        assert_eq!(rebuilt, b);
        // Optimal edit script: D = 5.
        assert_eq!(ops.iter().filter(|o| !matches!(o, Op::Equal(..))).count(), 5);
    }
}
