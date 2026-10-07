import Foundation
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

@MainActor
@Suite struct SpecGapTests {
    @Test func expandFooterRowsAreShown() async {
        let h = Harness()
        await h.load()
        let footers = h.store.layout!.rows.filter { $0.kind == .expandFooter }
        #expect(footers.count == h.store.files.count)
        #expect(h.store.canExpandBelow(file: 0))
    }

    @Test func expandBelowAddsContextAfterLastHunk() async {
        let h = Harness()
        await h.load()
        let before = h.store.layout!.rows.filter { $0.kind == .expanded }.count
        h.store.expandBelow(file: 0)
        #expect(await eventually { h.store.layout!.rows.filter { $0.kind == .expanded }.count > before })
        let last = h.store.files[0].hunks.last!
        let firstExpanded = h.store.layout!.rows.first { $0.kind == .expanded }!
        #expect(Int(firstExpanded.a) == last.newStart + last.newCount)
    }

    @Test func replyKeyTargetsThreadReachedWithNavigation() async {
        let h = Harness(threadsEvery: 1)
        await h.load()
        h.store.navigate(.nextThread)
        let focused = h.store.focusedThreadId
        #expect(focused != nil)
        h.store.replyToFocusedThread()
        #expect(h.store.composerFor(reply: focused!) != nil)
    }

    @Test func replyKeyFallsBackToFirstVisibleThread() async {
        let h = Harness(threadsEvery: 1)
        await h.load()
        #expect(h.store.focusedThreadId == nil)
        h.store.replyToFocusedThread()
        #expect(h.store.composers.values.contains { if case .reply = $0.kind { return true }; return false })
    }

    @Test func findingsBecomeOutdatedWhenHeadMoves() async {
        let h = Harness()
        await h.load()
        h.store.setFindings([AIFinding(path: h.store.files[0].path, side: .right, startLine: h.store.firstAddition(file: 0),
                                       endLine: h.store.firstAddition(file: 0), severity: .low, title: "t", body: "b")])
        #expect(!h.store.findingsAreOutdated)
        var d = h.store.detail!
        d.headOid = String(repeating: "c", count: 40)
        h.store.setDetailForTesting(d)
        #expect(h.store.findingsAreOutdated)
    }

    @Test func findingsHeadSurvivesPersistence() async {
        let h = Harness()
        await h.load()
        h.store.setFindings([AIFinding(path: h.store.files[0].path, side: .right, startLine: 1, endLine: 1,
                                       severity: .low, title: "t", body: "b")])
        let original = h.store.detail!.headOid
        var d = h.store.detail!
        d.headOid = String(repeating: "d", count: 40)
        h.store.setDetailForTesting(d)
        h.store.persist()
        await h.services.store.flush()
        let reloaded = await h.services.store.load(Harness.ref)
        #expect(reloaded.preReview?.headSHA == original)
    }

    @Test func mergedPRWorktreeIsRemovedWhenClean() async {
        let h = Harness()
        await h.load()
        h.worktrees.hasExistingWorktree = true
        var d = h.store.detail!
        d.state = "MERGED"
        h.store.setDetailForTesting(d)
        await h.store.cleanUpWorktreeIfClosed()
        #expect(h.worktrees.calls.contains("remove"))
    }

    @Test func mergedPRWorktreeKeptWithUnpushedCommits() async {
        let h = Harness()
        await h.load()
        h.worktrees.hasExistingWorktree = true
        h.worktrees.localCommitsValue = [CommitInfo(sha: "abc1234", subject: "fix", author: "me", date: nil)]
        var d = h.store.detail!
        d.state = "CLOSED"
        h.store.setDetailForTesting(d)
        await h.store.cleanUpWorktreeIfClosed()
        #expect(!h.worktrees.calls.contains("remove"))
    }

    @Test func closedPRWorktreeNotRemovedWhileInUse() async {
        let h = Harness()
        await h.load()
        h.worktrees.hasExistingWorktree = true
        _ = try? await h.store.ensureWorktree()   // e.g. Claude is summarizing in it
        var d = h.store.detail!
        d.state = "MERGED"
        h.store.setDetailForTesting(d)
        await h.store.cleanUpWorktreeIfClosed()
        #expect(!h.worktrees.calls.contains("remove"))
    }

    @Test func openPRWorktreeNotTouched() async {
        let h = Harness()
        await h.load()
        h.worktrees.hasExistingWorktree = true
        await h.store.cleanUpWorktreeIfClosed()
        #expect(!h.worktrees.calls.contains("ensureExisting"))
    }

    @Test func replyIsBoundToR() {
        #expect(KeyBindings.action(for: KeyStroke("r"), textInputFocused: false) == .reply)
    }
}
