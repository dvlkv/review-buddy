import Foundation
import Testing
@testable import ReviewBuddyCore

/// Records requests and answers with canned responses.
final class FakeTransport: HTTPTransport, @unchecked Sendable {
    struct Sent { var request: URLRequest; var query: String; var variables: [String: Any] }
    private let lock = NSLock()
    private var _sent: [Sent] = []
    var handler: @Sendable (URLRequest, String, [String: Any]) -> (Int, String)

    init(handler: @escaping @Sendable (URLRequest, String, [String: Any]) -> (Int, String)) { self.handler = handler }

    var sent: [Sent] { lock.withLock { _sent } }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        var query = "", vars: [String: Any] = [:]
        if let body = request.httpBody, let obj = try? JSONSerialization.jsonObject(with: body) as? [String: Any] {
            query = obj["query"] as? String ?? ""
            vars = obj["variables"] as? [String: Any] ?? [:]
        }
        lock.withLock { _sent.append(Sent(request: request, query: query, variables: vars)) }
        let (status, body) = handler(request, query, vars)
        let resp = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        return (Data(body.utf8), resp)
    }
}

func makeClient(_ handler: @escaping @Sendable (URLRequest, String, [String: Any]) -> (Int, String)) -> (GitHubClient, FakeTransport) {
    let t = FakeTransport(handler: handler)
    return (GitHubClient(transport: t, tokens: StaticTokenProvider("tok123")), t)
}

@Suite struct GitHubClientTests {
    static let threadJSON = """
    {"id":"T1","path":"a.swift","line":12,"startLine":10,"originalLine":12,"diffSide":"RIGHT","isResolved":false,"isOutdated":false,
     "comments":{"nodes":[{"id":"C1","databaseId":5,"body":"hi","createdAt":"2026-01-02T03:04:05Z","state":"PENDING","url":"https://github.com/x","viewerDidAuthor":true,"author":{"login":"bob","avatarUrl":"https://a/b.png"}}]}}
    """

    func prJSON(filesNext: Bool, threadsNext: Bool, permission: String = "WRITE", pending: Bool = true) -> String {
        """
        {"data":{"viewer":{"login":"me"},"repository":{"viewerPermission":"\(permission)","pullRequest":{
        "id":"PR_1","title":"Title","body":"Body","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"feat",
        "baseRefOid":"aaa","headRefOid":"bbb","maintainerCanModify":true,"changedFiles":3,"additions":10,"deletions":2,
        "author":{"login":"alice","avatarUrl":null},"headRepository":{"nameWithOwner":"o/r","sshUrl":"git@github.com:o/r.git","url":"https://github.com/o/r"},
        "files":{"pageInfo":{"hasNextPage":\(filesNext),"endCursor":"F1"},"nodes":[{"path":"a.swift","additions":5,"deletions":1,"viewerViewedState":"VIEWED"}]},
        "reviewThreads":{"pageInfo":{"hasNextPage":\(threadsNext),"endCursor":"T1c"},"nodes":[\(Self.threadJSON)]},
        "reviews":{"nodes":[\(pending ? #"{"id":"R_other","viewerDidAuthor":false},{"id":"R_mine","viewerDidAuthor":true}"# : "")]}
        }}}}
        """
    }

