import AppKit
import Foundation
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

// MARK: - Fake git

final class FakeWorktrees: WorktreeProviding, @unchecked Sendable {
    private let lock = NSLock()
    let dir: URL
    private var _calls: [String] = []
    private var _localCommits: [CommitInfo] = []
    private var _head = "base-head"
    var pushError: Error?
    var ensureError: Error?
    /// Unified diff returned by diff(from:to:path:).
    var pathDiff: String = ""
    var fileContentsText = (1...300).map { "content line \($0)" }.joined(separator: "\n")

    init() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("rb-wt-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    var calls: [String] { lock.withLock { _calls } }
    var localCommitsValue: [CommitInfo] {
        get { lock.withLock { _localCommits } }
        set { lock.withLock { _localCommits = newValue } }
    }
    var head: String {
        get { lock.withLock { _head } }
        set { lock.withLock { _head = newValue } }
    }
    private func record(_ s: String) { lock.withLock { _calls.append(s) } }

    var worktree: Worktree {
        Worktree(path: dir, branch: "review-buddy/pr-1", repoDir: dir.appendingPathComponent(".git"),
                 remoteHeadRef: "refs/review-buddy/head-1")
    }

    func ensureWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree {
        record("ensureWorktree")
        if let ensureError { throw ensureError }
        return worktree
    }
    func fetchPR(repoDir: URL, pr: PRDetail) async throws { record("fetchPR") }
    func localCommits(_ wt: Worktree) async throws -> [CommitInfo] { record("localCommits"); return localCommitsValue }
    func remoteAhead(_ wt: Worktree) async throws -> Int { record("remoteAhead"); return 0 }
    func rebaseOntoRemote(_ wt: Worktree) async throws { record("rebase") }
    func push(_ wt: Worktree, pr: PRDetail) async throws {
        record("push")
        if let pushError { throw pushError }
        localCommitsValue = []
    }
    func diff(repoDir: URL, base: String, head: String) async throws -> Data { record("diff"); return Data() }
    func diff(repoDir: URL, from: String, to: String, path: String) async throws -> Data {
        record("diffPath:\(path)")
        return Data(pathDiff.utf8)
    }
    func fileContents(repoDir: URL, sha: String, path: String) async throws -> Data {
        record("fileContents:\(path)")
        return Data(fileContentsText.utf8)
    }
    func headSHA(_ wt: Worktree) async throws -> String { record("headSHA"); return head }
    func commitDiff(_ wt: Worktree, sha: String) async throws -> Data { Data() }
    /// Whether a worktree "exists on disk" for ensureExistingWorktree.
    var hasExistingWorktree = false
    func ensureExistingWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree? {
        record("ensureExisting")
        return hasExistingWorktree ? worktree : nil
    }
    func remove(_ wt: Worktree) async throws { record("remove") }
}

// MARK: - Fake Claude session

final class FakeClaudeSession: ClaudeSessionProtocol, @unchecked Sendable {
    let config: ClaudeConfig
    let output: AsyncStream<SessionOutput>
    let continuation: AsyncStream<SessionOutput>.Continuation
    private let lock = NSLock()
    private var _sent: [String] = []
    private var _interrupts = 0
    private var _terminated = false
    private var _turnActive = false
    var turnActive: Bool {
        get { lock.withLock { _turnActive } }
        set { lock.withLock { _turnActive = newValue } }
    }
    var sent: [String] { lock.withLock { _sent } }
    var interrupts: Int { lock.withLock { _interrupts } }
    var terminated: Bool { lock.withLock { _terminated } }

    init(config: ClaudeConfig) {
        self.config = config
        var c: AsyncStream<SessionOutput>.Continuation!
        output = AsyncStream { c = $0 }
        continuation = c
    }

    func send(_ text: String) throws { lock.withLock { _sent.append(text) } }
    func interrupt() { lock.withLock { _interrupts += 1 } }
    func terminate() { lock.withLock { _terminated = true }; continuation.finish() }

    /// Emits a full assistant turn: init, text, result.
    func reply(_ text: String, sessionId: String = "sess-1") {
        continuation.yield(.event(.initialized(sessionId: sessionId, model: "test")))
        continuation.yield(.event(.assistant(messageId: UUID().uuidString, blocks: [.text(text)])))
        continuation.yield(.event(.result(ClaudeResult(sessionId: sessionId, isError: false, subtype: "success",
                                                       text: text, costUSD: 0, structuredOutput: nil))))
    }
}

/// Factory that keeps every session it creates.
@MainActor
final class SessionFactory {
    var sessions: [FakeClaudeSession] = []
    func make(_ cfg: ClaudeConfig) -> ClaudeSessionProtocol {
        let s = FakeClaudeSession(config: cfg)
        sessions.append(s)
        return s
    }
}

// MARK: - Fake one-shot

final class FakeOneShot: ClaudeOneShotRunning, @unchecked Sendable {
    private let lock = NSLock()
    private var _configs: [ClaudeConfig] = []
    private var _prompts: [String] = []
    var structured: Data?
    var text: String?
    var error: Error?

    var configs: [ClaudeConfig] { lock.withLock { _configs } }
    var prompts: [String] { lock.withLock { _prompts } }

