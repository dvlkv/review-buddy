//! Generates realistic-looking large diffs for benchmarks, tests and the
//! `--synthetic` launch mode used in the performance loop.

use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub files: usize,
    pub hunks_per_file: usize,
    pub lines_per_hunk: usize,
}

impl Shape {
    pub const fn new(files: usize, hunks_per_file: usize, lines_per_hunk: usize) -> Self {
        Shape { files, hunks_per_file, lines_per_hunk }
    }
    /// ~5k lines.
    pub const SMALL: Shape = Shape::new(50, 5, 20);
    /// ~50k lines.
    pub const LARGE: Shape = Shape::new(500, 5, 20);
    /// ~200k lines.
    pub const HUGE: Shape = Shape::new(2000, 5, 20);
}

pub struct SyntheticDiff;

const SWIFT_LINES: [&str; 14] = [
    "    func process(_ items: [Item], options: Options = .default) async throws -> Result {",
    "        let filtered = items.filter { $0.isEnabled && !$0.tags.isEmpty }",
    "        guard let first = filtered.first else { return .empty }",
    "        // Compute the aggregate score across all filtered items",
    "        var total = 0.0",
    "        for item in filtered where item.weight > 0.5 {",
    "            total += item.weight * Double(item.count) /* weighted */",
    "        }",
    "        let message = \"Processed \\(filtered.count) items for \\(first.name)\"",
    "        logger.info(message)",
    "        return Result(score: total, label: first.name, timestamp: Date())",
    "    }",
    "",
    "    @MainActor private var cache: [String: Int] = [:]",
];

const TS_LINES: [&str; 8] = [
    "export async function fetchUser(id: string): Promise<User | null> {",
    "  const res = await fetch(`/api/users/${id}`, { headers: { Accept: 'application/json' } });",
    "  if (!res.ok) return null; // not found",
    "  const data = (await res.json()) as UserPayload;",
    "  return { id: data.id, name: data.display_name ?? 'Unknown', createdAt: new Date(data.created) };",
    "}",
    "",
    "const DEFAULT_TIMEOUT = 30_000;",
];

impl SyntheticDiff {
    /// Deterministic diff text. Each hunk has context, a change block and an insertion.
    pub fn make(shape: Shape) -> String {
        let mut out = String::with_capacity(shape.files * shape.hunks_per_file * shape.lines_per_hunk * 70);
        for f in 0..shape.files {
            let ts = f % 3 == 0;
            let ext = if ts { "ts" } else { "swift" };
            let pool: &[&str] = if ts { &TS_LINES } else { &SWIFT_LINES };
            let path = format!("Sources/Module{}/Feature{f}/File{f}.{ext}", f / 50);
            let _ = write!(
                out,
                "diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n"
            );
            let mut old_line = 10;
            let mut new_line = 10;
            for h in 0..shape.hunks_per_file {
                let ctx = 3;
                let dels = (shape.lines_per_hunk / 4).max(1);
                let adds = shape.lines_per_hunk.saturating_sub(2 * ctx + dels).max(1);
                let old_count = ctx * 2 + dels;
                let new_count = ctx * 2 + adds;
                let _ = writeln!(out, "@@ -{old_line},{old_count} +{new_line},{new_count} @@ func section{h}()");
                let line = |k: usize| pool[(f + h + k) % pool.len()];
                for k in 0..ctx {
                    let _ = writeln!(out, " {}", line(k));
                }
                for k in 0..dels {
                    let _ = writeln!(out, "-{}", line(k + 3));
                }
                for k in 0..adds {
                    let _ = writeln!(out, "+{} // v2", line(k + 5));
                }
                for k in 0..ctx {
                    let _ = writeln!(out, " {}", line(k + 7));
                }
                old_line += old_count + 40;
                new_line += new_count + 40;
            }
        }
        out
    }
}

/// `SyntheticDiff::make(Shape::HUGE)`, generated once and shared by the perf tests.
#[cfg(test)]
pub(crate) fn huge_fixture() -> &'static str {
    static HUGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HUGE.get_or_init(|| SyntheticDiff::make(Shape::HUGE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_well_formed() {
        let a = SyntheticDiff::make(Shape::new(4, 2, 20));
        assert_eq!(a, SyntheticDiff::make(Shape::new(4, 2, 20)));
        assert_eq!(a.matches("diff --git ").count(), 4);
        assert_eq!(a.matches("\n@@ ").count(), 8);
        assert!(a.starts_with("diff --git a/Sources/Module0/Feature0/File0.ts b/Sources/Module0/Feature0/File0.ts\n"));
        assert!(a.contains("@@ -10,11 +10,15 @@ func section0()\n"));
        // Swift's `\\(` escape is kept literally in the generated source text.
        assert!(SyntheticDiff::make(Shape::new(2, 5, 20)).contains("\\(filtered.count)"));
    }
}
