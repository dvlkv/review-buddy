import Foundation

/// Parses `git diff` / GitHub `.diff` output in a single pass over raw bytes.
///
/// The parser never allocates per diff line: lines are recorded as byte ranges
/// into the source buffer. Hunk bodies are consumed by their declared counts, so
/// a deleted line whose text starts with `-- ` is never mistaken for a file header.
public enum UnifiedDiffParser {
    public static func parse(_ text: String) -> DiffSet {
        parse(bytes: Array(text.utf8))
    }

    public static func parse(data: Data) -> DiffSet {
        parse(bytes: [UInt8](data))
    }

    public static func parse(bytes: [UInt8]) -> DiffSet {
        var files: [FileDiff] = []
        bytes.withUnsafeBufferPointer { buf in
            var state = State(buf: buf)
            state.run(into: &files)
        }
        return DiffSet(buffer: bytes, files: files)
    }

    // MARK: - Implementation

    private struct State {
        let buf: UnsafeBufferPointer<UInt8>
        var pos = 0
        var current: FileDiff?
        var hunkStart = 0
        var oldRemaining = 0
        var newRemaining = 0
        var oldLine: Int32 = 0
        var newLine: Int32 = 0
        var inHunk = false
        var pendingHunk: Hunk?

        init(buf: UnsafeBufferPointer<UInt8>) { self.buf = buf }

        /// Returns the next line as (start, endExcludingNewline) and advances.
        mutating func nextLine() -> (Int, Int)? {
            guard pos < buf.count else { return nil }
            let start = pos
            var i = pos
            let n = buf.count
            // memchr is much faster than a Swift loop on long buffers.
            if let base = buf.baseAddress,
               let hit = memchr(base + i, 0x0A, n - i) {
                i = base.distance(to: hit.assumingMemoryBound(to: UInt8.self))
                pos = i + 1
            } else {
                i = n
                pos = n
            }
            var end = i
            if end > start && buf[end - 1] == 0x0D { end -= 1 }
            return (start, end)
        }

        func hasPrefix(_ start: Int, _ end: Int, _ prefix: StaticString) -> Bool {
            let len = prefix.utf8CodeUnitCount
            guard end - start >= len else { return false }
            let p = prefix.utf8Start
            for k in 0..<len where buf[start + k] != p[k] { return false }
            return true
        }

        func string(_ start: Int, _ end: Int) -> String {
            guard end > start else { return "" }
            return String(decoding: UnsafeBufferPointer(rebasing: buf[start..<end]), as: UTF8.self)
        }

        mutating func finishHunk() {
            guard var h = pendingHunk, var f = current else { return }
            h.lineRange = hunkStart..<f.lines.count
            f.hunks.append(h)
            current = f
            pendingHunk = nil
            inHunk = false
        }

        mutating func finishFile(into files: inout [FileDiff]) {
            finishHunk()
            if let f = current { files.append(f) }
            current = nil
        }

        mutating func run(into files: inout [FileDiff]) {
            while let (s, e) = nextLine() {
                if inHunk {
                    if consumeHunkLine(s, e) { continue }
                    finishHunk()
                }
                if hasPrefix(s, e, "diff --git ") {
                    finishFile(into: &files)
                    let (a, b) = Self.parseGitHeaderPaths(string(s + 11, e))
                    current = FileDiff(oldPath: a, newPath: b, status: .modified)
                } else if hasPrefix(s, e, "@@ ") {
                    if current == nil { current = FileDiff(oldPath: "", newPath: "", status: .modified) }
                    startHunk(s, e)
                } else if current != nil {
                    parseFileHeaderLine(s, e)
                } else if hasPrefix(s, e, "--- ") {
                    // Plain unified diff without a `diff --git` line.
                    current = FileDiff(oldPath: "", newPath: "", status: .modified)
                    parseFileHeaderLine(s, e)
                }
            }
            finishFile(into: &files)
        }

        mutating func parseFileHeaderLine(_ s: Int, _ e: Int) {
            guard var f = current else { return }
            if hasPrefix(s, e, "new file mode") {
                f.status = .added
            } else if hasPrefix(s, e, "deleted file mode") {
                f.status = .deleted
            } else if hasPrefix(s, e, "rename from ") {
                f.oldPath = Self.unquote(string(s + 12, e)); f.status = .renamed
            } else if hasPrefix(s, e, "rename to ") {
                f.newPath = Self.unquote(string(s + 10, e)); f.status = .renamed
            } else if hasPrefix(s, e, "copy from ") {
                f.oldPath = Self.unquote(string(s + 10, e)); f.status = .copied
            } else if hasPrefix(s, e, "copy to ") {
                f.newPath = Self.unquote(string(s + 8, e)); f.status = .copied
            } else if hasPrefix(s, e, "Binary files ") || hasPrefix(s, e, "GIT binary patch") {
                f.isBinary = true
            } else if hasPrefix(s, e, "--- ") {
                let p = Self.stripPrefix(Self.unquote(string(s + 4, e)), "a/")
                if p == "/dev/null" { f.status = .added } else { f.oldPath = p }
            } else if hasPrefix(s, e, "+++ ") {
                let p = Self.stripPrefix(Self.unquote(string(s + 4, e)), "b/")
                if p == "/dev/null" { f.status = .deleted } else { f.newPath = p }
            }
            if f.status == .added && f.oldPath.isEmpty { f.oldPath = f.newPath }
            if f.status == .deleted && f.newPath.isEmpty { f.newPath = f.oldPath }
            if f.oldPath.isEmpty { f.oldPath = f.newPath }
            if f.newPath.isEmpty { f.newPath = f.oldPath }
            current = f
        }

