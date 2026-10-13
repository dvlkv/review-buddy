import Foundation

enum Queries {
    static let commentFields = "id databaseId body createdAt state url viewerDidAuthor author { login avatarUrl }"
    static let threadFields = """
    id path line startLine originalLine diffSide isResolved isOutdated
    comments(first: 100) { nodes { \(commentFields) } }
    """

    static let search = """
    query($q: String!) {
      search(query: $q, type: ISSUE, first: 50) {
        nodes {
          ... on PullRequest {
            number title isDraft updatedAt additions deletions reviewDecision
            repository { name owner { login } }
            author { login avatarUrl }
            comments { totalCount }
            commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
          }
        }
      }
    }
    """

    static let pullRequest = """
    query($owner: String!, $name: String!, $number: Int!) {
      viewer { login }
      repository(owner: $owner, name: $name) {
        viewerPermission mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod
        pullRequest(number: $number) {
          id title body state isDraft baseRefName headRefName baseRefOid headRefOid maintainerCanModify
          changedFiles additions deletions
          author { login avatarUrl }
          headRepository { nameWithOwner sshUrl url }
          files(first: 100) { pageInfo { hasNextPage endCursor } nodes { path additions deletions viewerViewedState } }
          reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { \(threadFields) } }
          reviews(states: [PENDING], first: 5) { nodes { id viewerDidAuthor } }
          comments(last: 100) { nodes { id body createdAt url author { login avatarUrl } } }
          allReviews: reviews(last: 100) { nodes { id state body submittedAt url author { login avatarUrl } } }
          commits(first: 250) { nodes { commit { oid messageHeadline committedDate author { name user { login avatarUrl } } statusCheckRollup { state } } } }
          mergeable mergeStateStatus reviewDecision viewerCanMergeAsAdmin
          headCommit: commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
            __typename
            ... on CheckRun { name conclusion status detailsUrl checkSuite { app { name } } }
            ... on StatusContext { context state targetUrl }
          } } } } } }
        }
      }
    }
    """

    static let filesPage = """
    query($owner: String!, $name: String!, $number: Int!, $after: String) {
      repository(owner: $owner, name: $name) { pullRequest(number: $number) {
        files(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { path additions deletions viewerViewedState } }
      } }
    }
    """

    static let threadsPage = """
    query($owner: String!, $name: String!, $number: Int!, $after: String) {
      repository(owner: $owner, name: $name) { pullRequest(number: $number) {
        reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { \(threadFields) } }
      } }
    }
    """

    static let addReview = """
    mutation($pr: ID!) { addPullRequestReview(input: { pullRequestId: $pr }) { pullRequestReview { id } } }
    """

    static let addThread = """
    mutation($pr: ID!, $review: ID!, $path: String!, $body: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide) {
      addPullRequestReviewThread(input: {
        pullRequestId: $pr, pullRequestReviewId: $review, path: $path, body: $body,
        line: $line, side: $side, startLine: $startLine, startSide: $startSide, subjectType: LINE
      }) { thread { \(threadFields) } }
    }
    """

    static let reply = """
    mutation($thread: ID!, $review: ID, $body: String!) {
      addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, pullRequestReviewId: $review, body: $body }) {
        comment { \(commentFields) }
      }
    }
    """

    static let mergePullRequest = """
    mutation($id: ID!, $method: PullRequestMergeMethod!, $head: GitObjectID!) {
      mergePullRequest(input: { pullRequestId: $id, mergeMethod: $method, expectedHeadOid: $head }) {
        pullRequest { state }
      }
    }
    """

    /// Cheap poll for "was the PR pushed to since we loaded it".
    static let headOid = """
    query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) { pullRequest(number: $number) { headRefOid } }
    }
    """

    static let closePullRequest = """
    mutation($id: ID!) { closePullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }
    """

    static let reopenPullRequest = """
    mutation($id: ID!) { reopenPullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }
    """

    static let addComment = """
    mutation($subject: ID!, $body: String!) {
      addComment(input: { subjectId: $subject, body: $body }) {
        commentEdge { node { id body createdAt url author { login avatarUrl } } }
      }
    }
    """

