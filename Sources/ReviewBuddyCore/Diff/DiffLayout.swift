import Foundation

public enum DiffViewMode: String, Codable, Sendable, CaseIterable {
    case unified, split
}

/// Something rendered between diff rows: a review thread, a comment composer,
/// a Discuss marker, or an AI finding. Placed after the row showing `line` on `side`.
public struct InlineBlockRef: Sendable, Hashable {
    public enum Kind: UInt8, Sendable { case reviewThread, composer, discuss, finding }
    public var id: String
    public var kind: Kind
    public var side: DiffSide
    public var line: Int
    public init(id: String, kind: Kind, side: DiffSide, line: Int) {
        self.id = id; self.kind = kind; self.side = side; self.line = line
    }
}

/// A row of the flattened, virtualized diff table. 16 bytes; a 200k-line PR is ~3 MB.
public struct LayoutRow: Sendable, Equatable {
    public enum Kind: UInt8, Sendable {
        /// File header. `a` unused.
        case fileHeader
        /// `a` = hunk index.
        case hunkHeader
        /// Unified line. `a` = index into `FileDiff.lines`.
        case line
        /// Split row. `a` = left line index, `b` = right line index (-1 = empty).
        case pair
        /// Context line loaded from the full file. `a` = new line number, `b` = old line number.
        case expanded
        /// Inline block. `a` = index into `DiffLayout.blocks`.
        case block
        /// Placeholder for a collapsed large/generated/binary file. `a` = reason raw value.
        case placeholder
        /// "Expand below" affordance after the last hunk (only with `showExpandFooter`).
        case expandFooter
    }
    public var kind: Kind
    public var file: Int32
    public var a: Int32
    public var b: Int32
    public init(kind: Kind, file: Int32, a: Int32 = -1, b: Int32 = -1) {
        self.kind = kind; self.file = file; self.a = a; self.b = b
    }
}

public enum PlaceholderReason: Int32, Sendable {
    case binary, large, generated, deletedOnly, empty
}

public struct DiffLayoutOptions: Sendable {
    public var mode: DiffViewMode
    /// Files whose body is hidden (user collapsed, or marked viewed).
    public var collapsed: Set<Int>
    /// Large/generated files the user asked to render anyway.
    public var forceLoaded: Set<Int>
    /// Context lines expanded above hunk `h` of file `f`: key = f << 32 | h.
    public var expandedAbove: [Int64: Int]
    /// Lines expanded below the last hunk of file `f`.
    public var expandedBelow: [Int: Int]
    public var largeFileThreshold: Int
    /// Emit an `.expandFooter` row after each file's last hunk.
    public var showExpandFooter = false

    public init(mode: DiffViewMode = .unified, collapsed: Set<Int> = [], forceLoaded: Set<Int> = [],
                expandedAbove: [Int64: Int] = [:], expandedBelow: [Int: Int] = [:], largeFileThreshold: Int = 2000) {
        self.mode = mode
        self.collapsed = collapsed
        self.forceLoaded = forceLoaded
        self.expandedAbove = expandedAbove
        self.expandedBelow = expandedBelow
        self.largeFileThreshold = largeFileThreshold
    }

    public static func key(file: Int, hunk: Int) -> Int64 { Int64(file) << 32 | Int64(hunk) }
}

/// Flattens a `DiffSet` + inline blocks into table rows. Pure and O(rows), so it
/// can be rebuilt on every structural change (mode switch, collapse, new comment).
public final class DiffLayout: @unchecked Sendable {
    public let diff: DiffSet
    public let options: DiffLayoutOptions
    public private(set) var rows: ContiguousArray<LayoutRow> = []
    public private(set) var blocks: [InlineBlockRef] = []
    /// Row index of each file header.
    public private(set) var fileStartRow: [Int] = []
    /// Split pairs per file (only populated in split mode), cached for reuse.
    public private(set) var splitPairs: [ContiguousArray<SplitPair>] = []

    public init(diff: DiffSet, options: DiffLayoutOptions, blocks blocksByFile: [Int: [InlineBlockRef]] = [:],
                splitCache: [ContiguousArray<SplitPair>]? = nil) {
        self.diff = diff
        self.options = options
        build(blocksByFile: blocksByFile, splitCache: splitCache)
    }

    public var rowCount: Int { rows.count }

