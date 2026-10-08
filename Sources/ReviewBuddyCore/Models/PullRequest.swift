import Foundation

/// Identifies a pull request: `owner/repo#number`.
public struct PRRef: Hashable, Codable, Sendable, CustomStringConvertible {
    public var owner: String
    public var repo: String
    public var number: Int

    public init(owner: String, repo: String, number: Int) {
        self.owner = owner; self.repo = repo; self.number = number
    }

    public var nameWithOwner: String { "\(owner)/\(repo)" }
    public var description: String { "\(owner)/\(repo)#\(number)" }
    public var webURL: URL { URL(string: "https://github.com/\(owner)/\(repo)/pull/\(number)")! }
    /// Stable filesystem-safe key.
    public var storageKey: String { "\(owner)__\(repo)__\(number)" }

    /// Accepts `https://github.com/o/r/pull/12[/files...]`, `o/r#12`, `github.com/o/r/pull/12`.
    public init?(string raw: String) {
        let s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if let hash = s.firstIndex(of: "#"), !s.contains("/pull/") {
            let repoPart = s[..<hash].split(separator: "/")
            guard repoPart.count == 2, let n = Int(s[s.index(after: hash)...]) else { return nil }
            self.init(owner: String(repoPart[0]), repo: String(repoPart[1]), number: n)
            return
        }
        var path = s
        for prefix in ["https://", "http://"] where path.hasPrefix(prefix) { path.removeFirst(prefix.count) }
        if path.hasPrefix("www.") { path.removeFirst(4) }
        guard path.hasPrefix("github.com/") else { return nil }
        let parts = path.dropFirst("github.com/".count).split(separator: "/")
        guard parts.count >= 4, parts[2] == "pull" || parts[2] == "pulls",
              let n = Int(parts[3].prefix { $0.isNumber }) else { return nil }
        self.init(owner: String(parts[0]), repo: String(parts[1]), number: n)
    }
}

public enum CheckState: String, Codable, Sendable {
    case success = "SUCCESS", failure = "FAILURE", pending = "PENDING", error = "ERROR", expected = "EXPECTED", none
}

public enum ReviewDecision: String, Codable, Sendable {
    case approved = "APPROVED", changesRequested = "CHANGES_REQUESTED", reviewRequired = "REVIEW_REQUIRED", none
}

/// A row in the inbox sidebar.
public struct PRSummary: Identifiable, Hashable, Codable, Sendable {
    public var id: String { ref.description }
    public var ref: PRRef
    public var title: String
    public var author: String
    public var authorAvatarURL: URL?
    public var updatedAt: Date
    public var isDraft: Bool
    public var additions: Int
    public var deletions: Int
    public var checks: CheckState
    public var reviewDecision: ReviewDecision
    public var commentCount: Int

    public init(ref: PRRef, title: String, author: String, authorAvatarURL: URL? = nil, updatedAt: Date,
                isDraft: Bool = false, additions: Int = 0, deletions: Int = 0, checks: CheckState = .none,
                reviewDecision: ReviewDecision = .none, commentCount: Int = 0) {
        self.ref = ref; self.title = title; self.author = author; self.authorAvatarURL = authorAvatarURL
        self.updatedAt = updatedAt; self.isDraft = isDraft; self.additions = additions; self.deletions = deletions
        self.checks = checks; self.reviewDecision = reviewDecision; self.commentCount = commentCount
    }
}

public enum ViewedState: String, Codable, Sendable {
    case viewed = "VIEWED", unviewed = "UNVIEWED", dismissed = "DISMISSED"
}