    static let updateComment = """
    mutation($id: ID!, $body: String!) {
      updatePullRequestReviewComment(input: { pullRequestReviewCommentId: $id, body: $body }) {
        pullRequestReviewComment { \(commentFields) }
      }
    }
    """

    static let deleteComment = """
    mutation($id: ID!) { deletePullRequestReviewComment(input: { id: $id }) { clientMutationId } }
    """

    static let submitReview = """
    mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) {
      submitPullRequestReview(input: { pullRequestReviewId: $review, event: $event, body: $body }) { clientMutationId }
    }
    """

    static let deleteReview = """
    mutation($review: ID!) { deletePullRequestReview(input: { pullRequestReviewId: $review }) { clientMutationId } }
    """

    static let resolve = """
    mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id isResolved } } }
    """

    static let unresolve = """
    mutation($id: ID!) { unresolveReviewThread(input: { threadId: $id }) { thread { id isResolved } } }
    """

    static let markViewed = """
    mutation($pr: ID!, $path: String!) { markFileAsViewed(input: { pullRequestId: $pr, path: $path }) { clientMutationId } }
    """

    static let unmarkViewed = """
    mutation($pr: ID!, $path: String!) { unmarkFileAsViewed(input: { pullRequestId: $pr, path: $path }) { clientMutationId } }
    """
}

// MARK: - Response DTOs

struct GraphQLEnvelope<T: Decodable>: Decodable {
    var data: T?
    var errors: [GraphQLErrorDTO]?
}

struct GraphQLErrorsOnly: Decodable {
    var errors: [GraphQLErrorDTO]?
}

struct GraphQLErrorDTO: Decodable {
    var message: String
    var type: String?
}

struct EmptyData: Decodable {}

struct PageInfoDTO: Decodable {
    var hasNextPage: Bool
    var endCursor: String?
}

struct ActorDTO: Decodable {
    var login: String
    var avatarUrl: URL?
}

struct SearchData: Decodable {
    struct Search: Decodable { var nodes: [SearchNode] }
    var search: Search
}

struct SearchNode: Decodable {
    struct Repo: Decodable { var name: String; var owner: ActorDTO }
    struct Count: Decodable { var totalCount: Int }
    struct Commits: Decodable {
        struct Node: Decodable {
            struct Commit: Decodable {
                struct Rollup: Decodable { var state: String }
                var statusCheckRollup: Rollup?
            }
            var commit: Commit
        }
        var nodes: [Node]
    }
    var number: Int?
    var title: String?
    var isDraft: Bool?
    var updatedAt: Date?
    var additions: Int?
    var deletions: Int?
    var reviewDecision: String?
    var repository: Repo?
    var author: ActorDTO?
    var comments: Count?
    var commits: Commits?

    func toSummary() -> PRSummary? {
        guard let number, let title, let repository else { return nil }
        return PRSummary(
            ref: PRRef(owner: repository.owner.login, repo: repository.name, number: number),
            title: title, author: author?.login ?? "ghost", authorAvatarURL: author?.avatarUrl,
            updatedAt: updatedAt ?? .distantPast, isDraft: isDraft ?? false,
            additions: additions ?? 0, deletions: deletions ?? 0,
            checks: CheckState(rawValue: commits?.nodes.first?.commit.statusCheckRollup?.state ?? "") ?? .none,
            reviewDecision: ReviewDecision(rawValue: reviewDecision ?? "") ?? .none,
            commentCount: comments?.totalCount ?? 0)
    }
}

struct CommentDTO: Decodable {
    var id: String
    var databaseId: Int?
    var body: String
    var createdAt: Date
    var state: String?
    var url: URL?
    var viewerDidAuthor: Bool?
    var author: ActorDTO?

    func toComment() -> ReviewComment {
        ReviewComment(id: id, databaseId: databaseId, body: body, author: author?.login ?? "ghost",
                      authorAvatarURL: author?.avatarUrl, createdAt: createdAt, isPending: state == "PENDING",
                      url: url, viewerDidAuthor: viewerDidAuthor ?? false)
    }
}

struct ThreadDTO: Decodable {
    struct Comments: Decodable { var nodes: [CommentDTO] }
    var id: String
    var path: String
    var line: Int?
    var startLine: Int?
    var originalLine: Int?
    var diffSide: String
    var isResolved: Bool
    var isOutdated: Bool
    var comments: Comments

