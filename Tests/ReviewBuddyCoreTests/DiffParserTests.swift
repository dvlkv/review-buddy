import Testing
@testable import ReviewBuddyCore

@Suite struct DiffParserTests {
    static let sample = """
    diff --git a/src/app.swift b/src/app.swift
    index 83db48f..bf269f4 100644
    --- a/src/app.swift
    +++ b/src/app.swift
    @@ -1,5 +1,6 @@ struct App {
     import Foundation
    -let a = 1
    +let a = 2
    +let b = 3

     func main() {
    --- not a header, a deleted line starting with --
    @@ -20,2 +21,2 @@
     x
    -y
    +z
    \\ No newline at end of file
    diff --git a/new.txt b/new.txt
    new file mode 100644
    index 0000000..e69de29
    --- /dev/null
    +++ b/new.txt
    @@ -0,0 +1,2 @@
    +hello
    +world
    diff --git a/old.txt b/old.txt
    deleted file mode 100644
    --- a/old.txt
    +++ /dev/null
    @@ -1 +0,0 @@
    -bye
    diff --git a/from.txt b/to.txt
    similarity index 100%
    rename from from.txt
    rename to to.txt
    diff --git a/img.png b/img.png
    Binary files a/img.png and b/img.png differ

    """

    @Test func parsesFilesAndStatuses() {
        let d = UnifiedDiffParser.parse(Self.sample)
        #expect(d.files.map(\.path) == ["src/app.swift", "new.txt", "old.txt", "to.txt", "img.png"])
        #expect(d.files.map(\.status) == [.modified, .added, .deleted, .renamed, .modified])
        #expect(d.files[3].oldPath == "from.txt")
        #expect(d.files[4].isBinary)
    }

    @Test func parsesHunksAndLineNumbers() {
        let d = UnifiedDiffParser.parse(Self.sample)
        let f = d.files[0]
        #expect(f.hunks.count == 2)
        #expect(f.hunks[0].oldStart == 1 && f.hunks[0].oldCount == 5 && f.hunks[0].newStart == 1 && f.hunks[0].newCount == 6)
        #expect(f.hunks[0].heading == "struct App {")
        #expect(f.additions == 3 && f.deletions == 3)
        let first = f.hunks[0].lineRange.map { f.lines[$0] }
        #expect(first.map(\.kind) == [.context, .deletion, .addition, .addition, .context, .context, .deletion])
        #expect(first[1].oldLine == 2 && first[1].newLine == 0)
        #expect(first[3].newLine == 3)
        #expect(first[4].oldLine == 3 && first[4].newLine == 4)
        #expect(d.text(of: first[6]) == "-- not a header, a deleted line starting with --")
        #expect(d.text(of: first[4]) == "")
    }

    @Test func noNewlineMarker() {
        let d = UnifiedDiffParser.parse(Self.sample)
        let f = d.files[0]
        #expect(f.lines[f.hunks[1].lineRange.last!].kind == .noNewline)
    }

    @Test func hunkHeaderWithoutCounts() {
        let d = UnifiedDiffParser.parse(Self.sample)
        let h = d.files[2].hunks[0]
        #expect(h.oldStart == 1 && h.oldCount == 1 && h.newStart == 0 && h.newCount == 0)
    }

    @Test func quotedPathsAndCRLF() {
        let text = "diff --git \"a/sp ace/f\\303\\266.txt\" \"b/sp ace/f\\303\\266.txt\"\r\n--- \"a/sp ace/f\\303\\266.txt\"\r\n+++ \"b/sp ace/f\\303\\266.txt\"\r\n@@ -1 +1 @@\r\n-a\r\n+b\r\n"
        let d = UnifiedDiffParser.parse(text)
        #expect(d.files.count == 1)
        #expect(d.files[0].path == "sp ace/fö.txt")
        #expect(d.text(file: 0, line: 1) == "b")
    }

    @Test func pathWithSpacesInGitHeader() {
        let d = UnifiedDiffParser.parse("diff --git a/my file.txt b/my file.txt\nindex 1..2\n")
        #expect(d.files[0].path == "my file.txt")
    }

    @Test func plainUnifiedDiffWithoutGitHeader() {
        let d = UnifiedDiffParser.parse("--- a/x.c\n+++ b/x.c\n@@ -1 +1 @@\n-1\n+2\n")
        #expect(d.files.count == 1 && d.files[0].path == "x.c" && d.files[0].lines.count == 2)
    }

    @Test func emptyInput() {
        #expect(UnifiedDiffParser.parse("").files.isEmpty)
    }

    @Test func fileIndexLookup() {
        let d = UnifiedDiffParser.parse(Self.sample)
        #expect(d.fileIndex(path: "to.txt") == 3)
        #expect(d.fileIndex(path: "from.txt") == 3)
        #expect(d.fileIndex(path: "nope") == nil)
    }

