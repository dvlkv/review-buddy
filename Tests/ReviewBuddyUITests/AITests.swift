import Foundation
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

@MainActor
@Suite struct DiscussStoreTests {
    @Test func startDiscussAnchorsAndActivates() async {
        let h = Harness()
        await h.load()
        let s = h.store
        s.select(file: 1, side: .right, line: 12, extend: false)
        s.select(file: 1, side: .right, line: 13, extend: true)
        let id = s.startDiscuss()!
        let t = s.discussThreads[id]!
        #expect(t.anchor == CodeAnchor(path: s.files[1].path, side: .right, startLine: 12, endLine: 13))
        #expect(t.commitSHA == s.detail!.headOid)
        #expect(t.anchorTextHash == DiscussThread.hashLines(s.lines(for: t.anchor)))
        #expect(s.activeDiscussId == id)
        #expect(s.selection == nil)
        #expect(s.layout!.blocks.contains { $0.id == id && $0.kind == .discuss })
        #expect(s.discussPaths.contains(s.files[1].path))
        // Left-side anchors use the base commit.
        let left = s.startDiscuss(CodeAnchor(path: s.files[1].path, side: .left, startLine: 13, endLine: 13))!
        #expect(s.discussThreads[left]!.commitSHA == s.detail!.baseOid)
        #expect(s.sortedDiscussThreads.first?.id == left)
        // Worktree is warmed in the background.
        #expect(await eventually { h.worktrees.calls.contains("ensureWorktree") })
    }

    @Test func startDiscussWithoutAnchorReturnsNil() async {
        let h = Harness()
        await h.load()
        #expect(h.store.startDiscuss() == nil)
    }

    @Test func deleteDiscussRemovesEverything() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let id = s.startDiscuss(CodeAnchor(path: s.files[0].path, side: .right, startLine: 13, endLine: 13))!
        _ = s.controller(for: id)
        s.deleteDiscuss(id)
        #expect(s.discussThreads[id] == nil)
        #expect(s.controllers[id] == nil)
        #expect(s.activeDiscussId == nil)
        #expect(s.discussPaths.isEmpty)
        #expect(!s.layout!.blocks.contains { $0.id == id })
    }

    func seedThread(_ h: Harness, line: Int, text: [String]) -> DiscussThread {
        let s = h.store
        let a = CodeAnchor(path: s.files[0].path, side: .right, startLine: line, endLine: line)
        let t = DiscussThread(ref: Harness.ref, anchor: a, commitSHA: "oldsha", anchorTextHash: DiscussThread.hashLines(text))
        s.updateDiscussThread(t)
        return t
    }

    @Test func remapSameTextUpdatesCommit() async {
        let h = Harness()
        await h.load()
        let s = h.store
        let t = seedThread(h, line: 13, text: s.lines(for: CodeAnchor(path: s.files[0].path, side: .right, startLine: 13, endLine: 13)))
        s.remapDiscussThreads()
        #expect(s.discussThreads[t.id]!.commitSHA == s.detail!.headOid)
        #expect(s.discussThreads[t.id]!.status == .active)
    }

    @Test func remapMovesAnchorWithWorktreeDiff() async {
        let h = Harness()
        await h.load()
        let s = h.store
        try! await s.ensureWorktree()
        // Two lines inserted after line 5 → old line 13 is now 15.
        h.worktrees.pathDiff = "--- a/x\n+++ b/x\n@@ -5,0 +6,2 @@\n+a\n+b\n"
        let t = seedThread(h, line: 13, text: ["something else"])
        s.remapDiscussThreads()
        #expect(await eventually { s.discussThreads[t.id]!.commitSHA == s.detail!.headOid })
        #expect(s.discussThreads[t.id]!.anchor.startLine == 15)
        #expect(h.worktrees.calls.contains("diffPath:\(s.files[0].path)"))
    }

    @Test func remapMarksOutdatedWhenLineChanged() async {
        let h = Harness()
        await h.load()
        let s = h.store
        try! await s.ensureWorktree()
        h.worktrees.pathDiff = "--- a/x\n+++ b/x\n@@ -13 +13 @@\n-old\n+new\n"
        let t = seedThread(h, line: 13, text: ["something else"])
        s.remapDiscussThreads()
        #expect(await eventually { s.discussThreads[t.id]!.status == .outdated })
        // Outdated threads render at the end of their file.
        let bi = s.layout!.blocks.firstIndex { $0.id == t.id }!
        #expect(s.layout!.blocks[bi].line == -1)
    }

    @Test func remapWithoutWorktreeLeavesThread() async {
        let h = Harness()
        await h.load()
        let t = seedThread(h, line: 13, text: ["different"])
        h.store.remapDiscussThreads()
        #expect(h.store.discussThreads[t.id]!.commitSHA == "oldsha")
    }
}

