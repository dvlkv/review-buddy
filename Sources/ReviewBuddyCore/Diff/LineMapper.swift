import Foundation

/// Maps line numbers from an old version of a file to a newer version using the
/// hunks of a diff between them. Used to move Discuss anchors when the PR head moves.
public struct LineMapper: Sendable {
    public enum Mapping: Equatable, Sendable {
        /// The line is unchanged and now lives at this line number.
        case moved(Int)
        /// The line was modified or deleted.
        case changed
    }

    private let hunks: [Hunk]
    private let file: FileDiff?

    /// `file` is the diff from the anchor's commit to the new commit for this path,
    /// or nil when the file did not change between them.
    public init(file: FileDiff?) {
        self.file = file
        self.hunks = file?.hunks ?? []
    }

    public func map(oldLine: Int) -> Mapping {
        guard let file else { return .moved(oldLine) }
        if file.status == .deleted { return .changed }
        var offset = 0
        for h in hunks {
            if h.oldCount == 0 {
                // Pure insertion after line `oldStart`.
                if oldLine > h.oldStart { offset += h.newCount; continue }
                break
            }
            let hOldEnd = h.oldStart + h.oldCount // exclusive
            if oldLine < h.oldStart { break }
            if oldLine >= hOldEnd {
                offset += h.newCount - h.oldCount
                continue
            }
            // Inside the hunk: walk its lines.
            for i in h.lineRange {
                let l = file.lines[i]
                if Int(l.oldLine) == oldLine {
                    return l.kind == .context ? .moved(Int(l.newLine)) : .changed
                }
            }
            return .changed
        }
        return .moved(oldLine + offset)
    }

    /// Maps a whole range; any changed line inside makes the range outdated.
    public func map(range: ClosedRange<Int>) -> ClosedRange<Int>? {
        var mapped: [Int] = []
        for l in range {
            guard case .moved(let n) = map(oldLine: l) else { return nil }
            mapped.append(n)
        }
        guard let lo = mapped.first, let hi = mapped.last, hi - lo == range.count - 1 else { return nil }
        return lo...hi
    }
}

/// Classic Myers O(ND) line diff. Used to remap anchors when we hold both file
/// versions but no diff between them, and in tests as an oracle.
public enum MyersDiff {
    public enum Op: Equatable, Sendable { case equal(Int, Int), insert(Int), delete(Int) }

    public static func diff<T: Hashable>(_ a: [T], _ b: [T]) -> [Op] {
        let n = a.count, m = b.count
        let maxD = n + m
        if maxD == 0 { return [] }
        var v = [Int](repeating: 0, count: 2 * maxD + 2)
        var trace: [[Int]] = []
        let off = maxD + 1
        outer: for d in 0...maxD {
            trace.append(v)
            for k in stride(from: -d, through: d, by: 2) {
                var x: Int
                if k == -d || (k != d && v[off + k - 1] < v[off + k + 1]) {
                    x = v[off + k + 1]
                } else {
                    x = v[off + k - 1] + 1
                }
                var y = x - k
                while x < n && y < m && a[x] == b[y] { x += 1; y += 1 }
                v[off + k] = x
                if x >= n && y >= m { trace.append(v); break outer }
            }
        }
        // Backtrack.
        var ops: [Op] = []
        var x = n, y = m
        for d in stride(from: trace.count - 2, through: 0, by: -1) {
            let vd = trace[d]
            let k = x - y
            let prevK: Int
            if k == -d || (k != d && vd[off + k - 1] < vd[off + k + 1]) { prevK = k + 1 } else { prevK = k - 1 }
            let prevX = vd[off + prevK]
            let prevY = prevX - prevK
            while x > prevX && y > prevY { x -= 1; y -= 1; ops.append(.equal(x, y)) }
            if d > 0 {
                if x == prevX { y -= 1; ops.append(.insert(y)) } else { x -= 1; ops.append(.delete(x)) }
            }
        }
        while x > 0 && y > 0 { x -= 1; y -= 1; ops.append(.equal(x, y)) }
        return ops.reversed()
    }

    /// For each 0-based line of `a`, the 0-based line in `b` it survived as (or nil).
    public static func lineMap<T: Hashable>(_ a: [T], _ b: [T]) -> [Int?] {
        var map = [Int?](repeating: nil, count: a.count)
        for op in diff(a, b) { if case .equal(let i, let j) = op { map[i] = j } }
        return map
    }
}