    func toThread() -> ReviewThread {
        ReviewThread(id: id, path: path, line: line, startLine: startLine, originalLine: originalLine,
                     side: DiffSide(rawValue: diffSide) ?? .right, isResolved: isResolved, isOutdated: isOutdated,
                     comments: comments.nodes.map { $0.toComment() })
    }
}

struct FileDTO: Decodable {
    var path: String
    var additions: Int
    var deletions: Int
    var viewerViewedState: String
}

struct Nodes<T: Decodable>: Decodable { var nodes: [T] }

struct IssueCommentDTO: Decodable {
    var id: String
    var body: String
    var createdAt: Date
    var url: URL?
    var author: ActorDTO?
}

struct ReviewDTO: Decodable {
    var id: String
    var state: String
    var body: String
    var submittedAt: Date?
    var url: URL?
    var author: ActorDTO?
}

struct CommitNodeDTO: Decodable {
    struct Commit: Decodable {
        struct Author: Decodable { var name: String?; var user: ActorDTO? }
        struct Rollup: Decodable { var state: String }
        var oid: String
        var messageHeadline: String
        var committedDate: Date
        var author: Author?
        var statusCheckRollup: Rollup?
    }
    var commit: Commit
}

struct HeadCommitDTO: Decodable {
    struct Commit: Decodable {
        struct Rollup: Decodable { var contexts: Nodes<CheckContextDTO> }
        var statusCheckRollup: Rollup?
    }
    var commit: Commit
}

/// A CheckRun (Actions, apps) or a legacy StatusContext.
struct CheckContextDTO: Decodable {
    struct Suite: Decodable { struct App: Decodable { var name: String }; var app: App? }
    var __typename: String
    // CheckRun
    var name: String?
    var conclusion: String?
    var status: String?
    var detailsUrl: URL?
    var checkSuite: Suite?
    // StatusContext
    var context: String?
    var state: String?
    var targetUrl: URL?

    var check: PRMergeStatus.Check? {
        if let name {
            // A run without a conclusion is still queued or in progress.
            return .init(name: name, app: checkSuite?.app?.name, state: conclusion ?? "PENDING", url: detailsUrl)
        }
        if let context, let state {
            return .init(name: context, state: state == "EXPECTED" ? "PENDING" : state, url: targetUrl)
        }
        return nil
    }
}

struct HeadOidData: Decodable {
    struct Repo: Decodable { struct PR: Decodable { var headRefOid: String }; var pullRequest: PR? }
    var repository: Repo
}

struct AddCommentData: Decodable {
    struct Payload: Decodable { struct Edge: Decodable { var node: IssueCommentDTO }; var commentEdge: Edge }
    var addComment: Payload
}

struct Connection<T: Decodable>: Decodable {
    var pageInfo: PageInfoDTO
    var nodes: [T]
}

struct PRData: Decodable {
    struct Viewer: Decodable { var login: String }
    struct Repo: Decodable {
        var viewerPermission: String?
        var mergeCommitAllowed: Bool?
        var squashMergeAllowed: Bool?
        var rebaseMergeAllowed: Bool?
        var viewerDefaultMergeMethod: String?
        var pullRequest: PR?

        var allowedMergeMethods: [MergeMethod]? {
            guard mergeCommitAllowed != nil || squashMergeAllowed != nil || rebaseMergeAllowed != nil else { return nil }
            return [(MergeMethod.merge, mergeCommitAllowed), (.squash, squashMergeAllowed), (.rebase, rebaseMergeAllowed)]
                .filter { $0.1 == true }.map(\.0)
        }
    }
    struct PR: Decodable {
        struct HeadRepo: Decodable { var nameWithOwner: String; var sshUrl: String?; var url: String? }
        struct Reviews: Decodable { struct R: Decodable { var id: String; var viewerDidAuthor: Bool }; var nodes: [R] }
        var id: String
        var title: String
        var body: String
        var state: String
        var isDraft: Bool
        var baseRefName: String
        var headRefName: String
        var baseRefOid: String
        var headRefOid: String
        var maintainerCanModify: Bool
        var changedFiles: Int
        var additions: Int
        var deletions: Int
        var author: ActorDTO?
        var headRepository: HeadRepo?
        var files: Connection<FileDTO>?
        var reviewThreads: Connection<ThreadDTO>
        var reviews: Reviews
        // Optional: hand-written fixtures and older responses omit them.
        var comments: Nodes<IssueCommentDTO>?
        var allReviews: Nodes<ReviewDTO>?
        var commits: Nodes<CommitNodeDTO>?
        var mergeable: String?
        var mergeStateStatus: String?
        var reviewDecision: String?
        var viewerCanMergeAsAdmin: Bool?
        var headCommit: Nodes<HeadCommitDTO>?