@MainActor
@Suite struct SummaryAndPreReviewTests {
    @Test func summarizeStoresSummary() async {
        let h = Harness()
        await h.load()
        h.oneShot.structured = Data(#"{"summary":"Does things.","readingOrder":[{"path":"a","reason":"core"}],"risks":[]}"#.utf8)
        await h.store.summarize()
        #expect(h.store.summaryState == .idle)
        #expect(h.store.summary?.summary == "Does things.")
        #expect(h.store.summary?.headSHA == h.store.detail!.headOid)
        let cfg = h.oneShot.configs.last!
        #expect(cfg.jsonSchema == Prompts.summarySchema)
        #expect(cfg.permissionMode == "plan")
        #expect(cfg.cwd == h.worktrees.dir)
        #expect(h.oneShot.prompts.last!.contains(Prompts.diffRange(h.store.detail!)))
    }

    @Test func summarizeFallsBackToTextAndFailures() async {
        let h = Harness()
        await h.load()
        h.oneShot.text = #"{"summary":"T","readingOrder":[],"risks":[]}"#
        await h.store.summarize()
        #expect(h.store.summary?.summary == "T")
        h.oneShot.error = TestError(message: "rate limited")
        await h.store.summarize()
        #expect(h.store.summaryState == .failed("rate limited"))
        #expect(h.store.summary?.summary == "T") // previous summary kept
        h.oneShot.error = nil
        h.oneShot.text = "not json"
        await h.store.summarize()
        if case .failed = h.store.summaryState {} else { Issue.record("expected decode failure") }
    }

    @Test func missingClaudeFails() async {
        let h = Harness(claudePath: nil)
        await h.load()
        await h.store.summarize()
        await h.store.preReview()
        if case .failed = h.store.summaryState {} else { Issue.record("summary should fail") }
        if case .failed = h.store.preReviewState {} else { Issue.record("pre-review should fail") }
        #expect(h.oneShot.configs.isEmpty)
    }

    func findingsJSON(_ s: PRStore) -> Data {
        let p0 = s.files[0].path, p1 = s.files[1].path
        return Data("""
        {"findings":[
          {"path":"\(p0)","side":"RIGHT","startLine":13,"endLine":13,"severity":"low","title":"Low","body":"b"},
          {"path":"\(p1)","side":"RIGHT","startLine":14,"endLine":13,"severity":"high","title":"High","body":"b","suggestion":"fixed()"},
          {"path":"not/in/diff.swift","side":"RIGHT","startLine":1,"endLine":1,"severity":"high","title":"Ghost","body":"b"},
          {"path":"\(p0)","side":"LEFT","startLine":14,"endLine":14,"severity":"nit","title":"Nit","body":"b","suggestion":""}
        ]}
        """.utf8)
    }

    @Test func preReviewFiltersAndSorts() async {
        let h = Harness()
        await h.load()
        let s = h.store
        h.oneShot.structured = findingsJSON(s)
        await s.preReview()
        #expect(s.preReviewState == .idle)
        #expect(s.openFindings.map(\.title) == ["High", "Low", "Nit"])
        let high = s.openFindings[0]
        #expect(high.startLine == 13 && high.endLine == 14) // swapped range normalized
        #expect(s.openFindings[2].suggestion == nil) // empty suggestion dropped
        #expect(h.oneShot.configs.last!.jsonSchema == Prompts.preReviewSchema)
        #expect(s.layout!.blocks.filter { $0.kind == .finding }.count == 3)
    }

    @Test func findingActions() async {
        let h = Harness()
        await h.load()
        let s = h.store
        h.oneShot.structured = findingsJSON(s)
        await s.preReview()
        let high = s.openFindings.first { $0.title == "High" }!
        s.acceptFinding(high.id)
        #expect(s.findings[high.id]!.state == .accepted)
        let composer = s.composers.values.first!
        #expect(composer.text == high.commentBody)
        #expect(composer.text.contains("```suggestion\nfixed()\n```"))
        #expect(composer.origin == "From AI pre-review")
        let low = s.openFindings.first { $0.title == "Low" }!
        s.dismissFinding(low.id)
        #expect(s.findings[low.id]!.state == .dismissed)
        #expect(!s.layout!.blocks.contains { $0.id == low.id })
        let nit = s.openFindings.first { $0.title == "Nit" }!
        s.discussFinding(nit.id)
        #expect(s.findings[nit.id]!.state == .discussed)
        let id = s.activeDiscussId!
        #expect(s.controller(for: id).input.contains("Nit"))
        #expect(s.discussThreads[id]!.anchor.side == .left)
        // Re-running keeps non-open findings and replaces open ones.
        await s.preReview()
        #expect(s.findings.values.filter { $0.state != .open }.count == 3)
        #expect(s.openFindings.count == 3)
    }

    @Test func preReviewFailure() async {
        let h = Harness()
        await h.load()
        h.oneShot.error = TestError(message: "nope")
        await h.store.preReview()
        #expect(h.store.preReviewState == .failed("nope"))
    }

    @Test func autoRunAIWhenEnabled() async {
        let h = Harness()
        h.services.settings.autoSummarize = true
        h.services.settings.autoPreReviewMaxLines = 100_000
        h.oneShot.structured = Data(#"{"summary":"auto","readingOrder":[],"risks":[],"findings":[]}"#.utf8)
        await h.load()
        #expect(await eventually { h.oneShot.configs.count == 2 })
        #expect(await eventually { h.store.summary?.summary == "auto" })
    }
}

@MainActor
@Suite struct DiscussControllerTests {
    func setup() async -> (Harness, DiscussController) {
        let h = Harness()
        await h.load()
        let s = h.store
        let id = s.startDiscuss(CodeAnchor(path: s.files[0].path, side: .right, startLine: 13, endLine: 13))!
        return (h, s.controller(for: id))
    }

    @Test func firstMessageWrapsContextThenRaw() async {
        let (h, c) = await setup()
        // Surrounding code is read from the worktree.
        let path = h.worktrees.dir.appendingPathComponent(h.store.files[0].path)
        try? FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? (1...40).map { "src \($0)" }.joined(separator: "\n").write(to: path, atomically: true, encoding: .utf8)
        await c.send("Why is this here?")
        let session = h.sessions.sessions.first!
        #expect(session.sent.count == 1)
        let first = session.sent[0]
        #expect(first.contains("<review_context>"))
        #expect(first.contains("Why is this here?"))
        #expect(first.contains(h.store.files[0].path))
        #expect(first.contains("13\tsrc 13"))
        #expect(session.turnActive)
        #expect(session.config.appendSystemPrompt?.contains("Review Buddy") == true)
        #expect(session.config.cwd == h.worktrees.dir)
        #expect(c.isRunning)

        session.reply("Because reasons.")
        #expect(await eventually { !c.isRunning })
        #expect(c.messages.map(\.role) == [.user, .assistant])
        #expect(c.messages.last!.text == "Because reasons.")
        #expect(c.reducer.sessionId == "sess-1")
        #expect(!session.turnActive)
        // Persisted on the thread.
        #expect(h.store.discussThreads[c.threadId]!.sessionId == "sess-1")
        #expect(h.store.discussThreads[c.threadId]!.messages.count == 2)

        await c.send("And then?")
        #expect(session.sent.count == 2)
        #expect(session.sent[1] == "And then?")
        #expect(h.sessions.sessions.count == 1) // same process reused
        await c.send("   ")
        #expect(session.sent.count == 2)
    }

    @Test func streamingEventsAndToolCalls() async {
        let (h, c) = await setup()
        await c.send("q")
        let s = h.sessions.sessions[0]
        s.continuation.yield(.event(.textDelta(messageId: nil, text: "Hel")))
        s.continuation.yield(.event(.textDelta(messageId: nil, text: "lo")))
        #expect(await eventually { c.messages.last?.text == "Hello" })
        #expect(c.messages.last!.isStreaming)
        s.continuation.yield(.event(.assistant(messageId: "m", blocks: [.text("Hello!"), .toolUse(id: "t1", name: "Read", input: #"{"file_path":"/a/b.go"}"#)])))
        s.continuation.yield(.event(.toolResults([.init(toolUseId: "t1", content: "file", isError: false)])))
        #expect(await eventually { c.messages.contains { $0.role == .tool && $0.toolResult == "file" } })
        #expect(c.messages.first { $0.role == .assistant }!.text == "Hello!")
    }

    @Test func processExitShowsError() async {
        let (h, c) = await setup()
        await c.send("q")
        let s = h.sessions.sessions[0]
        s.continuation.yield(.exited(status: 1, stderr: "auth expired"))
        #expect(await eventually { c.messages.last?.role == .error })
        #expect(c.messages.last!.text == "auth expired")
        #expect(!c.isRunning)
        // Next send starts a new session (resume with the old id when known).
        await c.send("again")
        #expect(h.sessions.sessions.count == 2)
    }

    @Test func sendFailsWhenWorktreeFails() async {
        let (h, c) = await setup()
        h.worktrees.ensureError = TestError(message: "no git")
        // Reset the warmed worktree state by creating a fresh store.
        let h2 = Harness()
        h2.worktrees.ensureError = TestError(message: "no git")
        await h2.load()
        let id = h2.store.startDiscuss(CodeAnchor(path: h2.store.files[0].path, side: .right, startLine: 13, endLine: 13))!
        let c2 = h2.store.controller(for: id)
        await c2.send("q")
        #expect(c2.messages.last?.role == .error)
        #expect(c2.messages.last?.text == "no git")
        #expect(!c2.isRunning)
        _ = c
    }

    @Test func stopInterrupts() async {
        let (h, c) = await setup()
        await c.send("q")
        c.stop()
        #expect(h.sessions.sessions[0].interrupts == 1)
        #expect(c.messages.last?.role == .system)
        #expect(!c.reducer.isRunning)
    }

    @Test func fixItCommitsNewCommits() async {
        let (h, c) = await setup()
        await c.send("q")
        let s = h.sessions.sessions[0]
        s.reply("Found it.")
        #expect(await eventually { !c.isRunning })
        h.worktrees.head = "base"
        h.worktrees.localCommitsValue = [CommitInfo(sha: "old1", subject: "earlier", author: "a", date: nil)]
        await h.store.refreshGitState()
        h.worktrees.head = "base"
        await c.fixIt()
        #expect(s.sent.last == Prompts.fixInstruction)
        #expect(h.store.fixStatus(threadId: c.threadId)?.status == .running)
        #expect(c.isFixing)
        // The agent commits: HEAD moves and a new commit appears on top.
        h.worktrees.head = "new1"
        h.worktrees.localCommitsValue = [CommitInfo(sha: "new1", subject: "fix", author: "c", date: nil),
                                         CommitInfo(sha: "base", subject: "earlier", author: "a", date: nil)]
        s.reply("Committed.")
        #expect(await eventually { h.store.fixStatus(threadId: c.threadId)?.status == .committed })
        #expect(h.store.fixStatus(threadId: c.threadId)?.commits == ["new1"])
        #expect(h.store.discussThreads[c.threadId]!.fixCommits == ["new1"])
        #expect(h.store.localCommits.count == 2)
        #expect(!c.isFixing)
    }

    @Test func fixItWithoutCommitFails() async {
        let (h, c) = await setup()
        await c.send("q")
        let s = h.sessions.sessions[0]
        s.reply("ok")
        #expect(await eventually { !c.isRunning })
        await c.fixIt()
        s.reply("I could not fix it.")
        #expect(await eventually { h.store.fixStatus(threadId: c.threadId)?.status == .failed })
        #expect(h.store.fixStatus(threadId: c.threadId)?.error == "No commit was created.")
    }

    @Test func secondFixIsQueuedUntilFirstFinishes() async {
        let (h, c1) = await setup()
        let s = h.store
        let id2 = s.startDiscuss(CodeAnchor(path: s.files[1].path, side: .right, startLine: 13, endLine: 13))!
        let c2 = s.controller(for: id2)
        await c1.send("a"); h.sessions.sessions[0].reply("A")
        await c2.send("b"); h.sessions.sessions[1].reply("B")
        #expect(await eventually { !c1.isRunning && !c2.isRunning })
        await c1.fixIt()
        await c2.fixIt()
        #expect(s.fixStatus(threadId: c1.threadId)?.status == .running)
        #expect(s.fixStatus(threadId: id2)?.status == .queued)
        #expect(h.sessions.sessions[1].sent.last != Prompts.fixInstruction)
        h.worktrees.head = "x1"
        h.worktrees.localCommitsValue = [CommitInfo(sha: "x1", subject: "fix", author: "c", date: nil)]
        h.sessions.sessions[0].reply("done")
        #expect(await eventually { h.sessions.sessions[1].sent.last == Prompts.fixInstruction })
        #expect(s.fixStatus(threadId: id2)?.status == .running)
    }

    @Test func convertToCommentForksSession() async {
        let (h, c) = await setup()
        await c.send("q")
        h.sessions.sessions[0].reply("answer", sessionId: "S42")
        #expect(await eventually { !c.isRunning })
        h.oneShot.text = "```markdown\nPlease add a test.\n```"
        await c.convertToComment()
        let cfg = h.oneShot.configs.last!
        #expect(cfg.resumeSessionId == "S42")
        #expect(cfg.forkSession)
        #expect(cfg.permissionMode == "plan")
        #expect(h.oneShot.prompts.last == Prompts.draftComment(anchor: c.thread!.anchor))
        #expect(c.draftState == .idle)
        let composer = h.store.composers.values.first!
        #expect(composer.text == "Please add a test.")
        #expect(composer.origin == "Drafted from Discuss")
        #expect(composer.anchor == c.thread!.anchor)
    }

    @Test func convertWithoutSessionIncludesTranscriptAndFailure() async {
        let (h, c) = await setup()
        h.oneShot.text = "Draft"
        await c.convertToComment()
        #expect(h.oneShot.configs.last!.forkSession == false)
        #expect(h.oneShot.configs.last!.resumeSessionId == nil)
        h.oneShot.error = TestError(message: "fail")
        await c.convertToComment()
        #expect(c.draftState == .failed("fail"))
    }

    @Test func cleanDraft() {
        #expect(DiscussController.cleanDraft("```markdown\nHi\n```") == "Hi")
        #expect(DiscussController.cleanDraft("```md\nHi there\n```\n") == "Hi there")
        #expect(DiscussController.cleanDraft("  plain  ") == "plain")
        #expect(DiscussController.cleanDraft("```suggestion\nx\n```") == "```suggestion\nx\n```")
    }

    @Test func missingClaudeReportsNotInstalled() async {
        let h = Harness(claudePath: nil)
        await h.load()
        let id = h.store.startDiscuss(CodeAnchor(path: h.store.files[0].path, side: .right, startLine: 13, endLine: 13))!
        let c = h.store.controller(for: id)
        await c.send("q")
        #expect(c.messages.last?.role == .error)
        #expect(c.messages.last?.text == ClaudeError.notInstalled.localizedDescription)
    }

    @Test func shutdownTerminatesSession() async {
        let (h, c) = await setup()
        await c.send("q")
        h.store.deleteDiscuss(c.threadId)
        #expect(h.sessions.sessions[0].terminated)
    }
}
