import Foundation
import Testing
@testable import ReviewBuddyCore

/// Isolates git from the user's global config (signing, hooks, identity).
struct EnvRunner: CommandRunning {
    let env: [String: String]
    func run(_ executable: String, _ args: [String], cwd: URL?, env extra: [String: String]?, stdin: Data?) async throws -> ProcessResult {
        try await ProcessRunner().run(executable, args, cwd: cwd, env: env.merging(extra ?? [:]) { $1 }, stdin: stdin)
    }
}

/// Real-git sandbox: an origin repo with `main`, a `feature` branch and `refs/pull/1/head`.
struct GitSandbox {
    let dir: URL
    let origin: URL
    let runner = ProcessRunner()
    static let env = [
        "GIT_AUTHOR_NAME": "Test", "GIT_AUTHOR_EMAIL": "t@example.com",
        "GIT_COMMITTER_NAME": "Test", "GIT_COMMITTER_EMAIL": "t@example.com",
        "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
    ]

    init() async throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("rb-git-\(UUID().uuidString)")
        origin = dir.appendingPathComponent("origin")
        try FileManager.default.createDirectory(at: origin, withIntermediateDirectories: true)
        try await git(origin, "init", "-q", "-b", "main")
        try await git(origin, "config", "receive.denyCurrentBranch", "updateInstead")
        try write(origin, "a.txt", "one\ntwo\nthree\n")
        try await git(origin, "add", ".")
        try await git(origin, "commit", "-q", "-m", "base")
        try await git(origin, "checkout", "-q", "-b", "feature")
        try write(origin, "a.txt", "one\nTWO\nthree\n")
        try write(origin, "b.txt", "new file\n")
        try await git(origin, "add", ".")
        try await git(origin, "commit", "-q", "-m", "feature work")
        try await git(origin, "update-ref", "refs/pull/1/head", "HEAD")
        try await git(origin, "checkout", "-q", "main")
    }

    @discardableResult
    func git(_ cwd: URL, _ args: String...) async throws -> String {
        try await runner.check("/usr/bin/git", ["-C", cwd.path] + args, env: Self.env).stdoutString
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func write(_ cwd: URL, _ name: String, _ text: String) throws {
        try text.write(to: cwd.appendingPathComponent(name), atomically: true, encoding: .utf8)
    }

    func manager() throws -> WorktreeManager {
        let o = origin.path
        return try WorktreeManager(root: dir.appendingPathComponent("support"), runner: EnvRunner(env: Self.env),
                                   gitPath: "/usr/bin/git", remoteURL: { _ in o })
    }

    func pr(head: String, canPush: Bool = true) -> PRDetail {
        PRDetail(ref: PRRef(owner: "o", repo: "r", number: 1), nodeId: "PR", title: "T", body: "", author: "alice",
                 baseRefName: "main", headRefName: "feature", baseOid: "", headOid: head,
                 headRepoNameWithOwner: "o/r", maintainerCanModify: false, viewerLogin: "me", viewerCanPush: canPush)
    }

    func cleanup() { try? FileManager.default.removeItem(at: dir) }
}

@Suite(.serialized) struct WorktreeManagerTests {
    @Test func managedCloneAndWorktree() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let repo = try await m.ensureRepo(nameWithOwner: "o/r", localClone: nil)
        #expect(await repo == m.managedRepoDir(for: "o/r"))
        #expect(FileManager.default.fileExists(atPath: repo.appendingPathComponent("HEAD").path))
        // Idempotent.
        #expect(try await m.ensureRepo(nameWithOwner: "o/r", localClone: nil) == repo)

        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        #expect(wt.branch == "review-buddy/pr-1")
        #expect(wt.path.path.hasSuffix("worktrees/o/r/pr-1"))
        #expect(try await m.headSHA(wt) == head)
        #expect(try await sb.git(wt.path, "rev-parse", "--abbrev-ref", "HEAD") == "review-buddy/pr-1")
        #expect(try String(contentsOf: wt.path.appendingPathComponent("b.txt"), encoding: .utf8) == "new file\n")
        #expect(try await m.localCommits(wt).isEmpty)
        #expect(try await !m.isDirty(wt))

