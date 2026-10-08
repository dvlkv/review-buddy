import Foundation
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

@MainActor
@Suite struct PRStoreLoadTests {
    @Test func loadInstallsEverything() async {
        let h = Harness()
        await h.load()
        let s = h.store
        #expect(s.loadState == .loaded)
        #expect(s.detail?.ref == Harness.ref)
        #expect(s.files.count == 6)
        #expect(s.layout != nil && s.layout!.rowCount > 0)
        #expect(s.threads["T0"] != nil)
        #expect(s.viewed.isEmpty)
        #expect(s.pendingReviewId == nil)
        #expect(s.diffSource == "GitHub API")
        #expect(h.view.layoutChanges > 0)
        #expect(h.demo.calls.contains("pullRequest") && h.demo.calls.contains("diff"))
    }

    @Test func installWithDetailMarksViewedCollapsed() async {
        let h = Harness()
        var d = try! await h.demo.pullRequest(Harness.ref)
        d.files[2].viewed = .viewed
        d.pendingReviewId = "R9"
        let diff = UnifiedDiffParser.parse(data: try! await h.demo.diff(Harness.ref))
        h.store.install(diff: diff, detail: d)
        #expect(h.store.isViewed(2))
        #expect(h.store.options.collapsed == [2])
        #expect(h.store.pendingReviewId == "R9")
        #expect(h.store.viewedCount == 1)
        #expect(h.store.layout!.rowRange(forFile: 2).count == 1)
    }

    @Test func layoutRebuildsOnModeCollapseForceLoad() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let v0 = s.layoutVersion
        s.setMode(.split)
        #expect(s.viewMode == .split && s.layout!.options.mode == .split)
        #expect(s.services.settings.viewMode == .split)
        #expect(s.layout!.rows.contains { $0.kind == .pair })
        s.toggleMode()
        #expect(s.viewMode == .unified)
        s.toggleCollapsed(1)
        #expect(s.layout!.rowRange(forFile: 1).count == 1)
        s.toggleCollapsed(1)
        #expect(s.layout!.rowRange(forFile: 1).count > 1)
        s.options.collapsed.insert(3)
        s.forceLoad(3)
        #expect(s.options.forceLoaded.contains(3) && !s.options.collapsed.contains(3))
        #expect(s.layoutVersion > v0 + 3)
    }

    @Test func applySettingsUsesThreshold() async {
        let h = Harness()
        await h.load()
        h.services.settings.largeFileThreshold = 1
        h.store.applySettings()
        #expect(h.store.layout!.rows.contains { $0.kind == .placeholder })
    }
}

@MainActor
@Suite struct PRStoreViewedTests {
    @Test func toggleViewedIsOptimisticAndCallsGitHub() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.toggleViewed(1)
        #expect(s.isViewed(1))
        #expect(s.options.collapsed.contains(1))
        #expect(h.view.scrolls.last == s.layout!.fileStartRow[1])
        #expect(await eventually { h.demo.calls.contains("setViewed:true") })
        s.toggleViewed(1)
        #expect(!s.isViewed(1) && !s.options.collapsed.contains(1))
        #expect(await eventually { h.demo.calls.contains("setViewed:false") })
    }

    @Test func viewedRevertsOnFailure() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { flaky = FlakyGitHub(base: $0); flaky.failViewed = true; return flaky })
        await h.load()
        h.store.toggleViewed(0)
        #expect(h.store.isViewed(0))
        #expect(await eventually { !h.store.isViewed(0) })
        #expect(h.store.banner?.contains("viewed failed") == true)
    }

    @Test func outOfRangeIsIgnored() async {
        let h = Harness()
        await h.load()
        h.store.toggleViewed(99)
        #expect(!h.store.isViewed(99))
    }
}

