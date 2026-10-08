import Foundation
import ReviewBuddyCore

/// In-memory GitHub used by `--synthetic` / `--fixture` launch modes (the
/// performance loop) and by UI tests. Mutations behave like the real API.
public final class DemoGitHub: GitHubAPI, @unchecked Sendable {
    private let lock = NSLock()
    private var diffData: Data
    private var detail: PRDetail
    private var nextId = 1
    public private(set) var calls: [String] = []

    public init(diff: Data, detail: PRDetail) {
        self.diffData = diff
        self.detail = detail
    }

    /// A synthetic PR of the given shape with a sprinkling of review threads.
    public static func synthetic(_ shape: SyntheticDiff.Shape, threadsEvery: Int = 25) -> DemoGitHub {
        let text = SyntheticDiff.make(shape)
        let diff = UnifiedDiffParser.parse(text)
        let ref = PRRef(owner: "demo", repo: "synthetic", number: 1)
        var threads: [ReviewThread] = []
        var files: [PRFileMeta] = []
        for (i, f) in diff.files.enumerated() {
            files.append(PRFileMeta(path: f.path, additions: f.additions, deletions: f.deletions, viewed: .unviewed))
            guard threadsEvery > 0, i % threadsEvery == 0,
                  let line = f.lines.first(where: { $0.kind == .addition }) else { continue }
            threads.append(ReviewThread(
                id: "T\(i)", path: f.path, line: Int(line.newLine), side: .right, isResolved: i % (threadsEvery * 3) == 0,
                comments: [
                    ReviewComment(id: "C\(i)a", body: "Should this handle the empty case? `filtered.first` can be nil here.",
                                  author: "octocat", createdAt: Date().addingTimeInterval(-7200)),
                    ReviewComment(id: "C\(i)b", body: "Good catch — the guard above covers it, but I'll add a test.",
                                  author: "monalisa", createdAt: Date().addingTimeInterval(-3600)),
                ]))
        }
        let detail = PRDetail(
            ref: ref, nodeId: "PR_demo", title: "Refactor processing pipeline across \(shape.files) modules",
            body: "This PR reworks the item processing pipeline.\n\n- Moves scoring into `Result`\n- Adds async fetching\n- Updates call sites",
            author: "monalisa", baseRefName: "main", headRefName: "feature/pipeline",
            baseOid: String(repeating: "a", count: 40), headOid: String(repeating: "b", count: 40),
            headRepoNameWithOwner: ref.nameWithOwner, maintainerCanModify: true, viewerLogin: "you", viewerCanPush: true,
            files: files, threads: threads, changedFiles: diff.files.count,
            additions: diff.files.reduce(0) { $0 + $1.additions }, deletions: diff.files.reduce(0) { $0 + $1.deletions },
            conversation: demoConversation,
            status: PRMergeStatus(mergeable: "MERGEABLE", mergeStateStatus: "BLOCKED", reviewDecision: "REVIEW_REQUIRED", checks: [
                .init(name: "build", app: "GitHub Actions", state: "SUCCESS"),
                .init(name: "test", app: "GitHub Actions", state: "FAILURE"),
                .init(name: "lint", app: "GitHub Actions", state: "PENDING"),
            ], allowedMergeMethods: [.merge, .squash], defaultMergeMethod: .squash))
        return DemoGitHub(diff: Data(text.utf8), detail: detail)
    }

    /// A small conversation so the synthetic PR's Conversation screen isn't empty.
    static var demoConversation: PRConversation {
        let base = Date(timeIntervalSinceNow: -5 * 86_400)
        func at(_ h: Double) -> Date { base.addingTimeInterval(h * 3600) }
        return PRConversation(
            comments: [
                .init(id: "IC1", author: "octocat", body: "Could we split the scoring change into its own PR?", createdAt: at(30)),
                .init(id: "IC2", author: "monalisa", body: "Kept it here — the call sites change together. Added tests instead.", createdAt: at(52)),
            ],
            reviews: [
                .init(id: "RV1", author: "octocat", state: "CHANGES_REQUESTED", body: "A couple of edge cases need handling.", submittedAt: at(31)),
                .init(id: "RV2", author: "hubot", state: "APPROVED", body: "", submittedAt: at(70)),
            ],
            commits: [
                .init(oid: "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b", headline: "Move scoring into Result", author: "monalisa", date: at(0), checks: .success),
                .init(oid: "2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c", headline: "Add async fetching", author: "monalisa", date: at(5), checks: .success),
                .init(oid: "3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d", headline: "Handle empty input; add tests", author: "monalisa", date: at(50), checks: .failure),
            ])
    }