    @Test func syntheticDiffRoundTrip() {
        let d = UnifiedDiffParser.parse(SyntheticDiff.make(.init(files: 10, hunksPerFile: 3, linesPerHunk: 20)))
        #expect(d.files.count == 10)
        #expect(d.files.allSatisfy { $0.hunks.count == 3 })
        for f in d.files {
            for h in f.hunks {
                let lines = h.lineRange.map { f.lines[$0] }
                #expect(lines.filter { $0.kind != .addition }.count == h.oldCount)
                #expect(lines.filter { $0.kind != .deletion }.count == h.newCount)
            }
        }
    }
}

@Suite struct SplitAlignerTests {
    @Test func pairsDeletionsWithAdditions() {
        let d = UnifiedDiffParser.parse("--- a/x\n+++ b/x\n@@ -1,5 +1,5 @@\n a\n-b\n-c\n+B\n d\n-e\n+E\n+F\n")
        let pairs = SplitAligner.align(file: d.files[0])
        let render = pairs.map { p in
            (p.left >= 0 ? d.text(file: 0, line: Int(p.left)) : "_") + "|" + (p.right >= 0 ? d.text(file: 0, line: Int(p.right)) : "_")
        }
        #expect(render == ["a|a", "b|B", "c|_", "d|d", "e|E", "_|F"])
    }

    @Test func additionsThenDeletionsStartNewBlock() {
        let d = UnifiedDiffParser.parse("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n+n\n-o\n x\n")
        let pairs = SplitAligner.align(file: d.files[0])
        #expect(pairs.count == 3)
        #expect(pairs[0].left == -1 && pairs[1].right == -1)
    }
}

@Suite struct IntralineDiffTests {
    @Test func highlightsChangedWord() {
        let r = IntralineDiff.compute(old: "let value = computeOld(x)", new: "let value = computeNew(x)")
        #expect(r != nil)
        #expect(r!.old == 12..<22) // whole identifier, snapped to word boundary
        #expect(r!.new == 12..<22)
    }
    @Test func identicalReturnsNil() {
        #expect(IntralineDiff.compute(old: "same", new: "same") == nil)
    }
    @Test func totallyDifferentReturnsNil() {
        #expect(IntralineDiff.compute(old: "import Foundation", new: "return value * 42") == nil)
    }
    @Test func insertionOnly() {
        let r = IntralineDiff.compute(old: "foo(a)", new: "foo(a, b)")!
        #expect(r.old.isEmpty)
        #expect(r.new == 5..<8)
    }
}

@Suite struct LineMapperTests {
    // old: 1..10; diff inserts 2 lines after 3, changes line 6.
    static let diff = UnifiedDiffParser.parse("""
    --- a/f
    +++ b/f
    @@ -3,0 +4,2 @@
    +n1
    +n2
    @@ -6 +8 @@
    -six
    +SIX

    """)

    @Test func linesBeforeAnyHunkStay() {
        let m = LineMapper(file: Self.diff.files[0])
        #expect(m.map(oldLine: 1) == .moved(1))
        #expect(m.map(oldLine: 3) == .moved(3))
    }
    @Test func linesAfterInsertionShift() {
        let m = LineMapper(file: Self.diff.files[0])
        #expect(m.map(oldLine: 4) == .moved(6))
        #expect(m.map(oldLine: 5) == .moved(7))
        #expect(m.map(oldLine: 7) == .moved(9))
    }
    @Test func changedLineIsOutdated() {
        let m = LineMapper(file: Self.diff.files[0])
        #expect(m.map(oldLine: 6) == .changed)
        #expect(m.map(range: 5...6) == nil)
        #expect(m.map(range: 4...5) == 6...7)
    }
    @Test func unchangedFile() {
        #expect(LineMapper(file: nil).map(oldLine: 42) == .moved(42))
    }
    @Test func contextInsideHunkMoves() {
        let d = UnifiedDiffParser.parse("--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n a\n+x\n b\n c\n")
        let m = LineMapper(file: d.files[0])
        #expect(m.map(oldLine: 2) == .moved(3))
    }
    @Test func myersAgreesWithMapper() {
        let old = (1...10).map { "l\($0)" }
        var new = old
        new.insert(contentsOf: ["n1", "n2"], at: 3)
        new[7] = "SIX"
        let map = MyersDiff.lineMap(old, new)
        #expect(map[3] == 5) // old line 4 → new line 6 (0-based 3 → 5)
        #expect(map[5] == nil) // line 6 changed
        #expect(map[0] == 0)
    }
    @Test func myersEdgeCases() {
        #expect(MyersDiff.diff([Int](), []) == [])
        #expect(MyersDiff.diff([1], []) == [.delete(0)])
        #expect(MyersDiff.diff([], [1]) == [.insert(0)])
        #expect(MyersDiff.diff([1, 2], [1, 2]) == [.equal(0, 0), .equal(1, 1)])
    }
}
