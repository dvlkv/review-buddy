import Foundation

public protocol HTTPTransport: Sendable {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

public struct URLSessionTransport: HTTPTransport {
    let session: URLSession
    public init(session: URLSession = .shared) { self.session = session }
    public func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw GitHubError.invalidResponse }
        return (data, http)
    }
}

public protocol TokenProvider: Sendable {
    func token() async throws -> String
}

public struct StaticTokenProvider: TokenProvider {
    let value: String
    public init(_ value: String) { self.value = value }
    public func token() async throws -> String { value }
}

/// Reads the token from `gh auth token`, falling back to a Keychain PAT.
public final class GHTokenProvider: TokenProvider, @unchecked Sendable {
    private let runner: CommandRunning
    private let lock = NSLock()
    private var cached: String?

    public init(runner: CommandRunning = ProcessRunner()) { self.runner = runner }

    public func token() async throws -> String {
        if let c = lock.withLock({ cached }) { return c }
        if let gh = ToolLocator.find("gh"),
           let r = try? await runner.run(gh, ["auth", "token"], cwd: nil, env: nil, stdin: nil), r.status == 0 {
            let t = r.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)
            if !t.isEmpty { lock.withLock { cached = t }; return t }
        }
        if let pat = Keychain.read(account: "github-pat"), !pat.isEmpty {
            lock.withLock { cached = pat }
            return pat
        }
        throw GitHubError.notAuthenticated
    }

    public func invalidate() { lock.withLock { cached = nil } }
}

public enum GitHubError: Error, LocalizedError, Equatable {
    case notAuthenticated
    case invalidResponse
    case http(Int, String)
    case graphQL([String])
    case diffTooLarge
    case notFound

    public var errorDescription: String? {
        switch self {
        case .notAuthenticated: "Not signed in to GitHub. Run `gh auth login` or add a token in Settings."
        case .invalidResponse: "GitHub returned an invalid response."
        case .http(let code, let msg): "GitHub error \(code): \(msg)"
        case .graphQL(let errs): errs.joined(separator: "\n")
        case .diffTooLarge: "This diff is too large for the GitHub API; using the local checkout instead."
        case .notFound: "Not found."
        }
    }
}

public enum InboxSection: String, CaseIterable, Codable, Sendable {
    case reviewRequested, mine
    public var query: String {
        switch self {
        case .reviewRequested: "is:pr is:open review-requested:@me archived:false sort:updated-desc"
        case .mine: "is:pr is:open author:@me archived:false sort:updated-desc"
        }
    }
    public var title: String {
        switch self {
        case .reviewRequested: "Review requested"
        case .mine: "My pull requests"
        }
    }
}

/// The GitHub surface the app needs. A protocol so stores can be tested with a fake.
public protocol GitHubAPI: Sendable {
    func search(_ query: String) async throws -> [PRSummary]
    func pullRequest(_ ref: PRRef) async throws -> PRDetail
    func diff(_ ref: PRRef) async throws -> Data
    func fileContents(owner: String, repo: String, path: String, ref: String) async throws -> Data
    func startReview(pullRequestId: String) async throws -> String
    func addThread(pullRequestId: String, reviewId: String, anchor: CodeAnchor, body: String) async throws -> ReviewThread
    func reply(threadId: String, reviewId: String?, body: String) async throws -> ReviewComment
    func updateComment(id: String, body: String) async throws -> ReviewComment
    func deleteComment(id: String) async throws
    func submitReview(reviewId: String, event: ReviewEvent, body: String) async throws
    func deleteReview(reviewId: String) async throws
    func setResolved(threadId: String, resolved: Bool) async throws -> Bool
    func setViewed(pullRequestId: String, path: String, viewed: Bool) async throws
}

public final class GitHubClient: GitHubAPI, @unchecked Sendable {
    let transport: HTTPTransport
    let tokens: TokenProvider
    let apiBase: URL

    public init(transport: HTTPTransport = URLSessionTransport(), tokens: TokenProvider = GHTokenProvider(),
                apiBase: URL = URL(string: "https://api.github.com")!) {
        self.transport = transport
        self.tokens = tokens
        self.apiBase = apiBase
    }

    // MARK: Transport helpers

    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()