    func run(config: ClaudeConfig, prompt: String) async throws -> ClaudeResult {
        lock.withLock { _configs.append(config); _prompts.append(prompt) }
        if let error { throw error }
        return ClaudeResult(sessionId: "fork", isError: false, subtype: "success", text: text, costUSD: 0,
                            structuredOutput: structured)
    }
}

// MARK: - GitHub wrappers

struct TestError: Error, LocalizedError {
    var message = "boom"
    var errorDescription: String? { message }
}

/// Delegates to DemoGitHub but can fail selected calls.
final class FlakyGitHub: GitHubAPI, @unchecked Sendable {
    let base: DemoGitHub
    var failViewed = false
    var failSearch = false
    var failResolve = false
    init(base: DemoGitHub) { self.base = base }

    func search(_ query: String) async throws -> [PRSummary] {
        if failSearch && query.contains("author:@me") { throw TestError(message: "search failed") }
        return try await base.search(query)
    }
    func pullRequest(_ ref: PRRef) async throws -> PRDetail { try await base.pullRequest(ref) }
    func diff(_ ref: PRRef) async throws -> Data { try await base.diff(ref) }
    func fileContents(owner: String, repo: String, path: String, ref: String) async throws -> Data {
        try await base.fileContents(owner: owner, repo: repo, path: path, ref: ref)
    }
    func startReview(pullRequestId: String) async throws -> String { try await base.startReview(pullRequestId: pullRequestId) }
    func addThread(pullRequestId: String, reviewId: String, anchor: CodeAnchor, body: String) async throws -> ReviewThread {
        try await base.addThread(pullRequestId: pullRequestId, reviewId: reviewId, anchor: anchor, body: body)
    }
    func reply(threadId: String, reviewId: String?, body: String) async throws -> ReviewComment {
        try await base.reply(threadId: threadId, reviewId: reviewId, body: body)
    }
    func addComment(subjectId: String, body: String) async throws -> PRConversation.Comment {
        try await base.addComment(subjectId: subjectId, body: body)
    }
    func updateComment(id: String, body: String) async throws -> ReviewComment { try await base.updateComment(id: id, body: body) }
    func deleteComment(id: String) async throws { try await base.deleteComment(id: id) }
    func submitReview(reviewId: String, event: ReviewEvent, body: String) async throws {
        try await base.submitReview(reviewId: reviewId, event: event, body: body)
    }
    func mergePullRequest(id: String, method: MergeMethod, expectedHeadOid: String) async throws {
        try await base.mergePullRequest(id: id, method: method, expectedHeadOid: expectedHeadOid)
    }
    func setPullRequestClosed(id: String, closed: Bool) async throws {
        try await base.setPullRequestClosed(id: id, closed: closed)
    }
    func deleteReview(reviewId: String) async throws { try await base.deleteReview(reviewId: reviewId) }
    func setResolved(threadId: String, resolved: Bool) async throws -> Bool {
        if failResolve { throw TestError() }
        return try await base.setResolved(threadId: threadId, resolved: resolved)
    }
    func setViewed(pullRequestId: String, path: String, viewed: Bool) async throws {
        if failViewed { throw TestError(message: "viewed failed") }
        try await base.setViewed(pullRequestId: pullRequestId, path: path, viewed: viewed)
    }
}

// MARK: - Fake diff view

@MainActor
final class FakeDiffView: DiffViewDriving {
    var top = 0
    var scrolls: [Int] = []
    var layoutChanges = 0
    var reloadedFiles: [Int] = []
    func layoutDidChange(preserveScroll: Bool) { layoutChanges += 1 }
    func reloadFile(_ file: Int) { reloadedFiles.append(file) }
    func scrollToRow(_ row: Int, select: Bool) { scrolls.append(row); top = row }
    var topVisibleRow: Int { top }
    func focusDiff() {}
}

// MARK: - Harness

let testShape = SyntheticDiff.Shape(files: 6, hunksPerFile: 2, linesPerHunk: 12)

@MainActor
struct Harness {
    let demo: DemoGitHub
    let github: GitHubAPI
    let services: Services
    let worktrees: FakeWorktrees
    let oneShot: FakeOneShot
    let sessions: SessionFactory
    let root: URL
    let store: PRStore
    let view = FakeDiffView()

    static let ref = PRRef(owner: "demo", repo: "synthetic", number: 1)

    init(github wrap: ((DemoGitHub) -> GitHubAPI)? = nil, claudePath: String? = "/usr/bin/true",
         threadsEvery: Int = 25, root: URL? = nil, worktrees: Bool = true) {
        demo = DemoGitHub.synthetic(testShape, threadsEvery: threadsEvery)
        github = wrap?(demo) ?? demo
        self.root = root ?? FileManager.default.temporaryDirectory.appendingPathComponent("rb-ui-\(UUID().uuidString)")
        self.worktrees = FakeWorktrees()
        oneShot = FakeOneShot()
        let sessions = SessionFactory()
        self.sessions = sessions
        services = Services(github: github, store: LocalStore(root: self.root), settings: AppSettings(),
                            worktrees: worktrees ? self.worktrees : nil, claudePath: claudePath,
                            makeSession: { sessions.make($0) }, oneShot: oneShot)
        store = PRStore(ref: Self.ref, services: services)
        store.view = view
    }

    func load() async {
        await store.load()
    }
}

/// Waits (polling the main actor) until `cond` holds or times out.
@MainActor
func eventually(timeout: Double = 3, _ cond: () -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return true }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return cond()
}

extension PRStore {
    /// First addition line (new number) in file `f`.
    func firstAddition(file f: Int) -> Int { Int(files[f].lines.first { $0.kind == .addition }!.newLine) }
}