        func status(repo: Repo) -> PRMergeStatus? {
            guard let mergeable, let mergeStateStatus else { return nil }
            let contexts = headCommit?.nodes.first?.commit.statusCheckRollup?.contexts.nodes ?? []
            return PRMergeStatus(mergeable: mergeable, mergeStateStatus: mergeStateStatus, reviewDecision: reviewDecision,
                                 checks: contexts.compactMap(\.check),
                                 allowedMergeMethods: repo.allowedMergeMethods,
                                 defaultMergeMethod: repo.viewerDefaultMergeMethod.flatMap(MergeMethod.init(rawValue:)),
                                 viewerCanMergeAsAdmin: viewerCanMergeAsAdmin.map { $0 || repo.viewerPermission == "ADMIN" })
        }

        /// Built from the newest 100 comments and reviews (`last: 100`): every single comment
        /// is a review too, so busy PRs pass 100 quickly.
        var conversation: PRConversation? {
            guard comments != nil || allReviews != nil || commits != nil else { return nil }
            return PRConversation(
                comments: (comments?.nodes ?? []).map {
                    .init(id: $0.id, author: $0.author?.login ?? "ghost", authorAvatarURL: $0.author?.avatarUrl,
                          body: $0.body, createdAt: $0.createdAt, url: $0.url)
                },
                // PENDING is the viewer's own unsubmitted draft (see `pendingReviewId`).
                reviews: (allReviews?.nodes ?? []).compactMap { r in
                    guard r.state != "PENDING", let at = r.submittedAt else { return nil }
                    return .init(id: r.id, author: r.author?.login ?? "ghost", authorAvatarURL: r.author?.avatarUrl,
                                 state: r.state, body: r.body, submittedAt: at, url: r.url)
                },
                commits: (commits?.nodes ?? []).map { n in
                    let c = n.commit
                    return .init(oid: c.oid, headline: c.messageHeadline,
                                 author: c.author?.user?.login ?? c.author?.name ?? "unknown",
                                 authorAvatarURL: c.author?.user?.avatarUrl, date: c.committedDate,
                                 checks: CheckState(rawValue: c.statusCheckRollup?.state ?? "") ?? .none)
                })
        }
    }
    var viewer: Viewer
    var repository: Repo
}

struct PRFilesData: Decodable {
    struct Repo: Decodable { struct PR: Decodable { var files: Connection<FileDTO>? }; var pullRequest: PR }
    var repository: Repo
}

struct PRThreadsData: Decodable {
    struct Repo: Decodable { struct PR: Decodable { var reviewThreads: Connection<ThreadDTO> }; var pullRequest: PR }
    var repository: Repo
}

struct AddReviewData: Decodable {
    struct Payload: Decodable { struct R: Decodable { var id: String }; var pullRequestReview: R }
    var addPullRequestReview: Payload
}

struct AddThreadData: Decodable {
    struct Payload: Decodable { var thread: ThreadDTO? }
    var addPullRequestReviewThread: Payload
}

struct ReplyData: Decodable {
    struct Payload: Decodable { var comment: CommentDTO }
    var addPullRequestReviewThreadReply: Payload
}

struct UpdateCommentData: Decodable {
    struct Payload: Decodable { var pullRequestReviewComment: CommentDTO }
    var updatePullRequestReviewComment: Payload
}

struct ResolveData: Decodable {
    struct Payload: Decodable { struct T: Decodable { var id: String; var isResolved: Bool }; var thread: T }
    var resolveReviewThread: Payload?
    var unresolveReviewThread: Payload?
}
