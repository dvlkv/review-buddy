import Foundation
import Testing
@testable import ReviewBuddyCore

/// Performance budgets for the large-PR pipeline. Budgets are for debug builds
/// (what `swift test` runs); release is ~5–10x faster. See PERF.md.
@Suite(.serialized) struct PerformanceTests {
    static let huge = SyntheticDiff.make(.huge)

    func time<T>(_ body: () -> T) -> (T, Double) {
        let s = ContinuousClock.now
        let r = body()
        return (r, (ContinuousClock.now - s).milliseconds)
    }

    @Test func parseHugeDiff() {
        let bytes = Array(Self.huge.utf8)
        let (d, ms) = time { UnifiedDiffParser.parse(bytes: bytes) }
        print("parse \(bytes.count / 1_000_000)MB, \(d.totalLines) lines: \(ms)ms")
        #expect(d.files.count == 2000)
        #expect(d.totalLines >= 190_000)
        #expect(ms < 1500)
    }

    @Test func layoutHugeDiffBothModes() {
        let d = UnifiedDiffParser.parse(Self.huge)
        let (u, msU) = time { DiffLayout(diff: d, options: .init(mode: .unified)) }
        let (s, msS) = time { DiffLayout(diff: d, options: .init(mode: .split)) }
        let (_, msS2) = time { DiffLayout(diff: d, options: .init(mode: .split), splitCache: s.splitPairs) }
        print("layout unified \(u.rowCount) rows: \(msU)ms; split \(s.rowCount) rows: \(msS)ms; split cached: \(msS2)ms")
        #expect(msU < 800 && msS < 1500 && msS2 < 800)
    }

    @Test func highlightLargestFile() {
        let d = UnifiedDiffParser.parse(SyntheticDiff.make(.init(files: 1, hunksPerFile: 200, linesPerHunk: 50)))
        let (t, ms) = time { DiffHighlighter.highlight(file: d.files[0], in: d) }
        print("highlight \(t.count) lines: \(ms)ms")
        #expect(t.count == d.files[0].lines.count)
        #expect(ms < 1500)
    }

    @Test func rowLookupIsFast() {
        let d = UnifiedDiffParser.parse(Self.huge)
        let l = DiffLayout(diff: d, options: .init())
        let (_, ms) = time { for r in stride(from: 0, to: l.rowCount, by: 7) { _ = l.fileIndex(forRow: r) } }
        #expect(ms < 200)
    }
}
