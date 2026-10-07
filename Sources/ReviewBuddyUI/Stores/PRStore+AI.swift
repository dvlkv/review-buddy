import Foundation
import ReviewBuddyCore

extension PRStore {
    // MARK: Discuss

    /// Creates a Discuss thread on the selection (or anchor) and opens it in the panel.
    @discardableResult
    public func startDiscuss(_ anchor: CodeAnchor? = nil, seed: String? = nil) -> String? {
        guard let anchor = anchor ?? selection?.anchor, let detail else { return nil }
        let lines = lines(for: anchor)
        let t = DiscussThread(ref: ref, anchor: anchor, commitSHA: anchor.side == .right ? detail.headOid : detail.baseOid,
                              anchorTextHash: DiscussThread.hashLines(lines))
        updateDiscussThread(t)
        selection = nil
        activeDiscussId = t.id
        rebuildLayout()
        if let seed { controller(for: t.id).input = seed }
        // Warm the checkout while the user types their question.
        Task { try? await ensureWorktree() }
        return t.id
    }

    public func controller(for id: String) -> DiscussController {
        if let c = controllers[id] { return c }
        let c = DiscussController(thread: discussThreads[id]!, pr: self)
        setControllers(id, c)
        return c
    }

    public func deleteDiscuss(_ id: String) {
        controllers[id]?.shutdown()
        setControllers(id, nil)
        removeDiscuss(id)
        if activeDiscussId == id { activeDiscussId = nil }
        persist()
        rebuildLayout()
    }

    public var sortedDiscussThreads: [DiscussThread] {
        discussThreads.values.sorted { $0.createdAt > $1.createdAt }
    }

    /// Moves Discuss anchors to the current head; marks them outdated if their lines changed.
    func remapDiscussThreads() {
        guard let detail, let diff else { return }
        for t in discussThreads.values where t.anchor.side == .right && t.commitSHA != detail.headOid && t.status == .active {
            // Fast path: the anchored text is still at the same lines.
            if DiscussThread.hashLines(lines(for: t.anchor)) == t.anchorTextHash, !lines(for: t.anchor).isEmpty {
                var u = t; u.commitSHA = detail.headOid; updateDiscussThread(u)
                continue
            }
            guard let wt = worktreeState.worktree, let w = services.worktrees else { continue }
            Task {
                let data = try? await w.diff(repoDir: wt.repoDir, from: t.commitSHA, to: detail.headOid, path: t.anchor.path)
                let fd = data.map { UnifiedDiffParser.parse(data: $0) }?.files.first
                var u = t
                if let range = LineMapper(file: fd).map(range: t.anchor.startLine...t.anchor.endLine) {
                    u.anchor = CodeAnchor(path: t.anchor.path, side: .right, startLine: range.lowerBound, endLine: range.upperBound)
                    u.commitSHA = detail.headOid
                } else {
                    u.status = .outdated
                }
                updateDiscussThread(u)
                rebuildLayout()
            }
        }
        _ = diff
        rebuildLayout()
    }

    // MARK: Fix queue (one fix at a time per PR, since threads share the worktree)

    func isFixRunning(except threadId: String) -> Bool {
        fixRequests.contains { $0.status == .running && $0.threadId != threadId }
    }

    @discardableResult
    func enqueueFix(threadId: String, anchor: CodeAnchor, start: Bool = false) -> String {
        let r = FixRequest(threadId: threadId, anchor: anchor, instructions: Prompts.fixInstruction,
                           status: start ? .running : .queued)
        setFixRequests(fixRequests + [r])
        persist()
        return r.id
    }

    func markFix(_ id: String, _ status: FixRequest.Status, commits: [String] = [], error: String? = nil) {
        var list = fixRequests
        guard let i = list.firstIndex(where: { $0.id == id }) else { return }
        list[i].status = status
        if !commits.isEmpty { list[i].commits = commits }
        list[i].error = error
        setFixRequests(list)
        persist()
    }

    func runNextQueuedFix() {
        guard !fixRequests.contains(where: { $0.status == .running }),
              let next = fixRequests.first(where: { $0.status == .queued }) else { return }
        let c = controller(for: next.threadId)
        Task { await c.runFix(requestId: next.id) }
    }