    @Test func sendsAuthHeaders() async throws {
        let (c, t) = makeClient { _, _, _ in (200, #"{"data":{"search":{"nodes":[]}}}"#) }
        _ = try await c.search("x")
        let r = t.sent[0].request
        #expect(r.value(forHTTPHeaderField: "Authorization") == "Bearer tok123")
        #expect(r.httpMethod == "POST")
        #expect(r.url?.absoluteString == "https://api.github.com/graphql")
    }

    @Test func searchMapsSummaries() async throws {
        let (c, t) = makeClient { _, _, _ in
            (200, """
            {"data":{"search":{"nodes":[
              {"number":7,"title":"Fix","isDraft":true,"updatedAt":"2026-05-01T00:00:00Z","additions":3,"deletions":4,"reviewDecision":"APPROVED",
               "repository":{"name":"r","owner":{"login":"o"}},"author":{"login":"al","avatarUrl":"https://x/y.png"},"comments":{"totalCount":9},
               "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]}},
              {}
            ]}}}
            """)
        }
        let res = try await c.search(InboxSection.reviewRequested.query)
        #expect(t.sent[0].variables["q"] as? String == InboxSection.reviewRequested.query)
        #expect(res.count == 1)
        let s = res[0]
        #expect(s.ref == PRRef(owner: "o", repo: "r", number: 7))
        #expect(s.title == "Fix" && s.author == "al" && s.isDraft)
        #expect(s.checks == .failure && s.reviewDecision == .approved && s.commentCount == 9)
        #expect(s.additions == 3 && s.deletions == 4)
    }

    @Test func pullRequestPaginates() async throws {
        let first = prJSON(filesNext: true, threadsNext: true)
        let (c, t) = makeClient { _, q, vars in
            if q.contains("viewer { login }") { return (200, first) }
            if q.contains("files(first: 100, after") {
                let after = vars["after"] as? String
                let next = after == "F1"
                return (200, """
                {"data":{"repository":{"pullRequest":{"files":{"pageInfo":{"hasNextPage":\(next),"endCursor":"F2"},
                "nodes":[{"path":"f-\(after ?? "")","additions":1,"deletions":0,"viewerViewedState":"UNVIEWED"}]}}}}}
                """)
            }
            if q.contains("reviewThreads(first: 100, after") {
                let t2 = Self.threadJSON.replacingOccurrences(of: "\"T1\"", with: "\"T2\"")
                return (200, #"{"data":{"repository":{"pullRequest":{"reviewThreads":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":["# + t2 + "]}}}}}")
            }
            return (500, "unexpected")
        }
        let pr = try await c.pullRequest(PRRef(owner: "o", repo: "r", number: 1))
        #expect(pr.files.map(\.path) == ["a.swift", "f-F1", "f-F2"])
        #expect(pr.files[0].viewed == .viewed && pr.files[1].viewed == .unviewed)
        #expect(pr.threads.map(\.id) == ["T1", "T2"])
        #expect(pr.pendingReviewId == "R_mine")
        #expect(pr.viewerCanPush && pr.viewerLogin == "me")
        #expect(pr.nodeId == "PR_1" && pr.baseOid == "aaa" && pr.headOid == "bbb" && pr.author == "alice")
        let th = pr.threads[0]
        #expect(th.line == 12 && th.startLine == 10 && th.side == .right)
        #expect(th.comments[0].isPending && th.comments[0].viewerDidAuthor && th.comments[0].databaseId == 5)
        #expect(th.isPending)
        #expect(t.sent.count == 4)
        #expect(t.sent[1].variables["number"] as? Int == 1)
    }

    @Test func pullRequestReadPermissionAndNoPending() async throws {
        let body = prJSON(filesNext: false, threadsNext: false, permission: "READ", pending: false)
        let (c, t) = makeClient { _, _, _ in (200, body) }
        let pr = try await c.pullRequest(PRRef(owner: "o", repo: "r", number: 1))
        #expect(!pr.viewerCanPush)
        #expect(pr.pendingReviewId == nil)
        #expect(t.sent.count == 1)
    }

    @Test func pullRequestNullIsNotFound() async {
        let (c, _) = makeClient { _, _, _ in (200, #"{"data":{"viewer":{"login":"me"},"repository":{"viewerPermission":"READ","pullRequest":null}}}"#) }
        await #expect(throws: GitHubError.notFound) { try await c.pullRequest(PRRef(owner: "o", repo: "r", number: 1)) }
    }

    @Test func graphQLErrors() async {
        let (c, _) = makeClient { _, _, _ in (200, #"{"data":null,"errors":[{"message":"bad thing"},{"message":"worse"}]}"#) }
        await #expect(throws: GitHubError.graphQL(["bad thing", "worse"])) { _ = try await c.search("x") }
        let (c2, _) = makeClient { _, _, _ in (200, #"{"data":null,"errors":[{"message":"nope","type":"NOT_FOUND"}]}"#) }
        await #expect(throws: GitHubError.notFound) { _ = try await c2.search("x") }
    }

    /// Real GitHub response for approving your own PR: data present but null, plus errors.
    @Test func refusedMutationWithNullDataThrowsGitHubMessage() async {
        let (c, _) = makeClient { _, _, _ in
            (200, #"{"data":{"submitPullRequestReview":null},"errors":[{"type":"UNPROCESSABLE","path":["submitPullRequestReview"],"message":"Could not approve for pull request review. Can not approve your own pull request"}]}"#)
        }
        do {
            try await c.submitReview(reviewId: "R", event: .approve, body: "")
            Issue.record("expected an error")
        } catch let e as GitHubError {
            #expect(e == .graphQL(["Could not approve for pull request review. Can not approve your own pull request"]))
        } catch {
            Issue.record("wrong error \(error)")
        }
    }

    @Test func refusedResolveThrowsInsteadOfDecodingError() async {
        let (c, _) = makeClient { _, _, _ in
            (200, #"{"data":{"resolveReviewThread":null},"errors":[{"type":"FORBIDDEN","message":"Resource not accessible"}]}"#)
        }
        await #expect(throws: GitHubError.graphQL(["Resource not accessible"])) {
            _ = try await c.setResolved(threadId: "T", resolved: true)
        }
    }

    @Test func queryKeepsPartialDataDespiteErrors() async throws {
        // A search where one result is inaccessible still returns the others.
        let (c, _) = makeClient { _, _, _ in
            (200, #"{"data":{"search":{"nodes":[{"number":1,"title":"t","repository":{"name":"r","owner":{"login":"o"}},"updatedAt":"2026-01-01T00:00:00Z"},{}]}},"errors":[{"type":"FORBIDDEN","message":"no access to one repo"}]}"#)
        }
        let r = try await c.search("x")
        #expect(r.count == 1)
    }

    @Test func httpErrors() async {
        let (c, _) = makeClient { _, _, _ in (401, "unauthorized") }
        await #expect(throws: GitHubError.notAuthenticated) { _ = try await c.search("x") }
        let (c2, _) = makeClient { _, _, _ in (502, "gateway") }
        await #expect(throws: GitHubError.http(502, "gateway")) { _ = try await c2.search("x") }
    }

    @Test func diffStatuses() async throws {
        let (c, t) = makeClient { _, _, _ in (200, "diff --git a/x b/x\n") }
        let ref = PRRef(owner: "o", repo: "r", number: 3)
        let d = try await c.diff(ref)
        #expect(String(decoding: d, as: UTF8.self).hasPrefix("diff --git"))
        #expect(t.sent[0].request.url?.path == "/repos/o/r/pulls/3")
        #expect(t.sent[0].request.value(forHTTPHeaderField: "Accept") == "application/vnd.github.diff")
        for (code, err) in [(406, GitHubError.diffTooLarge), (422, .diffTooLarge), (404, .notFound), (500, .http(500, "x"))] {
            let (c2, _) = makeClient { _, _, _ in (code, "x") }
            await #expect(throws: err) { _ = try await c2.diff(ref) }
        }
    }

    @Test func fileContents() async throws {
        let (c, t) = makeClient { _, _, _ in (200, "content") }
        let d = try await c.fileContents(owner: "o", repo: "r", path: "src/a b.swift", ref: "abc")
        #expect(String(decoding: d, as: UTF8.self) == "content")
        let url = t.sent[0].request.url!
        #expect(url.path == "/repos/o/r/contents/src/a b.swift")
        #expect(url.query == "ref=abc")
        let (c2, _) = makeClient { _, _, _ in (404, "") }
        await #expect(throws: GitHubError.notFound) { _ = try await c2.fileContents(owner: "o", repo: "r", path: "x", ref: "y") }
    }

    @Test func startReview() async throws {
        let (c, t) = makeClient { _, _, _ in (200, #"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R1"}}}}"#) }
        #expect(try await c.startReview(pullRequestId: "PR_1") == "R1")
        #expect(t.sent[0].query.contains("addPullRequestReview("))
        #expect(t.sent[0].variables["pr"] as? String == "PR_1")
    }

    @Test func addThreadSingleAndMultiLine() async throws {
        let resp = #"{"data":{"addPullRequestReviewThread":{"thread":"# + Self.threadJSON + "}}}"
        let (c, t) = makeClient { _, _, _ in (200, resp) }
        let single = try await c.addThread(pullRequestId: "PR", reviewId: "R", anchor: CodeAnchor(path: "a.swift", side: .left, startLine: 4, endLine: 4), body: "b")
        #expect(single.id == "T1")
        var v = t.sent[0].variables
        #expect(t.sent[0].query.contains("addPullRequestReviewThread"))
        #expect(v["path"] as? String == "a.swift" && v["line"] as? Int == 4 && v["side"] as? String == "LEFT")
        #expect(v["startLine"] == nil && v["startSide"] == nil)
        #expect(v["body"] as? String == "b" && v["review"] as? String == "R" && v["pr"] as? String == "PR")
        _ = try await c.addThread(pullRequestId: "PR", reviewId: "R", anchor: CodeAnchor(path: "a.swift", side: .right, startLine: 9, endLine: 3), body: "m")
        v = t.sent[1].variables
        #expect(v["line"] as? Int == 9 && v["startLine"] as? Int == 3 && v["startSide"] as? String == "RIGHT")
    }

    @Test func addThreadNullThreadThrows() async {
        let (c, _) = makeClient { _, _, _ in (200, #"{"data":{"addPullRequestReviewThread":{"thread":null}}}"#) }
        await #expect(throws: GitHubError.self) {
            _ = try await c.addThread(pullRequestId: "P", reviewId: "R", anchor: CodeAnchor(path: "a", side: .right, startLine: 1, endLine: 1), body: "x")
        }
    }

    static let commentJSON = #"{"id":"C9","databaseId":9,"body":"yo","createdAt":"2026-01-02T03:04:05Z","state":"SUBMITTED","url":null,"viewerDidAuthor":true,"author":{"login":"me","avatarUrl":null}}"#

    @Test func replyAndUpdate() async throws {
        let (c, t) = makeClient { _, q, _ in
            if q.contains("addPullRequestReviewThreadReply") {
                return (200, #"{"data":{"addPullRequestReviewThreadReply":{"comment":"# + Self.commentJSON + "}}}")
            }
            return (200, #"{"data":{"updatePullRequestReviewComment":{"pullRequestReviewComment":"# + Self.commentJSON + "}}}")
        }
        let r = try await c.reply(threadId: "T1", reviewId: nil, body: "yo")
        #expect(r.id == "C9" && !r.isPending && r.author == "me")
        #expect(t.sent[0].variables["thread"] as? String == "T1")
        #expect(t.sent[0].variables["review"] == nil)
        _ = try await c.reply(threadId: "T1", reviewId: "R1", body: "yo")
        #expect(t.sent[1].variables["review"] as? String == "R1")
        let u = try await c.updateComment(id: "C9", body: "new")
        #expect(u.body == "yo")
        #expect(t.sent[2].query.contains("updatePullRequestReviewComment"))
        #expect(t.sent[2].variables["id"] as? String == "C9" && t.sent[2].variables["body"] as? String == "new")
    }

    @Test func simpleMutations() async throws {
        let (c, t) = makeClient { _, q, _ in
            if q.contains("resolveReviewThread(") && !q.contains("unresolve") {
                return (200, #"{"data":{"resolveReviewThread":{"thread":{"id":"T","isResolved":true}}}}"#)
            }
            if q.contains("unresolveReviewThread") {
                return (200, #"{"data":{"unresolveReviewThread":{"thread":{"id":"T","isResolved":false}}}}"#)
            }
            return (200, #"{"data":{"x":{"clientMutationId":null}}}"#)
        }
        try await c.deleteComment(id: "C1")
        #expect(t.sent[0].query.contains("deletePullRequestReviewComment") && t.sent[0].variables["id"] as? String == "C1")
        try await c.submitReview(reviewId: "R", event: .requestChanges, body: "pls")
        #expect(t.sent[1].query.contains("submitPullRequestReview"))
        #expect(t.sent[1].variables["event"] as? String == "REQUEST_CHANGES" && t.sent[1].variables["body"] as? String == "pls")
        try await c.deleteReview(reviewId: "R")
        #expect(t.sent[2].query.contains("deletePullRequestReview(") && t.sent[2].variables["review"] as? String == "R")
        #expect(try await c.setResolved(threadId: "T", resolved: true) == true)
        #expect(t.sent[3].query.contains("resolveReviewThread") && t.sent[3].variables["id"] as? String == "T")
        #expect(try await c.setResolved(threadId: "T", resolved: false) == false)
        #expect(t.sent[4].query.contains("unresolveReviewThread"))
        try await c.setViewed(pullRequestId: "PR", path: "a.swift", viewed: true)
        #expect(t.sent[5].query.contains("markFileAsViewed") && !t.sent[5].query.contains("unmark"))
        #expect(t.sent[5].variables["path"] as? String == "a.swift" && t.sent[5].variables["pr"] as? String == "PR")
        try await c.setViewed(pullRequestId: "PR", path: "a.swift", viewed: false)
        #expect(t.sent[6].query.contains("unmarkFileAsViewed"))
    }
}

@Suite struct PRRefTests {
    @Test func parsesVariants() {
        let expected = PRRef(owner: "apple", repo: "swift", number: 123)
        for s in ["https://github.com/apple/swift/pull/123", "http://github.com/apple/swift/pull/123/files",
                  "github.com/apple/swift/pull/123", "  https://www.github.com/apple/swift/pull/123#discussion_r1 ",
                  "apple/swift#123", "https://github.com/apple/swift/pulls/123"] {
            #expect(PRRef(string: s) == expected, "\(s)")
        }
    }
    @Test func rejectsInvalid() {
        for s in ["", "apple/swift", "https://gitlab.com/a/b/pull/1", "https://github.com/a/b/issues/1",
                  "a#1", "a/b#x", "https://github.com/a/b/pull/"] {
            #expect(PRRef(string: s) == nil, "\(s)")
        }
    }
    @Test func derived() {
        let r = PRRef(owner: "o", repo: "r", number: 5)
        #expect(r.description == "o/r#5" && r.nameWithOwner == "o/r")
        #expect(r.webURL.absoluteString == "https://github.com/o/r/pull/5")
        #expect(r.storageKey == "o__r__5")
    }

    func detail(head: String?, maintainer: Bool, viewer: String, canPush: Bool) -> PRDetail {
        PRDetail(ref: PRRef(owner: "o", repo: "r", number: 1), nodeId: "x", title: "", body: "", author: "alice",
                 baseRefName: "main", headRefName: "f", baseOid: "a", headOid: "b", headRepoNameWithOwner: head,
                 maintainerCanModify: maintainer, viewerLogin: viewer, viewerCanPush: canPush)
    }

    @Test func forkAndPushRules() {
        #expect(!detail(head: "O/R", maintainer: false, viewer: "me", canPush: true).isFromFork)
        #expect(detail(head: "fork/r", maintainer: false, viewer: "me", canPush: true).isFromFork)
        #expect(detail(head: nil, maintainer: false, viewer: "me", canPush: true).isFromFork)
        #expect(detail(head: "o/r", maintainer: false, viewer: "me", canPush: true).canPushFixes)
        #expect(!detail(head: "o/r", maintainer: false, viewer: "me", canPush: false).canPushFixes)
        #expect(detail(head: "o/r", maintainer: false, viewer: "alice", canPush: false).canPushFixes)
        #expect(detail(head: "fork/r", maintainer: true, viewer: "me", canPush: false).canPushFixes)
        #expect(!detail(head: "fork/r", maintainer: false, viewer: "me", canPush: true).canPushFixes)
        #expect(detail(head: "fork/r", maintainer: false, viewer: "alice", canPush: false).canPushFixes)
    }

    @Test func anchorNormalizesAndLabels() {
        let a = CodeAnchor(path: "src/x/file.swift", side: .right, startLine: 9, endLine: 3)
        #expect(a.startLine == 3 && a.endLine == 9 && a.isMultiline)
        #expect(a.label == "file.swift L3-9")
        #expect(CodeAnchor(path: "f", side: .left, startLine: 2, endLine: 2).label == "f L2")
    }
}