@MainActor
@Suite struct PRStoreSelectionTests {
    @Test func selectExtendAndLines() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.select(file: 0, side: .right, line: 10, extend: false)
        s.select(file: 0, side: .right, line: 13, extend: true)
        let sel = s.selection!
        #expect(sel.anchor == CodeAnchor(path: s.files[0].path, side: .right, startLine: 10, endLine: 13))
        #expect(sel.contains(side: .right, line: 12) && !sel.contains(side: .left, line: 12))
        let lines = s.lines(for: sel.anchor)
        #expect(lines.count == 4)
        #expect(lines[3].hasSuffix("// v2")) // line 13 is the first addition
        // Different side starts a new selection.
        s.select(file: 0, side: .left, line: 14, extend: true)
        #expect(s.selection!.side == .left && s.selection!.start == 14)
        // Left side excludes additions.
        #expect(s.lines(for: CodeAnchor(path: s.files[0].path, side: .left, startLine: 13, endLine: 15)).allSatisfy { !$0.hasSuffix("// v2") })
        s.select(file: 0, side: .right, line: 0, extend: false) // invalid line ignored
        #expect(s.selection!.start == 14)
        s.clearSelection()
        #expect(s.selection == nil)
        #expect(s.lines(for: CodeAnchor(path: "nope", side: .right, startLine: 1, endLine: 2)).isEmpty)
    }

    @Test func textSelectionWithinAndAcrossLines() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let line10 = s.lines(for: CodeAnchor(path: s.files[0].path, side: .right, startLine: 10, endLine: 10))[0]
        // Within one line, dragged right to left.
        s.select(file: 0, side: .right, line: 10, extend: false, column: 6)
        s.select(file: 0, side: .right, line: 10, extend: true, column: 2)
        #expect(s.selection!.isPartial)
        #expect(s.selection!.columnRange(line: 10) == 2..<6)
        #expect(s.selectedText() == String(Array(line10)[2..<6]))
        // Comments still anchor on whole lines.
        #expect(s.selection!.anchor.startLine == 10 && s.selection!.anchor.endLine == 10)
        // Across lines: the tail of the first line, the head of the last.
        s.select(file: 0, side: .right, line: 10, extend: false, column: 3)
        s.select(file: 0, side: .right, line: 12, extend: true, column: 4)
        let sel = s.selection!
        #expect(sel.columnRange(line: 10) == 3..<Int.max)
        #expect(sel.columnRange(line: 11) == 0..<Int.max)
        #expect(sel.columnRange(line: 12) == 0..<4)
        #expect(sel.columnRange(line: 13) == nil)
        let text = s.selectedText()!.components(separatedBy: "\n")
        #expect(text.count == 3 && text[0] == String(line10.dropFirst(3)) && text[2].count == 4)
        // A click without a drag is an empty selection (the canvas clears it).
        s.select(file: 0, side: .right, line: 10, extend: false, column: 5)
        #expect(s.isEmptyTextSelection)
        s.select(file: 0, side: .right, line: 10, extend: true, column: 7)
        #expect(!s.isEmptyTextSelection)
        // Whole-line selections (gutter) are never "empty".
        s.select(file: 0, side: .right, line: 10, extend: false)
        #expect(!s.isEmptyTextSelection && s.selectedText() == line10)
    }

    @Test func doubleClickWordRange() {
        let t = "  const res = await fetch(`/api/${id}`);"
        func word(_ col: Int) -> String {
            let r = DiffTableCoordinator.wordRange(in: t, at: col)
            return String(Array(t)[r])
        }
        #expect(word(4) == "const")
        #expect(word(2) == "const")   // first character
        #expect(word(9) == "res")
        #expect(word(18) == "await")
        #expect(word(12) == "=")       // punctuation selects one character
        #expect(word(32) == "$")       // `$` joins identifiers, but `{` does not
        #expect(word(35) == "id")
        #expect(word(999) == ";")      // past the end: last character
        #expect(DiffTableCoordinator.wordRange(in: "", at: 3) == 0..<0)
    }
}

@MainActor
@Suite struct ReviewOrderTests {
    @Test func readingOrderMovesFilesToTopAndCanBeEdited() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let original = s.files.map(\.path)
        // The diff follows the tree's order once loaded.
        #expect(original == FileTreeBuilder.displayOrder(paths: original).map { original[$0] })
        let a = original[original.count - 1], b = original[original.count / 2]
        s.setSummary(AISummary(summary: "x", readingOrder: [.init(path: a, reason: "core"), .init(path: b, reason: "uses")], risks: []),
                     state: .idle)
        #expect(s.files.prefix(2).map(\.path) == [a, b])
        #expect(s.reviewFirstCount == 2)
        #expect(s.files.count == original.count && Set(s.files.map(\.path)) == Set(original))
        // Viewed state follows the path, not the index.
        s.toggleViewed(0)
        #expect(s.isViewed(s.diff!.fileIndex(path: a)!))
        s.moveInReviewOrder(b, by: -1)
        #expect(s.files.prefix(2).map(\.path) == [b, a])
        let c = original[1]
        s.addToReviewOrder(c)
        #expect(s.files.prefix(3).map(\.path) == [b, a, c])
        s.removeFromReviewOrder(a)
        #expect(s.files.prefix(2).map(\.path) == [b, c] && s.reviewFirstCount == 2)
        s.resetReviewOrder()
        #expect(s.files.prefix(2).map(\.path) == [a, b])
    }
}

