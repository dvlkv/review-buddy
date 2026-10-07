import Foundation
import Observation
import ReviewBuddyCore

/// Drives one Discuss thread: a Claude session in the PR worktree plus its transcript.
@MainActor
@Observable
public final class DiscussController {
    public let threadId: String
    @ObservationIgnored unowned let pr: PRStore
    public private(set) var reducer: TranscriptReducer
    public private(set) var status: String?
    public private(set) var draftState: TaskState = .idle
    public var input = ""
    @ObservationIgnored private var session: ClaudeSessionProtocol?
    @ObservationIgnored private var pump: Task<Void, Never>?
    /// Set while a "Fix it" turn is running; holds the HEAD before the fix.
    @ObservationIgnored private var fixBaseline: (request: String, head: String)?

    init(thread: DiscussThread, pr: PRStore) {
        self.threadId = thread.id
        self.pr = pr
        self.reducer = TranscriptReducer(messages: thread.messages, sessionId: thread.sessionId)
    }

    public var thread: DiscussThread? { pr.discussThreads[threadId] }
    public var messages: [DiscussMessage] { reducer.messages }
    public var isRunning: Bool { reducer.isRunning || status != nil }
    public var isFixing: Bool { fixBaseline != nil }

    // MARK: Sending

    public func send(_ raw: String) async {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let thread, let detail = pr.detail else { return }
        let first = reducer.messages.isEmpty && reducer.sessionId == nil
        reducer.userSent(text)
        save()
        do {
            status = "Preparing checkout…"
            let wt = try await pr.ensureWorktree()
            status = nil
            let prompt: String
            if first {
                prompt = Prompts.discussContext(
                    pr: detail, anchor: thread.anchor, selectedLines: pr.lines(for: thread.anchor),
                    surrounding: await surrounding(anchor: thread.anchor, wt: wt),
                    threadsOnFile: pr.threads.values.filter { $0.path == thread.anchor.path },
                    otherThreads: pr.threads.values.filter { $0.path != thread.anchor.path }, question: text)
            } else {
                prompt = text
            }
            try startSessionIfNeeded(wt: wt, detail: detail)
            session?.turnActive = true
            try session?.send(prompt)
        } catch {
            status = nil
            reducer.processExited(status: -1, stderr: error.localizedDescription)
            save()
        }
    }

    /// "Fix it": ask the agent to implement and commit what was discussed.
    public func fixIt() async {
        guard let thread, !isRunning else { return }
        if pr.isFixRunning(except: threadId) {
            pr.enqueueFix(threadId: threadId, anchor: thread.anchor)
            return
        }
        await runFix(requestId: pr.enqueueFix(threadId: threadId, anchor: thread.anchor, start: true))
    }

    func runFix(requestId: String) async {
        do {
            let wt = try await pr.ensureWorktree()
            let head = (try? await pr.services.worktrees?.headSHA(wt)) ?? ""
            fixBaseline = (requestId, head)
            pr.markFix(requestId, .running)
            await send(Prompts.fixInstruction)
        } catch {
            pr.markFix(requestId, .failed, error: error.localizedDescription)
        }
    }

    public func stop() {
        session?.interrupt()
        reducer.interrupted()
        save()
    }

    // MARK: Convert to comment

    /// Forks the session to draft a review comment, then opens the inline composer.
    public func convertToComment() async {
        guard let thread, let detail = pr.detail, let claude = pr.services.claudePath else { return }
        draftState = .running("Drafting comment…")
        do {
            let wt = try await pr.ensureWorktree()
            var cfg = ClaudeConfig(executable: claude, cwd: wt.path, resumeSessionId: reducer.sessionId,
                                   forkSession: reducer.sessionId != nil,
                                   appendSystemPrompt: Prompts.systemPrompt(pr: detail),
                                   model: pr.services.settings.claudeModel.isEmpty ? nil : pr.services.settings.claudeModel)
            cfg.permissionMode = "plan"
            var prompt = Prompts.draftComment(anchor: thread.anchor)
            if reducer.sessionId == nil {
                // No session yet: include the visible transcript.
                prompt = messages.filter { $0.role == .user || $0.role == .assistant }
                    .map { "\($0.role.rawValue): \($0.text)" }.joined(separator: "\n\n") + "\n\n" + prompt
            }
            let res = try await pr.services.oneShot.run(config: cfg, prompt: prompt)
            let body = Self.cleanDraft(res.text ?? "")
            draftState = .idle
            pr.startComment(thread.anchor, text: body, origin: "Drafted from Discuss")
            pr.scrollTo(anchor: thread.anchor)
        } catch {
            draftState = .failed(error.localizedDescription)
        }
    }

