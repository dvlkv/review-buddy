import AppKit
import Foundation
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

@MainActor
private func expandedLines(_ s: PRStore, file: Int) -> [Int] {
    s.layout!.rows.filter { $0.kind == .expanded && $0.file == Int32(file) }.map { Int($0.a) }
}

@MainActor
private func headerRow(_ s: PRStore, file: Int, hunk: Int) -> Int {
    s.layout!.rows.firstIndex { $0.kind == .hunkHeader && $0.file == Int32(file) && $0.a == Int32(hunk) }!
}

/// Expands above `hunk` until nothing is hidden, waiting for each step to land.
@MainActor
private func expandFully(_ s: PRStore, file: Int, hunk: Int) async -> Int {
    var clicks = 0
    while s.canExpandAbove(file: file, hunk: hunk), clicks < 100 {
        let v = s.layoutVersion
        s.expandAbove(file: file, hunk: hunk)
        clicks += 1
        _ = await eventually { s.layoutVersion != v }
    }
    return clicks
}

@MainActor
@Suite struct ExpandContextTests {
    @Test func repeatedExpandAboveFillsTheGapAndStops() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let f = s.files[0]
        let gap = f.hunks[1].newFirst - f.hunks[0].newEnd
        #expect(gap > 20) // needs at least two clicks
        let header = headerRow(s, file: 0, hunk: 1)
        let clicks = await expandFully(s, file: 0, hunk: 1)
        #expect(clicks == (gap + 19) / 20)
        #expect(!s.canExpandAbove(file: 0, hunk: 1))
        // Every hidden line shown exactly once, in order, right after the header.
        #expect(expandedLines(s, file: 0) == Array(f.hunks[0].newEnd..<f.hunks[1].newFirst))
        #expect(headerRow(s, file: 0, hunk: 1) == header)
        #expect(s.layout!.rows[header].b == 0)
        // Never counts past the gap, so later clicks are no-ops.
        #expect(s.options.expandedAbove[DiffLayoutOptions.key(file: 0, hunk: 1)] == gap)
        let v = s.layoutVersion
        s.expandAbove(file: 0, hunk: 1)
        try? await Task.sleep(for: .milliseconds(30))
        #expect(s.layoutVersion == v)
    }

    @Test func expandAboveFirstHunkReachesLineOne() async {
        let h = Harness()
        await h.load()
        let s = h.store
        _ = await expandFully(s, file: 2, hunk: 0)
        #expect(expandedLines(s, file: 2) == Array(1..<s.files[2].hunks[0].newFirst))
        #expect(!s.canExpandAbove(file: 2, hunk: 0))
    }

    @Test func rapidClicksFetchTheFileOnce() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { d in
            flaky = FlakyGitHub(base: d)
            flaky.contents = { path, ref in
                try await Task.sleep(for: .milliseconds(80))
                return Data((1...400).map { "\(path) \($0)" }.joined(separator: "\n").utf8)
            }
            return flaky
        }, worktrees: false)
        await h.load()
        let s = h.store
        let gap = s.files[0].hunks[1].newFirst - s.files[0].hunks[0].newEnd
        s.expandAbove(file: 0, hunk: 1)
        s.expandAbove(file: 0, hunk: 1)
        #expect(await eventually { expandedLines(s, file: 0).count == min(40, gap) })
        #expect(flaky.contentCalls == 1)
    }

    @Test func expandBelowStopsAtEndOfFile() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let lastEnd = s.files[0].hunks.last!.newEnd
        let total = lastEnd + 45
        h.worktrees.fileContentsText = (1...total).map { "l\($0)" }.joined(separator: "\n") + "\n"
        _ = try? await h.store.ensureWorktree()
        for _ in 0..<10 {
            let v = s.layoutVersion
            s.expandBelow(file: 0)
            if !(await eventually(timeout: 0.3) { s.layoutVersion != v }) { break }
        }
        // The trailing newline doesn't add a phantom empty line 61.
        #expect(expandedLines(s, file: 0).last == total)
        #expect(expandedLines(s, file: 0).first == lastEnd)
        #expect(!s.canExpandBelow(file: 0))
    }

    @Test func checkoutWithoutTheCommitFallsBackToGitHub() async {
        let h = Harness()
        h.worktrees.fileContentsError = TestError(message: "fatal: invalid object name")
        await h.load()
        _ = try? await h.store.ensureWorktree()
        h.store.expandAbove(file: 0, hunk: 1)
        #expect(await eventually { !expandedLines(h.store, file: 0).isEmpty })
        #expect(h.worktrees.calls.contains { $0.hasPrefix("fileContents:") })
        #expect(h.demo.calls.contains("fileContents"))
        #expect(h.store.banner == nil)
    }

    @Test func splitLinesMatchesDiffNumbering() {
        #expect(PRStore.splitLines(Data("a\nb\n".utf8)) == ["a", "b"])
        #expect(PRStore.splitLines(Data("a\nb".utf8)) == ["a", "b"])
        #expect(PRStore.splitLines(Data("a\r\nb\r\n".utf8)) == ["a", "b"])
        #expect(PRStore.splitLines(Data("a\n\n".utf8)) == ["a", ""])
        #expect(PRStore.splitLines(Data()) == [])
        #expect(PRStore.splitLines(Data("é\r\n日本\n".utf8)) == ["é", "日本"])
    }
}