/// The PR's conversation (as on GitHub's Conversation and Commits tabs). Inline review
/// threads live in `PRDetail.threads`.
public struct PRConversation: Hashable, Codable, Sendable {
    public struct Comment: Hashable, Codable, Sendable, Identifiable {
        public var id: String
        public var author: String
        public var authorAvatarURL: URL?
        public var body: String
        public var createdAt: Date
        public var url: URL?
        public init(id: String, author: String, authorAvatarURL: URL? = nil, body: String, createdAt: Date, url: URL? = nil) {
            self.id = id; self.author = author; self.authorAvatarURL = authorAvatarURL; self.body = body
            self.createdAt = createdAt; self.url = url
        }
    }
    public struct Review: Hashable, Codable, Sendable, Identifiable {
        public var id: String
        public var author: String
        public var authorAvatarURL: URL?
        /// APPROVED, CHANGES_REQUESTED, COMMENTED or DISMISSED.
        public var state: String
        public var body: String
        public var submittedAt: Date
        public var url: URL?
        public init(id: String, author: String, authorAvatarURL: URL? = nil, state: String, body: String,
                    submittedAt: Date, url: URL? = nil) {
            self.id = id; self.author = author; self.authorAvatarURL = authorAvatarURL; self.state = state
            self.body = body; self.submittedAt = submittedAt; self.url = url
        }
    }
    public struct Commit: Hashable, Codable, Sendable, Identifiable {
        public var oid: String
        public var headline: String
        public var author: String
        public var authorAvatarURL: URL?
        public var date: Date
        public var checks: CheckState
        public var id: String { oid }
        public init(oid: String, headline: String, author: String, authorAvatarURL: URL? = nil, date: Date, checks: CheckState = .none) {
            self.oid = oid; self.headline = headline; self.author = author; self.authorAvatarURL = authorAvatarURL
            self.date = date; self.checks = checks
        }
    }

    public var comments: [Comment]
    public var reviews: [Review]
    public var commits: [Commit]
    public init(comments: [Comment] = [], reviews: [Review] = [], commits: [Commit] = []) {
        self.comments = comments; self.reviews = reviews; self.commits = commits
    }
}

/// What GitHub's merge box shows: can it merge, what blocks it, and the checks.
public struct PRMergeStatus: Hashable, Codable, Sendable {
    public struct Check: Hashable, Codable, Sendable, Identifiable {
        public var name: String
        /// The app or service that reported it (e.g. "GitHub Actions").
        public var app: String?
        /// SUCCESS, FAILURE, ERROR, CANCELLED, TIMED_OUT, ACTION_REQUIRED, NEUTRAL, SKIPPED, STALE or PENDING.
        public var state: String
        public var url: URL?
        public var id: String { (app ?? "") + "/" + name }
        public init(name: String, app: String? = nil, state: String, url: URL? = nil) {
            self.name = name; self.app = app; self.state = state; self.url = url
        }
        public var isSuccess: Bool { ["SUCCESS", "NEUTRAL", "SKIPPED"].contains(state) }
        public var isPending: Bool { state == "PENDING" }
        public var isFailure: Bool { !isSuccess && !isPending }
    }

    /// MERGEABLE, CONFLICTING or UNKNOWN.
    public var mergeable: String
    /// CLEAN, BLOCKED, BEHIND, DIRTY, UNSTABLE, DRAFT, HAS_HOOKS or UNKNOWN.
    public var mergeStateStatus: String
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED, or nil when reviews aren't required.
    public var reviewDecision: String?
    public var checks: [Check]
    /// Merge methods the repository allows; nil in older caches and fixtures (treated as all).
    public var allowedMergeMethods: [MergeMethod]?
    /// The method GitHub preselects for this viewer.
    public var defaultMergeMethod: MergeMethod?
    /// The viewer may merge past branch protection (GitHub's "bypass rules" checkbox). GitHub's own
    /// `viewerCanMergeAsAdmin` misses ruleset/admin bypasses, so repo admins count too; GitHub
    /// still refuses the merge if a rule they can't bypass is failing.
    public var viewerCanMergeAsAdmin: Bool?
    public init(mergeable: String, mergeStateStatus: String, reviewDecision: String? = nil, checks: [Check] = [],
                allowedMergeMethods: [MergeMethod]? = nil, defaultMergeMethod: MergeMethod? = nil,
                viewerCanMergeAsAdmin: Bool? = nil) {
        self.mergeable = mergeable; self.mergeStateStatus = mergeStateStatus
        self.reviewDecision = reviewDecision; self.checks = checks
        self.allowedMergeMethods = allowedMergeMethods; self.defaultMergeMethod = defaultMergeMethod
        self.viewerCanMergeAsAdmin = viewerCanMergeAsAdmin
    }

