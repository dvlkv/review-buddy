import Foundation

public struct Worktree: Sendable, Equatable {
    public var path: URL
    public var branch: String
    /// The git dir (bare clone or `.git` of the user's clone) the worktree belongs to.
    public var repoDir: URL
    /// Ref holding the PR head as last fetched from GitHub.
    public var remoteHeadRef: String
}

public struct CommitInfo: Sendable, Equatable, Identifiable, Hashable {
    public var id: String { sha }
    public var sha: String
    public var subject: String
    public var author: String
    public var date: Date?
    public var shortSHA: String { String(sha.prefix(7)) }
}

public enum WorktreeError: Error, LocalizedError, Equatable {
    case gitNotFound
    case cannotPush(String)
    case notAGitRepo(String)
    public var errorDescription: String? {
        switch self {
        case .gitNotFound: "git was not found. Install the Xcode command line tools."
        case .cannotPush(let why): why
        case .notAGitRepo(let p): "\(p) is not a git repository."
        }
    }
}

/// Owns the on-disk checkouts used by Discuss / Fix it / summaries.
///
/// Every PR gets its own worktree on branch `review-buddy/pr-<N>`, so the user's
/// working copy is never touched. The repo is either the user's mapped clone or a
/// blobless bare clone managed under Application Support.
public actor WorktreeManager {
    public nonisolated let root: URL
    let runner: CommandRunning
    let git: String
    /// Remote URL builder, overridable in tests (file:// remotes).
    let remoteURL: @Sendable (String) -> String
    let credentialArgs: [String]

    public init(root: URL, runner: CommandRunning = ProcessRunner(), gitPath: String? = nil,
                remoteURL: (@Sendable (String) -> String)? = nil) throws {
        self.root = root
        self.runner = runner
        guard let g = gitPath ?? ToolLocator.find("git") else { throw WorktreeError.gitNotFound }
        self.git = g
        self.remoteURL = remoteURL ?? { "https://github.com/\($0).git" }
        if let gh = ToolLocator.find("gh") {
            // Authenticate https fetch/push through the GitHub CLI.
            credentialArgs = ["-c", "credential.helper=", "-c", "credential.helper=!\(gh) auth git-credential"]
        } else {
            credentialArgs = []
        }
    }

    // MARK: Repos

    public func managedRepoDir(for nameWithOwner: String) -> URL {
        root.appendingPathComponent("repos/\(nameWithOwner).git", isDirectory: true)
    }

    /// Returns the git dir to use, cloning a managed bare repo if needed.
    public func ensureRepo(nameWithOwner: String, localClone: URL?) async throws -> URL {
        if let localClone {
            let r = try await runner.run(git, ["-C", localClone.path, "rev-parse", "--absolute-git-dir"], cwd: nil, env: nil, stdin: nil)
            guard r.status == 0 else { throw WorktreeError.notAGitRepo(localClone.path) }
            return URL(fileURLWithPath: r.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines))
        }
        let dir = managedRepoDir(for: nameWithOwner)
        if FileManager.default.fileExists(atPath: dir.appendingPathComponent("HEAD").path) { return dir }
        try FileManager.default.createDirectory(at: dir.deletingLastPathComponent(), withIntermediateDirectories: true)
        try await runner.check(git, credentialArgs + ["clone", "--bare", "--filter=blob:none", "--quiet",
                                                      remoteURL(nameWithOwner), dir.path])
        return dir
    }

    func worktreePath(repoDir: URL, ref: PRRef, isManaged: Bool) -> URL {
        if isManaged {
            return root.appendingPathComponent("worktrees/\(ref.owner)/\(ref.repo)/pr-\(ref.number)", isDirectory: true)
        }
        return repoDir.appendingPathComponent("review-buddy-worktrees/pr-\(ref.number)", isDirectory: true)
    }

    // MARK: Worktrees

    /// Fetches the PR head and base and creates (or updates) its worktree.
    public func ensureWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree {
        let repoDir = try await ensureRepo(nameWithOwner: pr.ref.nameWithOwner, localClone: localClone)
        let n = pr.ref.number
        // Not `refs/review-buddy/pr-N`: that would make the branch name `review-buddy/pr-N` ambiguous.
        let headRef = "refs/review-buddy/head-\(n)"
        try await fetchPR(repoDir: repoDir, pr: pr)
        let path = worktreePath(repoDir: repoDir, ref: pr.ref, isManaged: localClone == nil)
        let branch = "review-buddy/pr-\(n)"
        let wt = Worktree(path: path, branch: branch, repoDir: repoDir, remoteHeadRef: headRef)
        if FileManager.default.fileExists(atPath: path.appendingPathComponent(".git").path) {
            // Fast-forward when there is nothing local to lose.
            if try await localCommits(wt).isEmpty, try await !isDirty(wt) {
                _ = try await runner.run(git, ["-C", path.path, "merge", "--ff-only", "--quiet", headRef], cwd: nil, env: nil, stdin: nil)
            }
        } else {
            try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            _ = try? await runner.run(git, ["--git-dir", repoDir.path, "worktree", "prune"], cwd: nil, env: nil, stdin: nil)
            try await runner.check(git, ["--git-dir", repoDir.path, "worktree", "add", "--quiet", "-B", branch, path.path, headRef])
        }
        return wt
    }

    /// The PR's worktree if one already exists; never clones or fetches.
    public func ensureExistingWorktree(for pr: PRDetail, localClone: URL?) async throws -> Worktree? {
        let repoDir: URL
        if let localClone {
            repoDir = try await ensureRepo(nameWithOwner: pr.ref.nameWithOwner, localClone: localClone)
        } else {
            repoDir = managedRepoDir(for: pr.ref.nameWithOwner)
            guard FileManager.default.fileExists(atPath: repoDir.appendingPathComponent("HEAD").path) else { return nil }
        }
        let path = worktreePath(repoDir: repoDir, ref: pr.ref, isManaged: localClone == nil)
        guard FileManager.default.fileExists(atPath: path.appendingPathComponent(".git").path) else { return nil }
        return Worktree(path: path, branch: "review-buddy/pr-\(pr.ref.number)", repoDir: repoDir,
                        remoteHeadRef: "refs/review-buddy/head-\(pr.ref.number)")
    }

    /// Fetches the PR head and base. The head comes from the head repository's branch:
    /// GitHub updates `refs/pull/N/head` asynchronously, so right after a push it can still
    /// point at the old commit. Falls back to the pull ref when the branch is gone.
    public func fetchPR(repoDir: URL, pr: PRDetail) async throws {
        let n = pr.ref.number
        try await runner.check(git, credentialArgs + [
            "--git-dir", repoDir.path, "fetch", "--quiet", "--no-tags", remoteURL(pr.ref.nameWithOwner),
            "+refs/heads/\(pr.baseRefName):refs/review-buddy/base-\(n)",
        ])
        if let headRepo = pr.headRepoNameWithOwner {
            let r = try await runner.run(git, credentialArgs + [
                "--git-dir", repoDir.path, "fetch", "--quiet", "--no-tags", remoteURL(headRepo),
                "+refs/heads/\(pr.headRefName):refs/review-buddy/head-\(n)",
            ], cwd: nil, env: nil, stdin: nil)
            if r.status == 0 { return }
        }
        try await runner.check(git, credentialArgs + [
            "--git-dir", repoDir.path, "fetch", "--quiet", "--no-tags", remoteURL(pr.ref.nameWithOwner),
            "+refs/pull/\(n)/head:refs/review-buddy/head-\(n)",
        ])
    }

    public func isDirty(_ wt: Worktree) async throws -> Bool {
        let r = try await runner.check(git, ["-C", wt.path.path, "status", "--porcelain"])
        return !r.stdout.isEmpty
    }

    /// Commits on the worktree branch that are not in the PR head on GitHub.
    public func localCommits(_ wt: Worktree) async throws -> [CommitInfo] {
        let r = try await runner.check(git, ["-C", wt.path.path, "log", "--format=%H%x1f%s%x1f%an%x1f%aI",
                                             "\(wt.remoteHeadRef)..HEAD"])
        return Self.parseLog(r.stdoutString)
    }

    /// Commits on the remote PR head that the worktree does not have (PR moved on).
    public func remoteAhead(_ wt: Worktree) async throws -> Int {
        let r = try await runner.check(git, ["-C", wt.path.path, "rev-list", "--count", "HEAD..\(wt.remoteHeadRef)"])
        return Int(r.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
    }

    public func headSHA(_ wt: Worktree) async throws -> String {
        try await runner.check(git, ["-C", wt.path.path, "rev-parse", "HEAD"]).stdoutString
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func parseLog(_ s: String) -> [CommitInfo] {
        s.split(separator: "\n").compactMap { line in
            let f = line.split(separator: "\u{1F}", omittingEmptySubsequences: false).map(String.init)
            guard f.count >= 4 else { return nil }
            return CommitInfo(sha: f[0], subject: f[1], author: f[2], date: ISO8601DateFormatter().date(from: f[3]))
        }
    }

    /// Rebases local fix commits on top of the updated PR head.
    public func rebaseOntoRemote(_ wt: Worktree) async throws {
        do {
            try await runner.check(git, ["-C", wt.path.path, "rebase", "--quiet", wt.remoteHeadRef])
        } catch {
            _ = try? await runner.run(git, ["-C", wt.path.path, "rebase", "--abort"], cwd: nil, env: nil, stdin: nil)
            throw error
        }
    }

    /// Pushes the worktree HEAD to the PR head branch. Never force-pushes.
    public func push(_ wt: Worktree, pr: PRDetail) async throws {
        guard pr.canPushFixes else {
            throw WorktreeError.cannotPush(pr.isFromFork
                ? "This PR is from a fork and the author did not allow edits from maintainers."
                : "You don't have push access to \(pr.ref.nameWithOwner).")
        }
        let target = remoteURL(pr.headRepoNameWithOwner ?? pr.ref.nameWithOwner)
        try await runner.check(git, credentialArgs + ["-C", wt.path.path, "push", "--quiet", target,
                                                      "HEAD:refs/heads/\(pr.headRefName)"])
        // Our view of the remote head is now our HEAD.
        try await runner.check(git, ["-C", wt.path.path, "update-ref", wt.remoteHeadRef, "HEAD"])
    }

    public func remove(_ wt: Worktree) async throws {
        try await runner.check(git, ["--git-dir", wt.repoDir.path, "worktree", "remove", "--force", wt.path.path])
        _ = try? await runner.run(git, ["--git-dir", wt.repoDir.path, "branch", "-D", wt.branch], cwd: nil, env: nil, stdin: nil)
    }

    // MARK: Content

    /// Three-dot diff (merge-base), the same thing GitHub shows. No size limits.
    public func diff(repoDir: URL, base: String, head: String) async throws -> Data {
        try await runner.check(git, ["--git-dir", repoDir.path, "diff", "--no-color", "--no-ext-diff", "-M",
                                     "\(base)...\(head)"]).stdout
    }

    /// Diff between two commits for one path — used to remap anchors.
    public func diff(repoDir: URL, from: String, to: String, path: String) async throws -> Data {
        try await runner.check(git, ["--git-dir", repoDir.path, "diff", "--no-color", "--no-ext-diff", "-U0",
                                     from, to, "--", path]).stdout
    }

    public func fileContents(repoDir: URL, sha: String, path: String) async throws -> Data {
        try await runner.check(git, ["--git-dir", repoDir.path, "show", "\(sha):\(path)"]).stdout
    }

    public func commitDiff(_ wt: Worktree, sha: String) async throws -> Data {
        try await runner.check(git, ["-C", wt.path.path, "show", "--format=", "--no-color", sha]).stdout
    }
}
