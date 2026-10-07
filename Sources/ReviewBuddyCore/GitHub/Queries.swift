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
        viewerPermission
        pullRequest(number: $number) {
          id title body state isDraft baseRefName headRefName baseRefOid headRefOid maintainerCanModify
          changedFiles additions deletions
          author { login avatarUrl }
          headRepository { nameWithOwner sshUrl url }
          files(first: 100) { pageInfo { hasNextPage endCursor } nodes { path additions deletions viewerViewedState } }
          reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { \(threadFields) } }
          reviews(states: [PENDING], first: 5) { nodes { id viewerDidAuthor } }
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

struct Connection<T: Decodable>: Decodable {
    var pageInfo: PageInfoDTO
    var nodes: [T]
}

struct PRData: Decodable {
    struct Viewer: Decodable { var login: String }
    struct Repo: Decodable {
        var viewerPermission: String?
        var pullRequest: PR?
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
