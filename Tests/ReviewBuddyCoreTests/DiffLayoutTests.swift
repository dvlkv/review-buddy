import Testing
@testable import ReviewBuddyCore

@Suite struct DiffLayoutTests {
    static let diff = UnifiedDiffParser.parse("""
    diff --git a/a.swift b/a.swift
    --- a/a.swift
    +++ b/a.swift
    @@ -10,3 +10,3 @@
     one
    -two
    +TWO
     three
    @@ -30,2 +30,3 @@
     x
    +y
     z
    diff --git a/package-lock.json b/package-lock.json
    --- a/package-lock.json
    +++ b/package-lock.json
    @@ -1 +1 @@
    -{}
    +{ }
    diff --git a/b.swift b/b.swift
    --- a/b.swift
    +++ b/b.swift
    @@ -1 +1 @@
    -old
    +new

    """)

    @Test func unifiedRows() {
        let l = DiffLayout(diff: Self.diff, options: .init(mode: .unified))
        #expect(l.fileStartRow == [0, 10, 12])
        #expect(l.rows[0].kind == .fileHeader)
        #expect(l.rows[1].kind == .hunkHeader)
        #expect(l.rows[2...5].allSatisfy { $0.kind == .line })
        #expect(l.rows[11].kind == .placeholder && l.rows[11].a == PlaceholderReason.generated.rawValue)
        #expect(l.rowCount == 16)
    }

    @Test func splitRows() {
        let l = DiffLayout(diff: Self.diff, options: .init(mode: .split))
        // file0: header, hunk, 3 pairs, hunk, 3 pairs
        #expect(l.rowRange(forFile: 0).count == 1 + 1 + 3 + 1 + 3)
        let pair = l.rows[3]
        #expect(pair.kind == .pair)
        #expect(Self.diff.text(file: 0, line: Int(pair.a)) == "two")
        #expect(Self.diff.text(file: 0, line: Int(pair.b)) == "TWO")
    }

    @Test func collapsedAndForceLoaded() {
        let l = DiffLayout(diff: Self.diff, options: .init(collapsed: [0], forceLoaded: [1]))
        #expect(l.rowRange(forFile: 0).count == 1)
        #expect(l.rows[l.fileStartRow[1] + 1].kind == .hunkHeader)
    }

    @Test func largeFileBecomesPlaceholder() {
        let l = DiffLayout(diff: Self.diff, options: .init(largeFileThreshold: 1))
        #expect(l.rows[1].kind == .placeholder && l.rows[1].a == PlaceholderReason.large.rawValue)
    }

    @Test func blocksPlacedAfterTheirLine() {
        let blocks = [0: [InlineBlockRef(id: "t1", kind: .reviewThread, side: .right, line: 11),
                          InlineBlockRef(id: "t2", kind: .discuss, side: .left, line: 11),
                          InlineBlockRef(id: "gone", kind: .reviewThread, side: .right, line: 999)]]
        for mode in DiffViewMode.allCases {
            let l = DiffLayout(diff: Self.diff, options: .init(mode: mode), blocks: blocks)
            let ids = l.rows.filter { $0.kind == .block }.map { l.blocks[Int($0.a)].id }
            // Unified: "-two" (old 11) precedes "+TWO" (new 11). Split: both on one row, right first.
            #expect(ids == (mode == .unified ? ["t2", "t1", "gone"] : ["t1", "t2", "gone"]), "mode \(mode)")
            // t1 follows the row containing new line 11 ("TWO")
            let r = l.row(file: 0, side: .right, line: 11)!
            #expect(l.rows[r + 1].kind == .block)
        }
    }

    @Test func fileIndexForRowBinarySearch() {
        let l = DiffLayout(diff: Self.diff, options: .init())
        #expect(l.fileIndex(forRow: 0) == 0)
        #expect(l.fileIndex(forRow: 9) == 0)
        #expect(l.fileIndex(forRow: 10) == 1)
        #expect(l.fileIndex(forRow: 14) == 2)
    }