    /// Whether GitHub would let the viewer press Merge now.
    public var canMerge: Bool {
        guard mergeable != "CONFLICTING" else { return false }
        switch mergeStateStatus {
        case "CLEAN", "HAS_HOOKS", "UNSTABLE", "BEHIND": return true
        default: return false
        }
    }

    /// Blocked by requirements, but the viewer can merge anyway by ticking "bypass rules".
    public var canBypass: Bool {
        mergeable != "CONFLICTING" && mergeStateStatus == "BLOCKED" && viewerCanMergeAsAdmin == true
    }
}

public enum MergeMethod: String, Hashable, Codable, Sendable, CaseIterable {
    case merge = "MERGE", squash = "SQUASH", rebase = "REBASE"
    public var title: String {
        switch self {
        case .merge: "Create a merge commit"
        case .squash: "Squash and merge"
        case .rebase: "Rebase and merge"
        }
    }
    public var buttonTitle: String {
        switch self {
        case .merge: "Merge pull request"
        case .squash: "Squash and merge"
        case .rebase: "Rebase and merge"
        }
    }
}

public struct PRFileMeta: Hashable, Codable, Sendable {
    public var path: String
    public var additions: Int
    public var deletions: Int
    public var viewed: ViewedState
    public init(path: String, additions: Int, deletions: Int, viewed: ViewedState) {
        self.path = path; self.additions = additions; self.deletions = deletions; self.viewed = viewed
    }
}

public struct ReviewComment: Identifiable, Hashable, Codable, Sendable {
    public var id: String
    public var databaseId: Int?
    public var body: String
    public var author: String
    public var authorAvatarURL: URL?
    public var createdAt: Date
    public var isPending: Bool
    public var url: URL?
    public var viewerDidAuthor: Bool

    public init(id: String, databaseId: Int? = nil, body: String, author: String, authorAvatarURL: URL? = nil,
                createdAt: Date, isPending: Bool = false, url: URL? = nil, viewerDidAuthor: Bool = false) {
        self.id = id; self.databaseId = databaseId; self.body = body; self.author = author
        self.authorAvatarURL = authorAvatarURL; self.createdAt = createdAt; self.isPending = isPending
        self.url = url; self.viewerDidAuthor = viewerDidAuthor
    }
}

public struct ReviewThread: Identifiable, Hashable, Codable, Sendable {
    public var id: String
    public var path: String
    /// Current line on the PR head (nil when outdated).
    public var line: Int?
    public var startLine: Int?
    public var originalLine: Int?
    public var side: DiffSide
    public var isResolved: Bool
    public var isOutdated: Bool
    public var comments: [ReviewComment]

    public init(id: String, path: String, line: Int?, startLine: Int? = nil, originalLine: Int? = nil,
                side: DiffSide = .right, isResolved: Bool = false, isOutdated: Bool = false, comments: [ReviewComment]) {
        self.id = id; self.path = path; self.line = line; self.startLine = startLine
        self.originalLine = originalLine; self.side = side; self.isResolved = isResolved
        self.isOutdated = isOutdated; self.comments = comments
    }

    /// The line the thread is drawn under.
    public var anchorLine: Int { line ?? originalLine ?? 0 }
    public var isPending: Bool { comments.allSatisfy(\.isPending) && !comments.isEmpty }
}