    private func record(_ s: String) { lock.withLock { calls.append(s) } }
    private func newId(_ p: String) -> String { lock.withLock { nextId += 1; return "\(p)\(nextId)" } }

    public func search(_ query: String) async throws -> [PRSummary] {
        record("search")
        return [PRSummary(ref: detail.ref, title: detail.title, author: detail.author, updatedAt: Date(),
                          additions: detail.additions, deletions: detail.deletions, checks: .success,
                          reviewDecision: .reviewRequired)]
    }

    public func pullRequest(_ ref: PRRef) async throws -> PRDetail { record("pullRequest"); return lock.withLock { detail } }
    public func diff(_ ref: PRRef) async throws -> Data { record("diff"); return diffData }
    public func headOid(_ ref: PRRef) async throws -> String { record("headOid"); return lock.withLock { detail.headOid } }
    /// Simulates a push to the PR (demo and tests).
    public func simulatePush(headOid: String) { lock.withLock { detail.headOid = headOid } }
    public func fileContents(owner: String, repo: String, path: String, ref: String) async throws -> Data {
        record("fileContents")
        let lines = (1...400).map { "    // \(path) line \($0)" }
        return Data(lines.joined(separator: "\n").utf8)
    }

    public func startReview(pullRequestId: String) async throws -> String {
        record("startReview")
        let id = newId("R")
        lock.withLock { detail.pendingReviewId = id }
        return id
    }

    public func addThread(pullRequestId: String, reviewId: String, anchor: CodeAnchor, body: String) async throws -> ReviewThread {
        record("addThread")
        let t = ReviewThread(id: newId("T"), path: anchor.path, line: anchor.endLine,
                             startLine: anchor.isMultiline ? anchor.startLine : nil, side: anchor.side,
                             comments: [ReviewComment(id: newId("C"), body: body, author: "you", createdAt: Date(),
                                                      isPending: true, viewerDidAuthor: true)])
        lock.withLock { detail.threads.append(t) }
        return t
    }

    public func reply(threadId: String, reviewId: String?, body: String) async throws -> ReviewComment {
        record("reply")
        return ReviewComment(id: newId("C"), body: body, author: "you", createdAt: Date(), isPending: reviewId != nil, viewerDidAuthor: true)
    }

    public func addSingleComment(ref: PRRef, commitId: String, anchor: CodeAnchor, body: String) async throws {
        record("addSingleComment")
        let t = ReviewThread(id: newId("T"), path: anchor.path, line: anchor.endLine,
                             startLine: anchor.isMultiline ? anchor.startLine : nil, side: anchor.side,
                             comments: [ReviewComment(id: newId("C"), databaseId: lock.withLock { nextId }, body: body, author: "you",
                                                      createdAt: Date(), viewerDidAuthor: true)])
        lock.withLock { detail.threads.append(t) }
    }

    public func replySingle(ref: PRRef, threadId: String, commentDatabaseId: Int, body: String) async throws {
        record("replySingle")
        let c = ReviewComment(id: newId("C"), body: body, author: "you", createdAt: Date(), viewerDidAuthor: true)
        lock.withLock {
            if let i = detail.threads.firstIndex(where: { $0.id == threadId }) { detail.threads[i].comments.append(c) }
        }
    }

    public func addComment(subjectId: String, body: String) async throws -> PRConversation.Comment {
        record("addComment")
        return .init(id: newId("IC"), author: "you", body: body, createdAt: Date())
    }

    public func updateComment(id: String, body: String) async throws -> ReviewComment {
        record("updateComment")
        return ReviewComment(id: id, body: body, author: "you", createdAt: Date(), isPending: true, viewerDidAuthor: true)
    }

    public func deleteComment(id: String) async throws { record("deleteComment") }
    public func submitReview(reviewId: String, event: ReviewEvent, body: String) async throws {
        record("submitReview:\(event.rawValue)")
        lock.withLock { detail.pendingReviewId = nil }
    }
    public func mergePullRequest(id: String, method: MergeMethod, expectedHeadOid: String) async throws {
        record("merge:\(method.rawValue)")
        lock.withLock { detail.state = "MERGED" }
    }
    public func setPullRequestClosed(id: String, closed: Bool) async throws {
        record(closed ? "close" : "reopen")
        lock.withLock { detail.state = closed ? "CLOSED" : "OPEN" }
    }
    public func deleteReview(reviewId: String) async throws { record("deleteReview") }
    public func setResolved(threadId: String, resolved: Bool) async throws -> Bool { record("setResolved:\(resolved)"); return resolved }
    public func setViewed(pullRequestId: String, path: String, viewed: Bool) async throws { record("setViewed:\(viewed)") }
}