        mutating func startHunk(_ s: Int, _ e: Int) {
            // @@ -a[,b] +c[,d] @@ heading
            var i = s + 3
            func readInt() -> Int {
                var v = 0
                while i < e, buf[i] >= 0x30, buf[i] <= 0x39 { v = v * 10 + Int(buf[i] - 0x30); i += 1 }
                return v
            }
            var oldStart = 0, oldCount = 1, newStart = 0, newCount = 1
            if i < e, buf[i] == UInt8(ascii: "-") { i += 1 }
            oldStart = readInt()
            if i < e, buf[i] == UInt8(ascii: ",") { i += 1; oldCount = readInt() }
            while i < e, buf[i] == UInt8(ascii: " ") { i += 1 }
            if i < e, buf[i] == UInt8(ascii: "+") { i += 1 }
            newStart = readInt()
            if i < e, buf[i] == UInt8(ascii: ",") { i += 1; newCount = readInt() }
            // Skip to closing "@@".
            while i + 1 < e, !(buf[i] == UInt8(ascii: "@") && buf[i + 1] == UInt8(ascii: "@")) { i += 1 }
            i = min(e, i + 2)
            while i < e, buf[i] == UInt8(ascii: " ") { i += 1 }
            let heading = string(i, e)

            hunkStart = current?.lines.count ?? 0
            pendingHunk = Hunk(oldStart: oldStart, oldCount: oldCount, newStart: newStart,
                               newCount: newCount, heading: heading, lineRange: hunkStart..<hunkStart)
            oldRemaining = oldCount
            newRemaining = newCount
            oldLine = Int32(oldStart)
            newLine = Int32(newStart)
            inHunk = true
        }

        /// Returns false when the line is not part of the current hunk.
        mutating func consumeHunkLine(_ s: Int, _ e: Int) -> Bool {
            let first: UInt8 = s < e ? buf[s] : UInt8(ascii: " ") // empty line == empty context
            if first == UInt8(ascii: "\\") {
                append(.noNewline, 0, 0, s, e)
                return true
            }
            if oldRemaining <= 0 && newRemaining <= 0 { return false }
            let textStart = s < e ? s + 1 : s
            switch first {
            case UInt8(ascii: " "):
                append(.context, oldLine, newLine, textStart, e)
                oldLine += 1; newLine += 1
                oldRemaining -= 1; newRemaining -= 1
            case UInt8(ascii: "-"):
                append(.deletion, oldLine, 0, textStart, e)
                oldLine += 1; oldRemaining -= 1
                current!.deletions += 1
            case UInt8(ascii: "+"):
                append(.addition, 0, newLine, textStart, e)
                newLine += 1; newRemaining -= 1
                current!.additions += 1
            default:
                return false
            }
            return true
        }

        @inline(__always)
        mutating func append(_ kind: LineKind, _ o: Int32, _ n: Int32, _ s: Int, _ e: Int) {
            current!.lines.append(DiffLine(kind: kind, oldLine: o, newLine: n,
                                           textStart: Int32(s), textLength: Int32(max(0, e - s))))
        }

        static func stripPrefix(_ s: String, _ p: String) -> String {
            s.hasPrefix(p) ? String(s.dropFirst(p.count)) : s
        }

        /// Undo git's C-style quoting: `"a/f\303\266o bar"`.
        static func unquote(_ raw: String) -> String {
            var s = raw
            if let tab = s.firstIndex(of: "\t") { s = String(s[..<tab]) } // `--- a/x\t2024-...`
            guard s.count >= 2, s.first == "\"", s.last == "\"" else { return s }
            let inner = s.dropFirst().dropLast()
            var out: [UInt8] = []
            var it = Array(inner.utf8).makeIterator()
            while let c = it.next() {
                guard c == UInt8(ascii: "\\"), let n = it.next() else { out.append(c); continue }
                switch n {
                case UInt8(ascii: "n"): out.append(0x0A)
                case UInt8(ascii: "t"): out.append(0x09)
                case UInt8(ascii: "\""): out.append(0x22)
                case UInt8(ascii: "\\"): out.append(0x5C)
                case UInt8(ascii: "0")...UInt8(ascii: "7"):
                    var v = Int(n - 0x30)
                    for _ in 0..<2 {
                        guard let d = it.next() else { break }
                        v = v * 8 + Int(d - 0x30)
                    }
                    out.append(UInt8(truncatingIfNeeded: v))
                default: out.append(n)
                }
            }
            return String(decoding: out, as: UTF8.self)
        }

        /// `a/foo b/foo` → ("foo", "foo"). Ambiguous when paths contain spaces;
        /// `---`/`+++`/`rename` lines override this later.
        static func parseGitHeaderPaths(_ rest: String) -> (String, String) {
            if rest.hasPrefix("\"") {
                // Quoted form: "a/x y" "b/x y"
                let parts = rest.split(separator: "\"", omittingEmptySubsequences: true)
                    .map(String.init).filter { $0 != " " }
                if parts.count >= 2 {
                    return (stripPrefix(unquote("\"\(parts[0])\""), "a/"), stripPrefix(unquote("\"\(parts[1])\""), "b/"))
                }
            }
            // Split at the midpoint " b/" where both halves are equal (common case).
            let chars = Array(rest)
            if chars.count > 4 {
                let mid = (chars.count - 1) / 2
                let left = String(chars[0..<mid]), right = String(chars[(mid + 1)...])
                if left.hasPrefix("a/"), right.hasPrefix("b/"), left.dropFirst(2) == right.dropFirst(2) {
                    return (String(left.dropFirst(2)), String(right.dropFirst(2)))
                }
            }
            if let r = rest.range(of: " b/") {
                return (stripPrefix(String(rest[..<r.lowerBound]), "a/"), String(rest[r.upperBound...]))
            }
            return (rest, rest)
        }
    }
}