public struct PRDetail: Sendable, Codable, Hashable {
    public var ref: PRRef
    public var nodeId: String
    public var title: String
    public var body: String
    public var author: String
    public var authorAvatarURL: URL?
    public var state: String
    public var isDraft: Bool
    public var baseRefName: String
    public var headRefName: String
    public var baseOid: String
    public var headOid: String
    public var headRepoNameWithOwner: String?
    public var headRepoSSHURL: String?
    public var headRepoURL: String?
    public var maintainerCanModify: Bool
    public var viewerLogin: String
    public var viewerCanPush: Bool
    public var files: [PRFileMeta]
    public var threads: [ReviewThread]
    /// The viewer's pending (draft) review, if any.
    public var pendingReviewId: String?
    public var changedFiles: Int
    public var additions: Int
    public var deletions: Int
    /// PR comments, submitted reviews and commits. Optional so details cached before it
    /// existed still decode.
    public var conversation: PRConversation?
    /// Mergeability, review decision and the head commit's checks. Optional like `conversation`.
    public var status: PRMergeStatus?

    public init(ref: PRRef, nodeId: String, title: String, body: String, author: String, authorAvatarURL: URL? = nil,
                state: String = "OPEN", isDraft: Bool = false, baseRefName: String, headRefName: String,
                baseOid: String, headOid: String, headRepoNameWithOwner: String? = nil, headRepoSSHURL: String? = nil,
                headRepoURL: String? = nil, maintainerCanModify: Bool = false, viewerLogin: String = "",
                viewerCanPush: Bool = false, files: [PRFileMeta] = [], threads: [ReviewThread] = [],
                pendingReviewId: String? = nil, changedFiles: Int = 0, additions: Int = 0, deletions: Int = 0,
                conversation: PRConversation? = nil, status: PRMergeStatus? = nil) {
        self.ref = ref; self.nodeId = nodeId; self.title = title; self.body = body; self.author = author
        self.authorAvatarURL = authorAvatarURL; self.state = state; self.isDraft = isDraft
        self.baseRefName = baseRefName; self.headRefName = headRefName; self.baseOid = baseOid; self.headOid = headOid
        self.headRepoNameWithOwner = headRepoNameWithOwner; self.headRepoSSHURL = headRepoSSHURL
        self.headRepoURL = headRepoURL; self.maintainerCanModify = maintainerCanModify; self.viewerLogin = viewerLogin
        self.viewerCanPush = viewerCanPush; self.files = files; self.threads = threads
        self.pendingReviewId = pendingReviewId; self.changedFiles = changedFiles
        self.additions = additions; self.deletions = deletions
        self.conversation = conversation
        self.status = status
    }

    public var isFromFork: Bool {
        guard let head = headRepoNameWithOwner else { return true }
        return head.lowercased() != ref.nameWithOwner.lowercased()
    }

    /// Whether the app may push fixes to the PR head branch.
    public var canPushFixes: Bool {
        if isFromFork { return maintainerCanModify || viewerLogin == author }
        return viewerCanPush || viewerLogin == author
    }
}

public enum ReviewEvent: String, Codable, Sendable, CaseIterable {
    case comment = "COMMENT", approve = "APPROVE", requestChanges = "REQUEST_CHANGES"
    public var title: String {
        switch self {
        case .comment: "Comment"
        case .approve: "Approve"
        case .requestChanges: "Request changes"
        }
    }
}

/// A line range on one side of a file's diff.
public struct CodeAnchor: Hashable, Codable, Sendable {
    public var path: String
    public var side: DiffSide
    public var startLine: Int
    public var endLine: Int
    public init(path: String, side: DiffSide, startLine: Int, endLine: Int) {
        self.path = path; self.side = side
        self.startLine = min(startLine, endLine); self.endLine = max(startLine, endLine)
    }
    public var isMultiline: Bool { startLine != endLine }
    public var label: String {
        let file = path.split(separator: "/").last.map(String.init) ?? path
        return isMultiline ? "\(file) L\(startLine)-\(endLine)" : "\(file) L\(startLine)"
    }
}