    func request(_ url: URL, accept: String) async throws -> URLRequest {
        var r = URLRequest(url: url)
        r.setValue("Bearer \(try await tokens.token())", forHTTPHeaderField: "Authorization")
        r.setValue(accept, forHTTPHeaderField: "Accept")
        r.setValue("ReviewBuddy", forHTTPHeaderField: "User-Agent")
        r.setValue("2022-11-28", forHTTPHeaderField: "X-GitHub-Api-Version")
        r.timeoutInterval = 60
        return r
    }

    public func graphql<T: Decodable>(_ query: String, _ variables: [String: Any?] = [:], as: T.Type = T.self) async throws -> T {
        var r = try await request(apiBase.appendingPathComponent("graphql"), accept: "application/json")
        r.httpMethod = "POST"
        let vars = variables.compactMapValues { $0 }
        r.httpBody = try JSONSerialization.data(withJSONObject: ["query": query, "variables": vars])
        r.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, http) = try await transport.send(r)
        guard (200..<300).contains(http.statusCode) else {
            if http.statusCode == 401 { (tokens as? GHTokenProvider)?.invalidate(); throw GitHubError.notAuthenticated }
            throw GitHubError.http(http.statusCode, String(decoding: data.prefix(500), as: UTF8.self))
        }
        let envelope = try Self.decoder.decode(GraphQLEnvelope<T>.self, from: data)
        if let errors = envelope.errors, !errors.isEmpty {
            if envelope.data == nil || errors.contains(where: { $0.type == "NOT_FOUND" }) {
                if errors.contains(where: { $0.type == "NOT_FOUND" }) { throw GitHubError.notFound }
                throw GitHubError.graphQL(errors.map(\.message))
            }
        }
        guard let d = envelope.data else { throw GitHubError.graphQL(["Empty response"]) }
        return d
    }

    // MARK: Queries

    public func search(_ query: String) async throws -> [PRSummary] {
        let res: SearchData = try await graphql(Queries.search, ["q": query])
        return res.search.nodes.compactMap { $0.toSummary() }
    }

    public func pullRequest(_ ref: PRRef) async throws -> PRDetail {
        let vars: [String: Any?] = ["owner": ref.owner, "name": ref.repo, "number": ref.number]
        let first: PRData = try await graphql(Queries.pullRequest, vars)
        guard let pr = first.repository.pullRequest else { throw GitHubError.notFound }
        var files = pr.files?.nodes ?? []
        var threads = pr.reviewThreads.nodes
        // Large PRs: keep paging files and threads independently.
        var filesPage = pr.files?.pageInfo
        while let p = filesPage, p.hasNextPage {
            let next: PRFilesData = try await graphql(Queries.filesPage, vars.merging(["after": p.endCursor]) { $1 })
            files += next.repository.pullRequest.files?.nodes ?? []
            filesPage = next.repository.pullRequest.files?.pageInfo
        }
        var threadsPage = pr.reviewThreads.pageInfo
        while threadsPage.hasNextPage {
            let next: PRThreadsData = try await graphql(Queries.threadsPage, vars.merging(["after": threadsPage.endCursor]) { $1 })
            threads += next.repository.pullRequest.reviewThreads.nodes
            threadsPage = next.repository.pullRequest.reviewThreads.pageInfo
        }
        let canPush = ["ADMIN", "MAINTAIN", "WRITE"].contains(first.repository.viewerPermission ?? "")
        return PRDetail(
            ref: ref, nodeId: pr.id, title: pr.title, body: pr.body, author: pr.author?.login ?? "ghost",
            authorAvatarURL: pr.author?.avatarUrl, state: pr.state, isDraft: pr.isDraft,
            baseRefName: pr.baseRefName, headRefName: pr.headRefName, baseOid: pr.baseRefOid, headOid: pr.headRefOid,
            headRepoNameWithOwner: pr.headRepository?.nameWithOwner, headRepoSSHURL: pr.headRepository?.sshUrl,
            headRepoURL: pr.headRepository?.url, maintainerCanModify: pr.maintainerCanModify,
            viewerLogin: first.viewer.login, viewerCanPush: canPush,
            files: files.map { PRFileMeta(path: $0.path, additions: $0.additions, deletions: $0.deletions,
                                          viewed: ViewedState(rawValue: $0.viewerViewedState) ?? .unviewed) },
            threads: threads.map { $0.toThread() },
            pendingReviewId: pr.reviews.nodes.first(where: { $0.viewerDidAuthor })?.id,
            changedFiles: pr.changedFiles, additions: pr.additions, deletions: pr.deletions)
    }

    public func diff(_ ref: PRRef) async throws -> Data {
        let url = apiBase.appendingPathComponent("repos/\(ref.owner)/\(ref.repo)/pulls/\(ref.number)")
        let r = try await request(url, accept: "application/vnd.github.diff")
        let (data, http) = try await transport.send(r)
        switch http.statusCode {
        case 200: return data
        case 406, 422: throw GitHubError.diffTooLarge
        case 404: throw GitHubError.notFound
        default: throw GitHubError.http(http.statusCode, String(decoding: data.prefix(300), as: UTF8.self))
        }
    }

    public func fileContents(owner: String, repo: String, path: String, ref: String) async throws -> Data {
        var comps = URLComponents(url: apiBase.appendingPathComponent("repos/\(owner)/\(repo)/contents/\(path)"),
                                  resolvingAgainstBaseURL: false)!
        comps.queryItems = [URLQueryItem(name: "ref", value: ref)]
        let r = try await request(comps.url!, accept: "application/vnd.github.raw+json")
        let (data, http) = try await transport.send(r)
        guard http.statusCode == 200 else {
            throw http.statusCode == 404 ? GitHubError.notFound : GitHubError.http(http.statusCode, "")
        }
        return data
    }

    // MARK: Mutations

    public func startReview(pullRequestId: String) async throws -> String {
        let r: AddReviewData = try await graphql(Queries.addReview, ["pr": pullRequestId])
        return r.addPullRequestReview.pullRequestReview.id
    }

    public func addThread(pullRequestId: String, reviewId: String, anchor: CodeAnchor, body: String) async throws -> ReviewThread {
        let vars: [String: Any?] = [
            "pr": pullRequestId, "review": reviewId, "path": anchor.path, "body": body,
            "line": anchor.endLine, "side": anchor.side.rawValue,
            "startLine": anchor.isMultiline ? anchor.startLine : nil,
            "startSide": anchor.isMultiline ? anchor.side.rawValue : nil,
        ]
        let r: AddThreadData = try await graphql(Queries.addThread, vars)
        guard let t = r.addPullRequestReviewThread.thread else { throw GitHubError.graphQL(["Could not create thread"]) }
        return t.toThread()
    }

    public func reply(threadId: String, reviewId: String?, body: String) async throws -> ReviewComment {
        let r: ReplyData = try await graphql(Queries.reply, ["thread": threadId, "review": reviewId, "body": body])
        return r.addPullRequestReviewThreadReply.comment.toComment()
    }

    public func updateComment(id: String, body: String) async throws -> ReviewComment {
        let r: UpdateCommentData = try await graphql(Queries.updateComment, ["id": id, "body": body])
        return r.updatePullRequestReviewComment.pullRequestReviewComment.toComment()
    }

    public func deleteComment(id: String) async throws {
        let _: EmptyData = try await graphql(Queries.deleteComment, ["id": id])
    }

    public func submitReview(reviewId: String, event: ReviewEvent, body: String) async throws {
        let _: EmptyData = try await graphql(Queries.submitReview, ["review": reviewId, "event": event.rawValue, "body": body])
    }

    public func deleteReview(reviewId: String) async throws {
        let _: EmptyData = try await graphql(Queries.deleteReview, ["review": reviewId])
    }

    public func setResolved(threadId: String, resolved: Bool) async throws -> Bool {
        let r: ResolveData = try await graphql(resolved ? Queries.resolve : Queries.unresolve, ["id": threadId])
        return (r.resolveReviewThread ?? r.unresolveReviewThread)?.thread.isResolved ?? resolved
    }

    public func setViewed(pullRequestId: String, path: String, viewed: Bool) async throws {
        let _: EmptyData = try await graphql(viewed ? Queries.markViewed : Queries.unmarkViewed,
                                             ["pr": pullRequestId, "path": path])
    }
}
