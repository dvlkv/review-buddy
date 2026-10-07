import Foundation

/// One visual row of a side-by-side diff. Indices point into `FileDiff.lines`; -1 means empty.
public struct SplitPair: Sendable, Equatable {
    public var left: Int32
    public var right: Int32
    public init(left: Int32, right: Int32) { self.left = left; self.right = right }
}

/// Aligns a hunk's lines into side-by-side rows the way GitHub does: context lines
/// share a row, and inside each change block the i-th deletion is paired with the
/// i-th addition. Linear in the number of lines.
public enum SplitAligner {
    public static func align(lines: ContiguousArray<DiffLine>, range: Range<Int>, into out: inout ContiguousArray<SplitPair>) {
        var dels: [Int32] = []
        var adds: [Int32] = []
        func flush() {
            let n = max(dels.count, adds.count)
            for k in 0..<n {
                out.append(SplitPair(left: k < dels.count ? dels[k] : -1,
                                     right: k < adds.count ? adds[k] : -1))
            }
            dels.removeAll(keepingCapacity: true)
            adds.removeAll(keepingCapacity: true)
        }
        for i in range {
            let line = lines[i]
            switch line.kind {
            case .deletion:
                // A deletion after additions starts a new change block.
                if !adds.isEmpty { flush() }
                dels.append(Int32(i))
            case .addition:
                adds.append(Int32(i))
            case .context:
                flush()
                out.append(SplitPair(left: Int32(i), right: Int32(i)))
            case .noNewline:
                // Attach the marker to whichever side the previous line was on.
                flush()
                let prev = i > range.lowerBound ? lines[i - 1].kind : .context
                switch prev {
                case .deletion: out.append(SplitPair(left: Int32(i), right: -1))
                case .addition: out.append(SplitPair(left: -1, right: Int32(i)))
                default: out.append(SplitPair(left: Int32(i), right: Int32(i)))
                }
            }
        }
        flush()
    }

    public static func align(file: FileDiff) -> ContiguousArray<SplitPair> {
        var out = ContiguousArray<SplitPair>()
        out.reserveCapacity(file.lines.count)
        for h in file.hunks { align(lines: file.lines, range: h.lineRange, into: &out) }
        return out
    }
}

/// Character ranges that differ between a paired deletion and addition, used to
/// emphasize the changed part of a line (GitHub's darker word highlight).
public enum IntralineDiff {
    public struct Result: Sendable, Equatable {
        /// UTF-16 range of the changed part in the old line.
        public var old: Range<Int>
        /// UTF-16 range of the changed part in the new line.
        public var new: Range<Int>
    }

    /// Common prefix/suffix trimming on UTF-16 units, snapped to token boundaries.
    /// Returns nil when the lines are too different for emphasis to be useful.
    public static func compute(old: String, new: String) -> Result? {
        let a = Array(old.utf16), b = Array(new.utf16)
        if a == b { return nil }
        var p = 0
        let maxP = min(a.count, b.count)
        while p < maxP && a[p] == b[p] { p += 1 }
        var s = 0
        while s < maxP - p && a[a.count - 1 - s] == b[b.count - 1 - s] { s += 1 }
        // Snap to word boundaries so we highlight whole identifiers.
        while p > 0 && isWord(a[p - 1]) && (p < a.count && isWord(a[p]) || p < b.count && isWord(b[p])) { p -= 1 }
        while s > 0 && isWord(a[a.count - s]) && (a.count - s - 1 >= p && isWord(a[a.count - s - 1]) || b.count - s - 1 >= p && isWord(b[b.count - s - 1])) { s -= 1 }
        let oldRange = p..<(a.count - s)
        let newRange = p..<(b.count - s)
        // If almost everything changed, emphasis is noise.
        let changed = max(oldRange.count, newRange.count)
        let total = max(a.count, b.count)
        if total > 0 && Double(changed) / Double(total) > 0.75 && changed > 6 { return nil }
        return Result(old: oldRange, new: newRange)
    }

    @inline(__always) static func isWord(_ c: UInt16) -> Bool {
        (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || c > 127
    }
}