@MainActor
@Suite struct ConversationCommentTests {
    @Test func postsAndAppendsComment() async {
        let h = Harness()
        await h.load()
        let before = h.store.detail?.conversation?.comments.count ?? 0
        #expect(await h.store.addConversationComment("  ") == nil) // blank: nothing sent
        #expect(await h.store.addConversationComment("Looks good") == nil)
        #expect(h.store.detail?.conversation?.comments.count == before + 1)
        #expect(h.store.detail?.conversation?.comments.last?.body == "Looks good")
    }
}

@MainActor
@Suite struct SingleCommentTests {
    @Test func singleCommentPublishesWithoutReview() async throws {
        let h = Harness()
        await h.load()
        let s = h.store
        #expect(s.pendingReviewId == nil)
        s.select(file: 1, side: .right, line: 13, extend: false)
        s.startComment()
        let c = s.composers.values.first!
        s.updateComposer(c.id, text: "nit")
        await s.submitComposer(c.id, single: true)
        #expect(s.composers.isEmpty)
        #expect(s.pendingReviewId == nil) // no review started
        let t = try #require(s.threads.values.first { $0.comments.first?.body == "nit" })
        #expect(t.comments.allSatisfy { !$0.isPending })
        #expect(h.demo.calls.contains("addSingleComment") && !h.demo.calls.contains("startReview"))

        s.startReply(threadId: t.id)
        let r = s.composers.values.first!
        s.updateComposer(r.id, text: "done")
        await s.submitComposer(r.id, single: true)
        #expect(s.threads[t.id]?.comments.map(\.body) == ["nit", "done"])
        #expect(s.pendingReviewId == nil)
    }
}

@MainActor
@Suite struct UpdateCheckTests {
    @Test func detectsPushAndRefreshUpdatesCheckout() async throws {
        let h = Harness()
        await h.load()
        _ = try await h.store.ensureWorktree()
        await h.store.checkForUpdates(force: true)
        #expect(!h.store.isOutdated)
        h.demo.simulatePush(headOid: String(repeating: "c", count: 40))
        await h.store.checkForUpdates(force: true)
        #expect(h.store.isOutdated)
        let before = h.worktrees.calls.filter { $0 == "ensureWorktree" }.count
        await h.store.refresh()
        #expect(!h.store.isOutdated)
        #expect(h.store.detail?.headOid == String(repeating: "c", count: 40))
        // Refresh goes through ensureWorktree, which fast-forwards a clean checkout.
        #expect(h.worktrees.calls.filter { $0 == "ensureWorktree" }.count == before + 1)
    }
}

@MainActor
@Suite struct MergeTests {
    @Test func mergeReloadsAsMerged() async {
        let h = Harness()
        await h.load()
        #expect(h.store.detail?.state == "OPEN")
        #expect(await h.store.mergePullRequest(method: .squash) == nil)
        #expect(h.store.detail?.state == "MERGED")
    }

    @Test func closeAndReopen() async {
        let h = Harness()
        await h.load()
        #expect(await h.store.setClosed(true) == nil)
        #expect(h.store.detail?.state == "CLOSED")
        #expect(await h.store.setClosed(false) == nil)
        #expect(h.store.detail?.state == "OPEN")
    }

    @Test func canMergeFollowsMergeState() {
        var st = PRMergeStatus(mergeable: "MERGEABLE", mergeStateStatus: "CLEAN")
        #expect(st.canMerge)
        #expect(!st.canBypass)
        st.mergeStateStatus = "BLOCKED"
        #expect(!st.canMerge && !st.canBypass)
        st.viewerCanMergeAsAdmin = true
        #expect(!st.canMerge && st.canBypass) // only with the bypass checkbox
        st.mergeable = "CONFLICTING"
        #expect(!st.canMerge && !st.canBypass)
    }
}

@MainActor
@Suite struct PRStoreCommentTests {
    func anchor(_ s: PRStore, _ f: Int, _ line: Int) -> CodeAnchor {
        CodeAnchor(path: s.files[f].path, side: .right, startLine: line, endLine: line)
    }

