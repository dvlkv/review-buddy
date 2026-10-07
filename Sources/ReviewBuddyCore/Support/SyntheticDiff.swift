import Foundation

/// Generates realistic-looking large diffs for benchmarks, tests and the
/// `--synthetic` launch mode used in the performance loop.
public enum SyntheticDiff {
    public struct Shape: Sendable {
        public var files: Int
        public var hunksPerFile: Int
        public var linesPerHunk: Int
        public init(files: Int, hunksPerFile: Int, linesPerHunk: Int) {
            self.files = files; self.hunksPerFile = hunksPerFile; self.linesPerHunk = linesPerHunk
        }
        public static let small = Shape(files: 50, hunksPerFile: 5, linesPerHunk: 20)      // ~5k lines
        public static let large = Shape(files: 500, hunksPerFile: 5, linesPerHunk: 20)     // ~50k lines
        public static let huge = Shape(files: 2000, hunksPerFile: 5, linesPerHunk: 20)     // ~200k lines
    }

    static let swiftLines = [
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
    ]
    static let tsLines = [
        "export async function fetchUser(id: string): Promise<User | null> {",
        "  const res = await fetch(`/api/users/${id}`, { headers: { Accept: 'application/json' } });",
        "  if (!res.ok) return null; // not found",
        "  const data = (await res.json()) as UserPayload;",
        "  return { id: data.id, name: data.display_name ?? 'Unknown', createdAt: new Date(data.created) };",
        "}",
        "",
        "const DEFAULT_TIMEOUT = 30_000;",
    ]

    /// Deterministic diff text. Each hunk has context, a change block and an insertion.
    public static func make(_ shape: Shape) -> String {
        var out = ""
        out.reserveCapacity(shape.files * shape.hunksPerFile * shape.linesPerHunk * 70)
        for f in 0..<shape.files {
            let ts = f % 3 == 0
            let ext = ts ? "ts" : "swift"
            let pool = ts ? tsLines : swiftLines
            let path = "Sources/Module\(f / 50)/Feature\(f)/File\(f).\(ext)"
            out += "diff --git a/\(path) b/\(path)\nindex 1111111..2222222 100644\n--- a/\(path)\n+++ b/\(path)\n"
            var oldLine = 10
            var newLine = 10
            for h in 0..<shape.hunksPerFile {
                let ctx = 3
                let dels = max(1, shape.linesPerHunk / 4)
                let adds = max(1, shape.linesPerHunk - 2 * ctx - dels)
                let oldCount = ctx * 2 + dels
                let newCount = ctx * 2 + adds
                out += "@@ -\(oldLine),\(oldCount) +\(newLine),\(newCount) @@ func section\(h)()\n"
                for k in 0..<ctx { out += " " + pool[(f + h + k) % pool.count] + "\n" }
                for k in 0..<dels { out += "-" + pool[(f + h + k + 3) % pool.count] + "\n" }
                for k in 0..<adds { out += "+" + pool[(f + h + k + 5) % pool.count] + " // v2\n" }
                for k in 0..<ctx { out += " " + pool[(f + h + k + 7) % pool.count] + "\n" }
                oldLine += oldCount + 40
                newLine += newCount + 40
            }
        }
        return out
    }
}