@MainActor
@Suite struct RefreshTests {
    static let newShape = SyntheticDiff.Shape(files: testShape.files, hunksPerFile: 3, linesPerHunk: 7)

    /// Regression: after a push, a refresh in split view kept the old layout's split pairs
    /// (same file count), so rows pointed at the old diff's lines.
    @Test func refreshInSplitModeLaysOutTheNewDiff() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.setMode(.split)
        _ = await expandFully(s, file: 0, hunk: 1)
        let oldText = s.files[0].lines.map { s.diff!.text(of: $0) }
        h.demo.simulatePush(headOid: String(repeating: "c", count: 40), diff: Data(SyntheticDiff.make(Self.newShape).utf8))
        await s.refresh()
        #expect(s.detail?.headOid == String(repeating: "c", count: 40))
        #expect(s.files[0].hunks.count == 3)
        #expect(s.files[0].lines.map { s.diff!.text(of: $0) } != oldText)
        let fresh = DiffLayout(diff: s.diff!, options: s.options, blocks: s.blocksByFile())
        #expect(s.layout!.rows == fresh.rows)
        for r in s.layout!.rows where r.kind == .pair {
            let n = Int32(s.files[Int(r.file)].lines.count)
            #expect(r.a < n && r.b < n)
        }
        // Expansions and file text were for the old head.
        #expect(s.options.expandedAbove.isEmpty && s.options.expandedBelow.isEmpty)
        #expect(s.fileContentCache.isEmpty)
        #expect(expandedLines(s, file: 0).isEmpty)
    }

    /// Regression: pushing fix commits left the diff on the old head until the app was reopened.
    @Test func pushReloadsTheDiff() async {
        let h = Harness()
        await h.load()
        let s = h.store
        try! await s.ensureWorktree()
        h.worktrees.localCommitsValue = [CommitInfo(sha: "c1", subject: "fix", author: "me", date: nil)]
        await s.refreshGitState()
        let newHead = String(repeating: "c", count: 40)
        h.demo.simulatePush(headOid: newHead, diff: Data(SyntheticDiff.make(Self.newShape).utf8))
        #expect(await s.push())
        #expect(s.detail?.headOid == newHead)
        #expect(s.files[0].hunks.count == 3)
    }

    /// A Discuss agent that ran `git push` itself: the turn's end reloads the PR.
    @Test func discussTurnThatPushedReloadsTheDiff() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let id = s.startDiscuss(CodeAnchor(path: s.files[0].path, side: .right, startLine: 13, endLine: 13))!
        let c = s.controller(for: id)
        await c.send("push it")
        let newHead = String(repeating: "c", count: 40)
        h.worktrees.head = newHead
        h.demo.simulatePush(headOid: newHead, diff: Data(SyntheticDiff.make(Self.newShape).utf8))
        h.sessions.sessions[0].reply("Pushed.")
        #expect(await eventually { s.detail?.headOid == newHead })
        #expect(s.files[0].hunks.count == 3)
    }

    /// Someone else pushed: don't reload under the user, just flag it as outdated.
    @Test func discussTurnDoesNotReloadForOthersPushes() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let id = s.startDiscuss(CodeAnchor(path: s.files[0].path, side: .right, startLine: 13, endLine: 13))!
        let c = s.controller(for: id)
        await c.send("hi")
        let oldHead = s.detail?.headOid
        h.demo.simulatePush(headOid: String(repeating: "d", count: 40))
        h.sessions.sessions[0].reply("Hello.")
        #expect(await eventually { s.isOutdated })
        #expect(s.detail?.headOid == oldHead)
    }

    @Test func contentArrivingAfterRefreshIsDropped() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { d in
            flaky = FlakyGitHub(base: d)
            flaky.contents = { _, ref in
                try await Task.sleep(for: .milliseconds(150))
                return Data((1...400).map { "OLD \(ref) \($0)" }.joined(separator: "\n").utf8)
            }
            return flaky
        }, worktrees: false)
        await h.load()
        let s = h.store
        s.expandAbove(file: 0, hunk: 1)
        h.demo.simulatePush(headOid: String(repeating: "c", count: 40), diff: Data(SyntheticDiff.make(Self.newShape).utf8))
        await s.refresh()
        try? await Task.sleep(for: .milliseconds(300))
        #expect(s.fileContentCache.isEmpty)
        #expect(s.options.expandedAbove.isEmpty)
        #expect(expandedLines(s, file: 0).isEmpty)
    }

    @Test func largeDiffRefreshFetchesExistingCheckout() async {
        let h = Harness(github: { d in
            let f = FlakyGitHub(base: d)
            f.diffTooLarge = true
            return f
        })
        await h.load()
        #expect(h.store.diffSource == "local git")
        let fetchesBefore = h.worktrees.calls.filter { $0 == "fetchPR" }.count
        await h.store.load()
        let calls = h.worktrees.calls
        #expect(calls.filter { $0 == "fetchPR" }.count == fetchesBefore + 1)
        // Fetched before diffing, so the diff is of the new head.
        #expect(calls.lastIndex(of: "fetchPR")! < calls.lastIndex(of: "diff")!)
    }

    @Test func staleDiffFromGitHubIsRetried() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { d in
            flaky = FlakyGitHub(base: d)
            // One file short, as if from the previous head.
            let full = SyntheticDiff.make(testShape)
            let lastFile = full.range(of: "diff --git", options: .backwards)!.lowerBound
            flaky.queuedDiffs = [Data(full[..<lastFile].utf8)]
            return flaky
        })
        h.store.staleDiffRetryDelay = .zero
        await h.load()
        #expect(h.store.files.count == testShape.files)
        #expect(h.demo.calls.filter { $0 == "diff" }.count == 1) // the retry reached the real diff
    }

    @Test func matchingDiffIsNotRefetched() async {
        let h = Harness()
        h.store.staleDiffRetryDelay = .seconds(10)
        await h.load()
        #expect(h.demo.calls.filter { $0 == "diff" }.count == 1)
        #expect(PRStore.diff(h.store.diff!, matches: h.store.detail!))
    }

    @Test func localDiffIsPinnedToTheLoadedHead() async {
        let h = Harness(github: { d in
            let f = FlakyGitHub(base: d)
            f.diffTooLarge = true
            return f
        })
        await h.load()
        let head = h.store.detail!.headOid
        #expect(h.worktrees.calls.contains("diff:\(head)"))
        // Not fetched locally: fall back to the fetched branch tip rather than failing.
        h.worktrees.unknownHeads = [head]
        await h.store.load()
        #expect(h.store.loadState == .loaded)
        #expect(h.worktrees.calls.last { $0.hasPrefix("diff:") } == "diff:refs/review-buddy/head-1")
    }

    @Test func refreshReturnsToTheSameFile() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let target = s.files[3].path
        h.view.top = s.layout!.fileStartRow[3] + 4
        // The new diff drops file 0, renumbering every row.
        var text = SyntheticDiff.make(testShape)
        let second = text.range(of: "diff --git", range: text.index(after: text.startIndex)..<text.endIndex)!.lowerBound
        text.removeSubrange(text.startIndex..<second)
        h.demo.simulatePush(headOid: String(repeating: "c", count: 40), diff: Data(text.utf8))
        s.staleDiffRetryDelay = .zero
        await s.refresh()
        let f = s.diff!.fileIndex(path: target)!
        #expect(h.view.scrolls.last == s.layout!.fileStartRow[f] + 4)
    }

    /// Two refreshes overlap and the older one's diff arrives last: the newer diff stays.
    @Test func olderLoadFinishingLastDoesNotWin() async {
        var flaky: FlakyGitHub!
        let oldText = SyntheticDiff.make(testShape)
        let h = Harness(github: { d in
            flaky = FlakyGitHub(base: d)
            return flaky
        })
        await h.load()
        let s = h.store
        // Load #2 gets the old diff, slowly; load #3 gets the new one at once.
        flaky.queuedDiffs = []
        flaky.beforeDiff = { n in if n == 1 { try? await Task.sleep(for: .milliseconds(200)) } }
        let newText = SyntheticDiff.make(RefreshTests.newShape)
        let slow = Task { await s.load() }
        try? await Task.sleep(for: .milliseconds(30))
        h.demo.simulatePush(headOid: String(repeating: "c", count: 40), diff: Data(newText.utf8))
        await s.load()
        await slow.value
        #expect(s.detail?.headOid == String(repeating: "c", count: 40))
        #expect(s.files[0].hunks.count == 3)
        #expect(s.loadState == .loaded)
        _ = oldText
    }

    @Test func requestsBypassTheHTTPCache() async throws {
        let (c, t) = makeRecordingClient()
        _ = try? await c.diff(PRRef(owner: "o", repo: "r", number: 1))
        _ = try? await c.headOid(PRRef(owner: "o", repo: "r", number: 1))
        #expect(t.requests.count == 2)
        #expect(t.requests.allSatisfy { $0.cachePolicy == .reloadIgnoringLocalCacheData })
    }
}