    @Test func pendingReviewCreatedOnceForTwoComments() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let before = s.threads.count
        s.select(file: 1, side: .right, line: 13, extend: false)
        s.startComment()
        #expect(s.selection == nil)
        let c1 = s.composers.values.first!
        #expect(c1.anchor == anchor(s, 1, 13))
        #expect(c1.anchorLines.count == 1)
        #expect(s.layout!.blocks.contains { $0.id == c1.id && $0.kind == .composer })
        s.updateComposer(c1.id, text: "first")
        await s.submitComposer(c1.id)
        #expect(s.composers.isEmpty)
        #expect(s.pendingReviewId != nil)
        s.startComment(anchor(s, 2, 14), text: "second")
        await s.submitComposer(s.composers.values.first!.id)
        #expect(h.demo.calls.filter { $0 == "startReview" }.count == 1)
        #expect(h.demo.calls.filter { $0 == "addThread" }.count == 2)
        #expect(s.threads.count == before + 2)
        #expect(s.pendingCommentCount == 2)
        #expect(s.openThreadCountByPath[s.files[1].path] == 1)
    }

    @Test func emptyComposerNotSubmittedAndReuseSameAnchor() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.startComment(anchor(s, 1, 13))
        s.startComment(anchor(s, 1, 13), text: "filled")
        #expect(s.composers.count == 1)
        #expect(s.composers.values.first!.text == "filled")
        s.updateComposer(s.composers.values.first!.id, text: "   ")
        await s.submitComposer(s.composers.values.first!.id)
        #expect(!h.demo.calls.contains("addThread"))
        let id = s.composers.values.first!.id
        s.cancelComposer(id)
        #expect(s.composers.isEmpty)
        #expect(!s.layout!.blocks.contains { $0.id == id })
    }

    @Test func startCommentWithoutSelectionDoesNothing() async {
        let h = Harness()
        await h.load()
        h.store.startComment()
        #expect(h.store.composers.isEmpty)
    }

    @Test func replyEditDelete() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.startReply(threadId: "T0")
        s.startReply(threadId: "T0")
        #expect(s.composers.count == 1)
        let reply = s.composerFor(reply: "T0")!
        s.updateComposer(reply.id, text: "thanks")
        await s.submitComposer(reply.id)
        #expect(s.threads["T0"]!.comments.count == 3)
        #expect(s.threads["T0"]!.comments.last!.isPending)
        let last = s.threads["T0"]!.comments.last!
        s.startEdit(threadId: "T0", comment: last)
        let edit = s.composerFor(edit: last.id)!
        #expect(edit.text == "thanks")
        s.updateComposer(edit.id, text: "thanks!!")
        await s.submitComposer(edit.id)
        #expect(s.threads["T0"]!.comments.last!.body == "thanks!!")
        #expect(h.demo.calls.contains("updateComment"))
        await s.deleteComment(threadId: "T0", commentId: last.id)
        #expect(s.threads["T0"]!.comments.count == 2)
        for c in s.threads["T0"]!.comments { await s.deleteComment(threadId: "T0", commentId: c.id) }
        #expect(s.threads["T0"] == nil)
    }

    @Test func resolveAndRevertOnFailure() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { flaky = FlakyGitHub(base: $0); return flaky })
        await h.load()
        let s = h.store
        #expect(s.threads["T0"]!.isResolved) // synthetic T0 starts resolved
        await s.setResolved(threadId: "T0", false)
        #expect(!s.threads["T0"]!.isResolved)
        #expect(s.openThreadCountByPath[s.files[0].path] == 1)
        flaky.failResolve = true
        await s.setResolved(threadId: "T0", true)
        #expect(!s.threads["T0"]!.isResolved)
        #expect(s.banner != nil)
    }

    @Test func submitAndDiscardReview() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.startComment(anchor(s, 1, 13), text: "c")
        await s.submitComposer(s.composers.values.first!.id)
        s.reviewBody = "LGTM"
        let ok = await s.submitReview(event: .approve)
        #expect(ok)
        #expect(h.demo.calls.contains("submitReview:APPROVE"))
        #expect(s.pendingReviewId == nil && s.reviewBody.isEmpty)
        #expect(s.pendingCommentCount == 0)
        #expect(s.banner == "Approved ✓")
        #expect(s.submitState == .idle)

        s.startComment(anchor(s, 2, 13), text: "d")
        await s.submitComposer(s.composers.values.first!.id)
        let threadsBefore = s.threads.count
        await s.discardPendingReview()
        #expect(h.demo.calls.contains("deleteReview"))
        #expect(s.pendingReviewId == nil)
        #expect(s.threads.count == threadsBefore - 1)
    }

    @Test func discardWithoutPendingIsNoop() async {
        let h = Harness()
        await h.load()
        await h.store.discardPendingReview()
        #expect(!h.demo.calls.contains("deleteReview"))
    }
}

