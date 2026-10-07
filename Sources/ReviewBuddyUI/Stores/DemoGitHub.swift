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
            additions: diff.files.reduce(0) { $0 + $1.additions }, deletions: diff.files.reduce(0) { $0 + $1.deletions })
        return DemoGitHub(diff: Data(text.utf8), detail: detail)
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

    public func updateComment(id: String, body: String) async throws -> ReviewComment {
        record("updateComment")
        return ReviewComment(id: id, body: body, author: "you", createdAt: Date(), isPending: true, viewerDidAuthor: true)
    }

    public func deleteComment(id: String) async throws { record("deleteComment") }
    public func submitReview(reviewId: String, event: ReviewEvent, body: String) async throws {
        record("submitReview:\(event.rawValue)")
        lock.withLock { detail.pendingReviewId = nil }
    }
    public func deleteReview(reviewId: String) async throws { record("deleteReview") }
    public func setResolved(threadId: String, resolved: Bool) async throws -> Bool { record("setResolved:\(resolved)"); return resolved }
    public func setViewed(pullRequestId: String, path: String, viewed: Bool) async throws { record("setViewed:\(viewed)") }
}