final class RecordingTransport: HTTPTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var _requests: [URLRequest] = []
    var requests: [URLRequest] { lock.withLock { _requests } }
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        lock.withLock { _requests.append(request) }
        return (Data("{}".utf8), HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!)
    }
}

func makeRecordingClient() -> (GitHubClient, RecordingTransport) {
    let t = RecordingTransport()
    return (GitHubClient(transport: t, tokens: StaticTokenProvider("t")), t)
}

@MainActor
@Suite struct ExpandCanvasTests {
    static func click(_ c: DiffTableCoordinator, at p: CGPoint) {
        let e = NSEvent.mouseEvent(with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
                                   context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        _ = c.handleMouseDown(at: p, event: e)
    }

    static func scroll(_ rig: CanvasRig, toY y: CGFloat) {
        rig.scroll.contentView.scroll(to: CGPoint(x: 0, y: y))
        rig.scroll.reflectScrolledClipView(rig.scroll.contentView)
    }

    /// Clicking the same spot keeps expanding: the arrow stays under the pointer.
    @Test func arrowStaysUnderThePointer() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator, s = rig.h.store
        let row = headerRow(s, file: 1, hunk: 1)
        let screenY: CGFloat = 200
        Self.scroll(rig, toY: c.rect(ofRow: row).minY - screenY)
        let p = CGPoint(x: 4, y: c.rect(ofRow: row).midY)
        var clicks = 0
        while s.canExpandAbove(file: 1, hunk: 1), clicks < 20 {
            let v = s.layoutVersion
            Self.click(c, at: p)
            clicks += 1
            #expect(await eventually { s.layoutVersion != v })
            let r = headerRow(s, file: 1, hunk: 1)
            #expect(c.rect(ofRow: r).minY - rig.scroll.contentView.bounds.minY == screenY)
            #expect(c.row(at: p) == r)
        }
        #expect(clicks >= 2)
        #expect(!s.canExpandAbove(file: 1, hunk: 1))
    }

    /// Regression: with a hunk header at the top of the view, expanding jumped to the file header.
    @Test func expandingWithHeaderAtTopDoesNotJump() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator, s = rig.h.store
        let row = headerRow(s, file: 2, hunk: 1)
        let top = c.rect(ofRow: row).minY
        Self.scroll(rig, toY: top)
        let v = s.layoutVersion
        s.expandAbove(file: 2, hunk: 1)
        #expect(await eventually { s.layoutVersion != v })
        #expect(rig.scroll.contentView.bounds.minY == c.rect(ofRow: headerRow(s, file: 2, hunk: 1)).minY)
    }
}