@MainActor
@Suite struct PRStoreNavigationTests {
    @Test func navigateFilesHunksThreads() async {
        let h = Harness(threadsEvery: 2)
        await h.load()
        let s = h.store
        let l = s.layout!
        h.view.top = 0
        s.navigate(.nextFile)
        #expect(h.view.scrolls.last == l.fileStartRow[1])
        s.navigate(.nextFile)
        #expect(h.view.scrolls.last == l.fileStartRow[2])
        h.view.top = l.fileStartRow[2] + 3
        s.navigate(.previousFile)
        #expect(h.view.scrolls.last == l.fileStartRow[2])
        s.navigate(.previousFile)
        #expect(h.view.scrolls.last == l.fileStartRow[1])
        h.view.top = 0
        s.navigate(.nextHunk)
        let firstHunk = l.rows.firstIndex { $0.kind == .hunkHeader }!
        #expect(h.view.scrolls.last == firstHunk)
        s.navigate(.nextHunk)
        #expect(l.rows[h.view.scrolls.last!].kind == .hunkHeader && h.view.scrolls.last! > firstHunk)
        s.navigate(.previousHunk)
        #expect(h.view.scrolls.last == firstHunk)
        h.view.top = 0
        s.navigate(.nextThread)
        let firstBlock = l.rows.firstIndex { $0.kind == .block }!
        #expect(h.view.scrolls.last == max(0, firstBlock - 3))
        h.view.top = l.rowCount - 1
        let n = h.view.scrolls.count
        s.navigate(.nextFile) // past the end: no scroll
        #expect(h.view.scrolls.count == n)
        s.navigate(.previousThread)
        #expect(h.view.scrolls.count == n + 1)
    }

    @Test func scrollHelpers() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.scrollToFile(3)
        #expect(h.view.scrolls.last == s.layout!.fileStartRow[3])
        s.scrollToBlock(id: "T0")
        let blockRow = s.layout!.rows.firstIndex { $0.kind == .block }!
        #expect(h.view.scrolls.last == max(0, blockRow - 4))
        s.toggleCollapsed(4)
        s.scrollTo(anchor: CodeAnchor(path: s.files[4].path, side: .right, startLine: 13, endLine: 13))
        #expect(!s.options.collapsed.contains(4))
        let row = s.layout!.row(file: 4, side: .right, line: 13)!
        #expect(h.view.scrolls.last == row - 4)
    }

    @Test func reportTopRowNotifiesSubscribers() async {
        let h = Harness()
        await h.load()
        var got: [Int] = []
        h.store.onCurrentFileChange["t"] = { got.append($0) }
        h.store.reportTopRow(h.store.layout!.fileStartRow[2] + 1)
        h.store.reportTopRow(h.store.layout!.fileStartRow[2] + 2) // same file: no callback
        h.store.reportTopRow(-1)
        h.store.reportTopRow(10_000_000)
        #expect(got == [2])
        #expect(h.store.currentFile == 2)
    }
}

@MainActor
@Suite struct PRStoreExpandTests {
    @Test func expandAboveAndBelowUsesFileContents() async {
        let h = Harness()
        await h.load()
        let s = h.store
        #expect(!s.canExpandAbove(file: 0, hunk: 0) || s.files[0].hunks[0].newStart > 1)
        #expect(s.canExpandAbove(file: 0, hunk: 1))
        #expect(s.canExpandBelow(file: 0))
        #expect(!s.canExpandAbove(file: 99, hunk: 0))
        s.expandAbove(file: 0, hunk: 1)
        #expect(await eventually { s.layout!.rows.contains { $0.kind == .expanded } })
        #expect(h.demo.calls.contains("fileContents"))
        let expanded = s.layout!.rows.filter { $0.kind == .expanded }
        #expect(expanded.count == 20)
        // Lines from fetched content are available to lines(for:).
        let n = Int(expanded.first!.a)
        #expect(s.lines(for: CodeAnchor(path: s.files[0].path, side: .right, startLine: n, endLine: n)).first?.contains("line \(n)") == true)
        // Demo content has 400 lines; expanding below is capped by file length.
        s.expandBelow(file: 0)
        #expect(await eventually { s.options.expandedBelow[0] == 20 })
        #expect(s.canExpandBelow(file: 0))
    }