    static func cleanDraft(_ s: String) -> String {
        var t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        // Models sometimes wrap the whole answer in ```markdown fences.
        if t.hasPrefix("```markdown") || t.hasPrefix("```md") {
            t = t.components(separatedBy: "\n").dropFirst().joined(separator: "\n")
            if t.hasSuffix("```") { t = String(t.dropLast(3)) }
        }
        return t.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: Session plumbing

    private func startSessionIfNeeded(wt: Worktree, detail: PRDetail) throws {
        if session != nil { return }
        guard let claude = pr.services.claudePath else { throw ClaudeError.notInstalled }
        let cfg = ClaudeConfig(executable: claude, cwd: wt.path, resumeSessionId: reducer.sessionId,
                               appendSystemPrompt: Prompts.systemPrompt(pr: detail),
                               model: pr.services.settings.claudeModel.isEmpty ? nil : pr.services.settings.claudeModel)
        let s = pr.services.makeSession(cfg)
        session = s
        pump = Task { [weak self] in
            for await out in s.output {
                guard let self else { return }
                self.handle(out)
            }
        }
    }

    func handle(_ out: SessionOutput) {
        switch out {
        case .event(let ev):
            reducer.apply(ev)
            if case .result = ev {
                session?.turnActive = false
                save()
                if fixBaseline != nil { Task { await finishFix() } }
            } else if case .assistant = ev {
                save()
            }
        case .exited(let status, let stderr):
            reducer.processExited(status: status, stderr: stderr)
            session = nil
            pump = nil
            save()
            if fixBaseline != nil { Task { await finishFix() } }
        }
    }

    private func finishFix() async {
        guard let (requestId, before) = fixBaseline else { return }
        fixBaseline = nil
        await pr.refreshGitState()
        var produced: [String] = []
        if let wt = pr.worktreeState.worktree, let w = pr.services.worktrees,
           let after = try? await w.headSHA(wt), after != before {
            // localCommits is newest-first; keep the ones created after the baseline.
            produced = pr.localCommits.prefix { $0.sha != before }.map(\.sha)
        }
        if produced.isEmpty {
            pr.markFix(requestId, .failed, error: "No commit was created.")
        } else {
            pr.markFix(requestId, .committed, commits: produced)
            if var t = thread {
                t.fixCommits = Array(Set(t.fixCommits + produced))
                pr.updateDiscussThread(t)
            }
        }
        pr.runNextQueuedFix()
    }

    private func surrounding(anchor: CodeAnchor, wt: Worktree) async -> String? {
        let lines: [String]
        if anchor.side == .right {
            let url = wt.path.appendingPathComponent(anchor.path)
            guard let s = try? String(contentsOf: url, encoding: .utf8) else { return nil }
            lines = s.components(separatedBy: "\n")
        } else {
            guard let detail = pr.detail, let w = pr.services.worktrees,
                  let d = try? await w.fileContents(repoDir: wt.repoDir, sha: detail.baseOid, path: anchor.path) else { return nil }
            lines = String(decoding: d, as: UTF8.self).components(separatedBy: "\n")
        }
        let lo = max(1, anchor.startLine - 20), hi = min(lines.count, anchor.endLine + 20)
        guard lo <= hi else { return nil }
        return (lo...hi).map { "\($0)\t\(lines[$0 - 1])" }.joined(separator: "\n")
    }

    private func save() {
        guard var t = thread else { return }
        t.messages = reducer.messages
        t.sessionId = reducer.sessionId
        pr.updateDiscussThread(t)
    }

    func shutdown() {
        session?.terminate()
        pump?.cancel()
        session = nil
    }
}