    /// Index of the file containing `row` (binary search over file starts).
    public func fileIndex(forRow row: Int) -> Int {
        var lo = 0, hi = fileStartRow.count - 1
        if hi < 0 { return 0 }
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if fileStartRow[mid] <= row { lo = mid } else { hi = mid - 1 }
        }
        return lo
    }

    public func rowRange(forFile f: Int) -> Range<Int> {
        let start = fileStartRow[f]
        let end = f + 1 < fileStartRow.count ? fileStartRow[f + 1] : rows.count
        return start..<end
    }

    /// The row showing `line` on `side` in file `f`, if visible.
    public func row(file f: Int, side: DiffSide, line: Int) -> Int? {
        guard f < fileStartRow.count else { return nil }
        let file = diff.files[f]
        for r in rowRange(forFile: f) {
            let row = rows[r]
            switch row.kind {
            case .line:
                if file.lines[Int(row.a)].lineNumber(on: side) == line { return r }
            case .pair:
                let idx = side == .left ? row.a : row.b
                if idx >= 0, file.lines[Int(idx)].lineNumber(on: side) == line { return r }
            case .expanded:
                if Int(side == .left ? row.b : row.a) == line { return r }
            default: break
            }
        }
        return nil
    }

    public static func placeholderReason(for file: FileDiff, index: Int, options: DiffLayoutOptions,
                                         generated: Bool? = nil) -> PlaceholderReason? {
        if file.isBinary { return .binary }
        if options.forceLoaded.contains(index) { return nil }
        if file.lines.isEmpty { return file.status == .renamed || file.status == .copied ? nil : .empty }
        if generated ?? GeneratedFileDetector.isGenerated(file.path) { return .generated }
        if file.changedLineCount > options.largeFileThreshold { return .large }
        return nil
    }

    // MARK: - Build

    /// Per-file inline block lookup, only built for files that have blocks.
    private struct FileBlocks {
        var left: [Int: [Int32]] = [:]
        var right: [Int: [Int32]] = [:]
        var first: Int32 = 0
        var end: Int32 = 0

        @inline(__always)
        func emit(after line: DiffLine, file: Int32, into rows: inout ContiguousArray<LayoutRow>) {
            if line.newLine > 0, line.kind != .deletion, let ids = right[Int(line.newLine)] {
                for id in ids { rows.append(LayoutRow(kind: .block, file: file, a: id)) }
            }
            if line.oldLine > 0, line.kind != .addition, let ids = left[Int(line.oldLine)] {
                for id in ids { rows.append(LayoutRow(kind: .block, file: file, a: id)) }
            }
        }
    }

    // Written without nested closures over `rows`: capturing it would box the array and
    // route every append through the box (5x slower with bounds checks on).
    private func build(blocksByFile: [Int: [InlineBlockRef]], splitCache: [ContiguousArray<SplitPair>]?) {
        let files = diff.files
        var rows = ContiguousArray<LayoutRow>()
        rows.reserveCapacity(diff.totalLines + files.count * 4)
        var starts: [Int] = []
        starts.reserveCapacity(files.count)
        var allBlocks: [InlineBlockRef] = []
        let split = options.mode == .split
        var pairsCache: [ContiguousArray<SplitPair>] = []
        if split {
            if let splitCache, splitCache.count == files.count {
                pairsCache = splitCache
            } else {
                pairsCache = files.map { SplitAligner.align(file: $0) }
            }
        }
        let hasExpansions = !options.expandedAbove.isEmpty || !options.expandedBelow.isEmpty

        for fi in files.indices {
            let file = files[fi]
            let f32 = Int32(fi)
            starts.append(rows.count)
            rows.append(LayoutRow(kind: .fileHeader, file: f32))
            if options.collapsed.contains(fi) { continue }
            if let reason = Self.placeholderReason(for: file, index: fi, options: options, generated: diff.isGenerated[fi]) {
                rows.append(LayoutRow(kind: .placeholder, file: f32, a: reason.rawValue))
                continue
            }

            var fb: FileBlocks?
            if let bs = blocksByFile[fi], !bs.isEmpty {
                var x = FileBlocks()
                x.first = Int32(allBlocks.count)
                for b in bs {
                    let idx = Int32(allBlocks.count)
                    allBlocks.append(b)
                    if b.side == .left { x.left[b.line, default: []].append(idx) } else { x.right[b.line, default: []].append(idx) }
                }
                x.end = Int32(allBlocks.count)
                fb = x
            }
            let fileRowStart = rows.count
            let pairs = split ? pairsCache[fi] : []
            var pairCursor = 0
            var prevNewEnd = 1 // first new line not yet shown

            file.lines.withUnsafeBufferPointer { lines in
                for hi in file.hunks.indices {
                    let hunk = file.hunks[hi]
                    if hasExpansions, let n = options.expandedAbove[DiffLayoutOptions.key(file: fi, hunk: hi)], n > 0 {
                        let lo = max(prevNewEnd, hunk.newStart - n)
                        if lo < hunk.newStart {
                            Self.emitExpanded(lo..<hunk.newStart, oldOffset: hunk.newStart - hunk.oldStart, file: f32, blocks: fb, into: &rows)
                        }
                    }
                    rows.append(LayoutRow(kind: .hunkHeader, file: f32, a: Int32(hi)))
                    if split {
                        // Pairs for this hunk are contiguous in `pairs`.
                        while pairCursor < pairs.count {
                            let p = pairs[pairCursor]
                            let anyIdx = Int(p.left >= 0 ? p.left : p.right)
                            if anyIdx >= hunk.lineRange.upperBound { break }
                            pairCursor += 1
                            rows.append(LayoutRow(kind: .pair, file: f32, a: p.left, b: p.right))
                            if let fb {
                                if p.right >= 0 { fb.emit(after: lines[Int(p.right)], file: f32, into: &rows) }
                                if p.left >= 0 && p.left != p.right {
                                    let l = lines[Int(p.left)]
                                    if l.kind == .deletion, let ids = fb.left[Int(l.oldLine)] {
                                        for id in ids { rows.append(LayoutRow(kind: .block, file: f32, a: id)) }
                                    }
                                }
                            }
                        }
                    } else if let fb {
                        for i in hunk.lineRange {
                            rows.append(LayoutRow(kind: .line, file: f32, a: Int32(i)))
                            fb.emit(after: lines[i], file: f32, into: &rows)
                        }
                    } else {
                        for i in hunk.lineRange { rows.append(LayoutRow(kind: .line, file: f32, a: Int32(i))) }
                    }
                    prevNewEnd = hunk.newStart + hunk.newCount
                }
            }
            if hasExpansions, let n = options.expandedBelow[fi], n > 0, let last = file.hunks.last {
                let oldOffset = (last.newStart + last.newCount) - (last.oldStart + last.oldCount)
                Self.emitExpanded(prevNewEnd..<(prevNewEnd + n), oldOffset: oldOffset, file: f32, blocks: fb, into: &rows)
            }
            if options.showExpandFooter && !file.hunks.isEmpty && file.status != .deleted {
                rows.append(LayoutRow(kind: .expandFooter, file: f32))
            }
            // Blocks whose line is not visible (outdated threads, lines outside hunks)
            // go at the end of the file so nothing is silently hidden.
            if let fb {
                var shown = Set<Int32>()
                for r in fileRowStart..<rows.count where rows[r].kind == .block { shown.insert(rows[r].a) }
                for id in fb.first..<fb.end where !shown.contains(id) {
                    rows.append(LayoutRow(kind: .block, file: f32, a: id))
                }
            }
        }
        self.rows = rows
        self.fileStartRow = starts
        self.blocks = allBlocks
        self.splitPairs = pairsCache
    }

    private static func emitExpanded(_ newRange: Range<Int>, oldOffset: Int, file: Int32, blocks: FileBlocks?,
                                     into rows: inout ContiguousArray<LayoutRow>) {
        for n in newRange {
            rows.append(LayoutRow(kind: .expanded, file: file, a: Int32(n), b: Int32(n - oldOffset)))
            if let ids = blocks?.right[n] { for id in ids { rows.append(LayoutRow(kind: .block, file: file, a: id)) } }
        }
    }
}

/// Heuristics matching GitHub's linguist "generated" defaults for common files.
public enum GeneratedFileDetector {
    static let exactNames: Set<String> = [
        "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "Gemfile.lock",
        "Podfile.lock", "composer.lock", "poetry.lock", "go.sum", "Package.resolved", "bun.lockb", "uv.lock",
    ]
    static let suffixes: [String] = [
        ".min.js", ".min.css", ".pb.go", "_pb2.py", ".pbxproj", ".snap", ".generated.swift", ".g.dart", ".map",
    ]

    public static func isGenerated(_ path: String) -> Bool {
        let name = path.split(separator: "/").last.map(String.init) ?? path
        if exactNames.contains(name) { return true }
        for s in suffixes where name.hasSuffix(s) { return true }
        return path.contains("/__generated__/") || path.hasPrefix("dist/") || path.contains("/dist/")
            || path.hasPrefix("vendor/") || path.contains("/node_modules/")
    }
}
