import Foundation
import ReviewBuddyCore

/// Git operations the stores need; `WorktreeManager` in production, fakes in tests.
public protocol WorktreeProviding: Sendable {
    func ensureWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree
    func fetchPR(repoDir: URL, pr: PRDetail) async throws
    func localCommits(_ wt: Worktree) async throws -> [CommitInfo]
    func remoteAhead(_ wt: Worktree) async throws -> Int
    func rebaseOntoRemote(_ wt: Worktree) async throws
    func push(_ wt: Worktree, pr: PRDetail) async throws
    func diff(repoDir: URL, base: String, head: String) async throws -> Data
    func diff(repoDir: URL, from: String, to: String, path: String) async throws -> Data
    func fileContents(repoDir: URL, sha: String, path: String) async throws -> Data
    func headSHA(_ wt: Worktree) async throws -> String
    func commitDiff(_ wt: Worktree, sha: String) async throws -> Data
    /// The PR's worktree if it already exists on disk (never clones or fetches).
    func ensureExistingWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree?
    func remove(_ wt: Worktree) async throws
}

extension WorktreeManager: WorktreeProviding {}

/// A running Claude conversation; `ClaudeSession` in production.
public protocol ClaudeSessionProtocol: AnyObject, Sendable {
    var output: AsyncStream<SessionOutput> { get }
    var turnActive: Bool { get set }
    func send(_ text: String) throws
    func interrupt()
    func terminate()
}

extension ClaudeSession: ClaudeSessionProtocol {}

/// Dependency container shared by all stores.
@MainActor
public final class Services {
    public let github: GitHubAPI
    public let store: LocalStore
    public var settings: AppSettings
    public let worktrees: WorktreeProviding?
    public let claudePath: String?
    public let makeSession: (ClaudeConfig) -> ClaudeSessionProtocol
    public let oneShot: ClaudeOneShotRunning

    public init(github: GitHubAPI, store: LocalStore, settings: AppSettings, worktrees: WorktreeProviding?,
                claudePath: String?, makeSession: @escaping (ClaudeConfig) -> ClaudeSessionProtocol = { ClaudeSession(config: $0) },
                oneShot: ClaudeOneShotRunning = ClaudeOneShot()) {
        self.github = github
        self.store = store
        self.settings = settings
        self.worktrees = worktrees
        self.claudePath = claudePath
        self.makeSession = makeSession
        self.oneShot = oneShot
    }

    public static func live() async -> Services {
        let store = LocalStore()
        let settings = await store.loadSettings()
        let worktrees = try? WorktreeManager(root: store.root)
        return Services(github: GitHubClient(), store: store, settings: settings, worktrees: worktrees,
                        claudePath: ToolLocator.find("claude"))
    }

    public func saveSettings() {
        let s = settings
        let store = self.store
        Task { try? await store.saveSettings(s) }
    }
}
