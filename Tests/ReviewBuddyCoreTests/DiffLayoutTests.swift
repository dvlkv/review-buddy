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