    @Test func expandedContextAboveAndBelow() {
        let opts = DiffLayoutOptions(expandedAbove: [DiffLayoutOptions.key(file: 0, hunk: 1): 5], expandedBelow: [0: 2])
        let l = DiffLayout(diff: Self.diff, options: opts)
        let expanded = l.rows.filter { $0.kind == .expanded }
        #expect(expanded.map(\.a) == [25, 26, 27, 28, 29, 33, 34])
        // Old numbering offset after hunk 0 is 0, after hunk 1 is +1.
        #expect(expanded.first!.b == 25)
        #expect(expanded.last!.b == 33)
        #expect(l.row(file: 0, side: .right, line: 27) != nil)
    }

    @Test func expandedAboveClampsToPreviousHunk() {
        let opts = DiffLayoutOptions(expandedAbove: [DiffLayoutOptions.key(file: 0, hunk: 1): 100])
        let l = DiffLayout(diff: Self.diff, options: opts)
        let expanded = l.rows.filter { $0.kind == .expanded }
        #expect(expanded.first!.a == 13)
        #expect(expanded.last!.a == 29)
    }

    /// Regression: the header used to come *after* lines expanded above it, so each click
    /// pushed the ⤒ arrow 20 rows down and the next click landed on code.
    @Test func hunkHeaderStaysAboveExpandedContext() {
        let key = DiffLayoutOptions.key(file: 0, hunk: 1)
        var headerRows: [Int] = []
        var hidden: [Int32] = []
        for n in [0, 5, 10, 17, 100] {
            let l = DiffLayout(diff: Self.diff, options: DiffLayoutOptions(expandedAbove: [key: n]))
            let h = l.rows.firstIndex { $0.kind == .hunkHeader && $0.file == 0 && $0.a == 1 }!
            headerRows.append(h)
            hidden.append(l.rows[h].b)
            // Expanded lines follow the header, contiguous and ending right before the hunk.
            let exp = l.rows[(h + 1)...].prefix { $0.kind == .expanded }.map { Int($0.a) }
            #expect(exp == Array(max(13, 30 - n)..<30))
        }
        #expect(Set(headerRows).count == 1)
        // 13..<30 is hidden at first: 17 lines.
        #expect(hidden == [17, 12, 7, 0, 0])
    }

    @Test func expandingTheFirstHunkReachesLineOne() {
        let key = DiffLayoutOptions.key(file: 0, hunk: 0)
        let l = DiffLayout(diff: Self.diff, options: DiffLayoutOptions(expandedAbove: [key: 9]))
        #expect(l.rows[1].kind == .hunkHeader && l.rows[1].b == 0)
        #expect(l.rows[2...10].map(\.a) == Array(1...9).map(Int32.init))
        #expect(l.rows[2...10].allSatisfy { $0.kind == .expanded })
        let partial = DiffLayout(diff: Self.diff, options: DiffLayoutOptions(expandedAbove: [key: 4]))
        #expect(partial.rows[1].b == 5)
        // Old numbers follow the hunk's offset (0 here).
        #expect(partial.rows[2].a == 6 && partial.rows[2].b == 6)
    }

    @Test func expandedRowsAreFoundByLine() {
        let opts = DiffLayoutOptions(expandedAbove: [DiffLayoutOptions.key(file: 0, hunk: 1): 3], expandedBelow: [0: 2])
        let l = DiffLayout(diff: Self.diff, options: opts)
        for line in [27, 28, 29, 33, 34] {
            let r = l.row(file: 0, side: .right, line: line)
            #expect(r != nil && l.rows[r!].kind == .expanded)
        }
        #expect(l.row(file: 0, side: .right, line: 26) == nil)
    }