        // Diff base...head, same as GitHub.
        let diff = UnifiedDiffParser.parse(data: try await m.diff(repoDir: repo, base: "refs/review-buddy/base-1", head: "refs/review-buddy/head-1"))
        #expect(diff.files.map(\.path).sorted() == ["a.txt", "b.txt"])
        #expect(diff.files.first { $0.path == "b.txt" }?.status == .added)

        let content = try await m.fileContents(repoDir: repo, sha: head, path: "a.txt")
        #expect(String(decoding: content, as: UTF8.self) == "one\nTWO\nthree\n")

        let base = try await sb.git(sb.origin, "rev-parse", "main")
        let fileDiff = UnifiedDiffParser.parse(data: try await m.diff(repoDir: repo, from: base, to: head, path: "a.txt"))
        #expect(LineMapper(file: fileDiff.files.first).map(oldLine: 2) == .changed)

        try await m.remove(wt)
        #expect(!FileManager.default.fileExists(atPath: wt.path.path))
    }

    @Test func ensureExistingWorktreeNeverClones() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        // Nothing on disk yet: nil, and no clone was made.
        #expect(try await m.ensureExistingWorktree(for: sb.pr(head: head), localClone: nil) == nil)
        #expect(!FileManager.default.fileExists(atPath: await m.managedRepoDir(for: "o/r").path))
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        #expect(try await m.ensureExistingWorktree(for: sb.pr(head: head), localClone: nil) == wt)
        try await m.remove(wt)
        #expect(try await m.ensureExistingWorktree(for: sb.pr(head: head), localClone: nil) == nil)
    }

    @Test func localCloneWorktreeLivesInsideGitDir() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let clone = sb.dir.appendingPathComponent("clone")
        try await sb.runner.check("/usr/bin/git", ["clone", "-q", sb.origin.path, clone.path], env: GitSandbox.env)
        let m = try sb.manager()
        let repo = try await m.ensureRepo(nameWithOwner: "o/r", localClone: clone)
        #expect(repo.standardizedFileURL.resolvingSymlinksInPath().path
                == clone.appendingPathComponent(".git").standardizedFileURL.resolvingSymlinksInPath().path)
        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: clone)
        #expect(wt.path.path.contains(".git/review-buddy-worktrees/pr-1"))
        #expect(try await m.headSHA(wt) == head)
        // User's working copy untouched.
        #expect(try await sb.git(clone, "rev-parse", "--abbrev-ref", "HEAD") == "main")
    }

    @Test func notARepo() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let bogus = sb.dir.appendingPathComponent("nothing")
        try FileManager.default.createDirectory(at: bogus, withIntermediateDirectories: true)
        await #expect(throws: WorktreeError.notAGitRepo(bogus.path)) {
            _ = try await m.ensureRepo(nameWithOwner: "o/r", localClone: bogus)
        }
    }

    @Test func localCommitsRemoteAheadFastForwardAndRebase() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        var head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)

        // PR moves on remotely; no local commits → re-ensure fast-forwards.
        try await sb.git(sb.origin, "checkout", "-q", "feature")
        try sb.write(sb.origin, "c.txt", "c\n")
        try await sb.git(sb.origin, "add", ".")
        try await sb.git(sb.origin, "commit", "-q", "-m", "remote 2")
        head = try await sb.git(sb.origin, "rev-parse", "HEAD")
        try await sb.git(sb.origin, "update-ref", "refs/pull/1/head", head)
        try await m.fetchPR(repoDir: wt.repoDir, pr: sb.pr(head: head))
        #expect(try await m.remoteAhead(wt) == 1)
        _ = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        #expect(try await m.headSHA(wt) == head)
        #expect(try await m.remoteAhead(wt) == 0)

        // Local fix commit.
        try sb.write(wt.path, "fix.txt", "fixed\n")
        #expect(try await m.isDirty(wt))
        try await sb.git(wt.path, "add", ".")
        try await sb.git(wt.path, "commit", "-q", "-m", "Fix the thing")
        let local = try await m.localCommits(wt)
        #expect(local.count == 1)
        #expect(local[0].subject == "Fix the thing" && local[0].author == "Test" && local[0].date != nil)
        #expect(local[0].shortSHA.count == 7)
        let show = try await m.commitDiff(wt, sha: local[0].sha)
        #expect(UnifiedDiffParser.parse(data: show).files.map(\.path) == ["fix.txt"])

        // Remote moves again; re-ensure must NOT fast-forward over local work.
        try sb.write(sb.origin, "d.txt", "d\n")
        try await sb.git(sb.origin, "add", ".")
        try await sb.git(sb.origin, "commit", "-q", "-m", "remote 3")
        head = try await sb.git(sb.origin, "rev-parse", "HEAD")
        try await sb.git(sb.origin, "update-ref", "refs/pull/1/head", head)
        try await sb.git(sb.origin, "checkout", "-q", "main")
        _ = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        #expect(try await m.remoteAhead(wt) == 1)
        #expect(try await m.localCommits(wt).count == 1)

        try await m.rebaseOntoRemote(wt)
        #expect(try await m.remoteAhead(wt) == 0)
        let after = try await m.localCommits(wt)
        #expect(after.count == 1 && after[0].subject == "Fix the thing")
        #expect(FileManager.default.fileExists(atPath: wt.path.appendingPathComponent("d.txt").path))
    }

    @Test func refetchAfterPushSeesPushedHeadEvenIfPullRefLags() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        let pr = sb.pr(head: head)
        let wt = try await m.ensureWorktree(for: pr, localClone: nil)
        try sb.write(wt.path, "fix.txt", "fixed\n")
        try await sb.git(wt.path, "add", ".")
        try await sb.git(wt.path, "commit", "-q", "-m", "Fix")
        try await m.push(wt, pr: pr)
        // GitHub hasn't moved refs/pull/1/head yet; the branch has the new commit.
        #expect(try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head") == head)
        try await m.fetchPR(repoDir: wt.repoDir, pr: pr)
        #expect(try await m.localCommits(wt).isEmpty)
        #expect(try await m.remoteAhead(wt) == 0)
    }

    @Test func fetchFallsBackToPullRefWhenBranchIsGone() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        try await sb.git(sb.origin, "branch", "-D", "feature")
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        #expect(try await m.headSHA(wt) == head)
    }

    @Test func pushUpdatesBranchAndRemoteRef() async throws {
        let sb = try await GitSandbox()
        defer { sb.cleanup() }
        let m = try sb.manager()
        let head = try await sb.git(sb.origin, "rev-parse", "refs/pull/1/head")
        let wt = try await m.ensureWorktree(for: sb.pr(head: head), localClone: nil)
        try sb.write(wt.path, "fix.txt", "fixed\n")
        try await sb.git(wt.path, "add", ".")
        try await sb.git(wt.path, "commit", "-q", "-m", "fix")
        let local = try await m.headSHA(wt)

        await #expect(throws: WorktreeError.self) { try await m.push(wt, pr: sb.pr(head: head, canPush: false)) }
        #expect(try await sb.git(sb.origin, "rev-parse", "feature") == head)

        try await m.push(wt, pr: sb.pr(head: head))
        #expect(try await sb.git(sb.origin, "rev-parse", "feature") == local)
        #expect(try await m.localCommits(wt).isEmpty)
    }

    @Test func refusalMessages() {
        var pr = PRDetail(ref: PRRef(owner: "o", repo: "r", number: 1), nodeId: "", title: "", body: "", author: "a",
                          baseRefName: "main", headRefName: "f", baseOid: "", headOid: "", headRepoNameWithOwner: "x/r")
        #expect(!pr.canPushFixes)
        pr.headRepoNameWithOwner = "o/r"
        #expect(!pr.canPushFixes)
    }

    @Test func parseLog() {
        let s = "abc1234567\u{1F}Subject here\u{1F}Ann\u{1F}2026-01-02T03:04:05Z\nbad line\n"
        let c = WorktreeManager.parseLog(s)
        #expect(c.count == 1 && c[0].sha == "abc1234567" && c[0].subject == "Subject here" && c[0].date != nil)
    }
}