    @Test func expandUsesWorktreeWhenReady() async {
        let h = Harness()
        await h.load()
        try! await h.store.ensureWorktree()
        h.store.expandAbove(file: 1, hunk: 1)
        #expect(await eventually { h.worktrees.calls.contains { $0.hasPrefix("fileContents:") } })
    }
}

@MainActor
@Suite struct PRStorePersistenceTests {
    @Test func discussThreadRoundTrip() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("rb-rt-\(UUID().uuidString)")
        let h = Harness(root: root)
        await h.load()
        let id = h.store.startDiscuss(CodeAnchor(path: h.store.files[0].path, side: .right, startLine: 13, endLine: 13))!
        try? await Task.sleep(for: .milliseconds(50))
        await h.services.store.flush()
        let h2 = Harness(root: root)
        await h2.load()
        #expect(h2.store.discussThreads[id] != nil)
        #expect(h2.store.discussPaths == [h.store.files[0].path])
        #expect(h2.store.layout!.blocks.contains { $0.id == id && $0.kind == .discuss })
    }

    @Test func interruptedFixRequestsBecomeFailedOnReload() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("rb-rt-\(UUID().uuidString)")
        let store = LocalStore(root: root)
        var state = PRLocalState()
        state.fixRequests = [FixRequest(threadId: "x", anchor: CodeAnchor(path: "a", side: .right, startLine: 1, endLine: 1),
                                        instructions: "", status: .running)]
        await store.save(Harness.ref, state, debounce: .zero)
        await store.flush()
        let h = Harness(root: root)
        await h.load()
        #expect(h.store.fixRequests.first?.status == .failed)
        #expect(h.store.fixRequests.first?.error == "Interrupted")
    }
}

@MainActor
@Suite struct PRStoreGitTests {
    @Test func pushSuccessAndFailure() async {
        let h = Harness()
        await h.load()
        let s = h.store
        #expect(await s.push() == false) // no worktree yet
        try! await s.ensureWorktree()
        h.worktrees.localCommitsValue = [CommitInfo(sha: "c1", subject: "fix", author: "me", date: nil)]
        await s.refreshGitState()
        #expect(s.localCommits.count == 1)
        #expect(s.canPush && s.pushDisabledReason == nil)
        #expect(await s.push())
        #expect(s.pushState == .idle)
        #expect(s.localCommits.isEmpty)
        #expect(s.banner?.contains("Pushed 1 commit") == true)
        h.worktrees.pushError = TestError(message: "rejected")
        #expect(await s.push() == false)
        if case .failed(let e) = s.pushState { #expect(e.contains("rejected")) } else { Issue.record("expected failure") }
    }

    @Test func pushDisabledReasonForFork() async {
        let h = Harness()
        await h.load()
        var d = h.store.detail!
        d.headRepoNameWithOwner = "someone/fork"
        d.maintainerCanModify = false
        d.viewerLogin = "me"
        h.store.setDetailForTesting(d)
        #expect(!h.store.canPush)
        #expect(h.store.pushDisabledReason?.contains("fork") == true)
        d.headRepoNameWithOwner = Harness.ref.nameWithOwner
        d.viewerCanPush = false
        h.store.setDetailForTesting(d)
        #expect(h.store.pushDisabledReason?.contains("push access") == true)
    }

    @Test func ensureWorktreeFailureAndMissingGit() async {
        let h = Harness()
        await h.load()
        h.worktrees.ensureError = TestError(message: "clone failed")
        await #expect(throws: TestError.self) { try await h.store.ensureWorktree() }
        if case .failed(let e) = h.store.worktreeState { #expect(e == "clone failed") } else { Issue.record("expected failed") }
        let h2 = Harness(worktrees: false)
        await h2.load()
        await #expect(throws: WorktreeError.self) { try await h2.store.ensureWorktree() }
    }

    @Test func ensureWorktreeIsSharedAcrossConcurrentCallers() async {
        let h = Harness()
        await h.load()
        async let a = h.store.ensureWorktree()
        async let b = h.store.ensureWorktree()
        _ = try! await (a, b)
        #expect(h.worktrees.calls.filter { $0 == "ensureWorktree" }.count == 1)
        await h.store.rebaseOntoRemote()
        #expect(h.worktrees.calls.contains("rebase"))
    }
}