    @Test func splitModeExpandsTheSameWay() {
        var opts = DiffLayoutOptions(expandedAbove: [DiffLayoutOptions.key(file: 0, hunk: 1): 5])
        opts.mode = .split
        let l = DiffLayout(diff: Self.diff, options: opts)
        let h = l.rows.firstIndex { $0.kind == .hunkHeader && $0.a == 1 }!
        #expect(l.rows[(h + 1)...(h + 5)].map(\.a) == [25, 26, 27, 28, 29])
        #expect(l.rows[h + 6].kind == .pair)
    }

    @Test func footerRowChangesWhenExpandedBelow() {
        var o = DiffLayoutOptions()
        o.showExpandFooter = true
        let before = DiffLayout(diff: Self.diff, options: o).rows.first { $0.kind == .expandFooter }!
        o.expandedBelow = [0: 20]
        let after = DiffLayout(diff: Self.diff, options: o).rows.first { $0.kind == .expandFooter }!
        #expect(before != after && after.b == 20)
    }

    @Test func hunkBoundsWithEmptySides() {
        // "+4,0": nothing added; the hunk sits after new line 4.
        let del = Hunk(oldStart: 5, oldCount: 3, newStart: 4, newCount: 0, heading: "", lineRange: 0..<3)
        #expect(del.newFirst == 5 && del.newEnd == 5 && del.oldFirst == 5 && del.oldEnd == 8)
        let add = Hunk(oldStart: 4, oldCount: 0, newStart: 5, newCount: 2, heading: "", lineRange: 0..<2)
        #expect(add.oldFirst == 5 && add.oldEnd == 5 && add.newFirst == 5 && add.newEnd == 7)
        let wholeFileDeleted = Hunk(oldStart: 1, oldCount: 3, newStart: 0, newCount: 0, heading: "", lineRange: 0..<3)
        #expect(wholeFileDeleted.newFirst == 1)
    }

    @Test func zeroContextDiffExpandsWithoutGapsOrOverlap() {
        // -U0 style: a pure deletion then a pure addition.
        let d = UnifiedDiffParser.parse("""
        diff --git a/z.swift b/z.swift
        --- a/z.swift
        +++ b/z.swift
        @@ -5,2 +4,0 @@
        -gone1
        -gone2
        @@ -20,0 +19,2 @@
        +new1
        +new2

        """)
        let opts = DiffLayoutOptions(expandedAbove: [DiffLayoutOptions.key(file: 0, hunk: 0): 100,
                                                     DiffLayoutOptions.key(file: 0, hunk: 1): 100])
        let l = DiffLayout(diff: d, options: opts)
        let shown = l.rows.filter { $0.kind == .expanded }.map { Int($0.a) }
        // New lines 1-4 sit above the deletion; 5-18 between the hunks; 19-20 are added.
        #expect(shown == Array(1...18))
        // Old numbering: before the deletion old == new; after it, old = new + 2.
        let rows = l.rows.filter { $0.kind == .expanded }
        #expect(rows.first { $0.a == 4 }!.b == 4)
        #expect(rows.first { $0.a == 5 }!.b == 7)
        #expect(l.rows.filter { $0.kind == .hunkHeader }.map(\.b) == [0, 0])
    }

    @Test func expandFooterOptional() {
        var o = DiffLayoutOptions()
        #expect(!DiffLayout(diff: Self.diff, options: o).rows.contains { $0.kind == .expandFooter })
        o.showExpandFooter = true
        let l = DiffLayout(diff: Self.diff, options: o)
        // One per rendered (non-placeholder) file, after its last hunk line.
        #expect(l.rows.filter { $0.kind == .expandFooter }.map(\.file) == [0, 2])
        #expect(l.rows[l.rowRange(forFile: 0).upperBound - 1].kind == .expandFooter)
    }

    @Test func generatedDetector() {
        #expect(GeneratedFileDetector.isGenerated("web/package-lock.json"))
        #expect(GeneratedFileDetector.isGenerated("a/b.min.js"))
        #expect(!GeneratedFileDetector.isGenerated("src/main.swift"))
    }
}