    public func fixStatus(threadId: String) -> FixRequest? {
        fixRequests.last { $0.threadId == threadId }
    }

    // MARK: Summary & pre-review

    func autoRunAI() {
        if services.settings.autoSummarize, summary == nil || summary?.headSHA != detail?.headOid {
            Task { await summarize() }
        }
        let max = services.settings.autoPreReviewMaxLines
        if max > 0, let d = detail, d.additions + d.deletions <= max, findings.isEmpty {
            Task { await preReview() }
        }
    }

    public func summarize() async {
        guard let detail, let claude = services.claudePath else {
            setSummary(summary, state: .failed(ClaudeError.notInstalled.localizedDescription)); return
        }
        setSummary(summary, state: .running("Preparing checkout…"))
        do {
            let wt = try await ensureWorktree()
            setSummary(summary, state: .running("Claude is reading the diff…"))
            let cfg = ClaudeConfig(executable: claude, cwd: wt.path, appendSystemPrompt: Prompts.systemPrompt(pr: detail),
                                   model: services.settings.claudeModel.isEmpty ? nil : services.settings.claudeModel,
                                   permissionMode: "plan", jsonSchema: Prompts.summarySchema)
            let res = try await services.oneShot.run(config: cfg, prompt: Prompts.summary(pr: detail))
            guard let data = res.structuredOutput ?? res.text?.data(using: .utf8) else { throw ClaudeError.badOutput }
            var s = try JSONDecoder().decode(AISummary.self, from: data)
            s.headSHA = detail.headOid
            s.createdAt = Date()
            setSummary(s, state: .idle)
            persist()
        } catch {
            setSummary(summary, state: .failed(error.localizedDescription))
        }
    }

    public func preReview() async {
        guard let detail, let claude = services.claudePath else {
            setPreReviewState(.failed(ClaudeError.notInstalled.localizedDescription)); return
        }
        setPreReviewState(.running("Preparing checkout…"))
        do {
            let wt = try await ensureWorktree()
            setPreReviewState(.running("Claude is reviewing…"))
            let cfg = ClaudeConfig(executable: claude, cwd: wt.path, appendSystemPrompt: Prompts.systemPrompt(pr: detail),
                                   model: services.settings.claudeModel.isEmpty ? nil : services.settings.claudeModel,
                                   permissionMode: "plan", jsonSchema: Prompts.preReviewSchema)
            let res = try await services.oneShot.run(config: cfg, prompt: Prompts.preReview(pr: detail))
            guard let data = res.structuredOutput ?? res.text?.data(using: .utf8) else { throw ClaudeError.badOutput }
            let result = try JSONDecoder().decode(PreReviewResult.self, from: data)
            // Keep only findings that land on files in this diff.
            let valid = result.findings.filter { diff?.fileIndex(path: $0.path) != nil }
            let kept = findings.values.filter { $0.state != .open }
            setFindings(kept + valid.sorted { $0.severity < $1.severity })
            setPreReviewState(.idle)
            persist()
            rebuildLayout()
        } catch {
            setPreReviewState(.failed(error.localizedDescription))
        }
    }

    public var openFindings: [AIFinding] {
        findings.values.filter { $0.state == .open }.sorted { ($0.severity, $0.path, $0.startLine) < ($1.severity, $1.path, $1.startLine) }
    }

    public func acceptFinding(_ id: String) {
        guard var f = findings[id] else { return }
        f.state = .accepted
        updateFinding(f)
        persist()
        startComment(f.anchor, text: f.commentBody, origin: "From AI pre-review")
    }

    public func dismissFinding(_ id: String) {
        guard var f = findings[id] else { return }
        f.state = .dismissed
        updateFinding(f)
        persist()
        rebuildLayout()
    }

    public func discussFinding(_ id: String) {
        guard var f = findings[id] else { return }
        f.state = .discussed
        updateFinding(f)
        persist()
        startDiscuss(f.anchor, seed: "The AI pre-review flagged this: **\(f.title)** — \(f.body)\n\nIs this a real problem? ")
    }
}
