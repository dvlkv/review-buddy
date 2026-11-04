//! On-disk checkouts used by Discuss / Fix it / summaries.
//!
//! Every PR gets its own worktree on branch `review-buddy/pr-<N>`, so the user's working
//! copy is never touched. The repo is either the user's mapped clone or a blobless bare
//! clone managed under the app's data dir. Blocking: call from worker threads.

use crate::args;
use crate::git::process::{CommandRunning, ProcessResult, ProcessRunner, ToolLocator};
use crate::models::{Date, PRDetail};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// The git dir (bare clone or `.git` of the user's clone) the worktree belongs to.
    pub repo_dir: PathBuf,
    /// Ref holding the PR head as last fetched from GitHub.
    pub remote_head_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub date: Option<Date>,
}

impl CommitInfo {
    pub fn id(&self) -> &str {
        &self.sha
    }
    pub fn short_sha(&self) -> &str {
        match self.sha.char_indices().nth(7) {
            Some((i, _)) => &self.sha[..i],
            None => &self.sha,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeError {
    GitNotFound,
    CannotPush(String),
    NotAGitRepo(String),
    /// A git command failed or couldn't start (message from the process runner).
    Command(String),
    Io(String),
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktreeError::GitNotFound => f.write_str("git was not found. Install the Xcode command line tools."),
            WorktreeError::CannotPush(why) => f.write_str(why),
            WorktreeError::NotAGitRepo(p) => write!(f, "{p} is not a git repository."),
            WorktreeError::Command(msg) | WorktreeError::Io(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for WorktreeError {}

impl From<std::io::Error> for WorktreeError {
    fn from(e: std::io::Error) -> Self {
        WorktreeError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, WorktreeError>;

/// Builds the remote URL for `owner/repo`.
pub type RemoteUrlFn = Box<dyn Fn(&str) -> String + Send + Sync>;

pub struct WorktreeManager {
    root: PathBuf,
    runner: Arc<dyn CommandRunning>,
    git: String,
    /// Remote URL builder, overridable in tests (file remotes).
    remote_url: RemoteUrlFn,
    credential_args: Vec<String>,
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn trimmed(r: &ProcessResult) -> String {
    r.stdout_string().trim().to_string()
}

fn concat(a: &[String], b: Vec<String>) -> Vec<String> {
    a.iter().cloned().chain(b).collect()
}

impl WorktreeManager {
    /// A manager using the real process runner, `git` from the tool locator and GitHub https remotes.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        Self::with_options(root, Arc::new(ProcessRunner::new()), None, None)
    }

    pub fn with_options(
        root: impl Into<PathBuf>,
        runner: Arc<dyn CommandRunning>,
        git_path: Option<String>,
        remote_url: Option<RemoteUrlFn>,
    ) -> Result<Self> {
        let git = git_path.or_else(|| ToolLocator::find("git")).ok_or(WorktreeError::GitNotFound)?;
        let remote_url = remote_url.unwrap_or_else(|| Box::new(|nwo: &str| format!("https://github.com/{nwo}.git")));
        // Authenticate https fetch/push through the GitHub CLI.
        let credential_args = match ToolLocator::find("gh") {
            Some(gh) => args!["-c", "credential.helper=", "-c", format!("credential.helper=!{gh} auth git-credential")],
            None => Vec::new(),
        };
        Ok(WorktreeManager { root: root.into(), runner, git, remote_url, credential_args })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run(&self, args: Vec<String>) -> Result<ProcessResult> {
        self.runner.run(&self.git, &args, None, None, None).map_err(WorktreeError::Command)
    }

    fn check(&self, args: Vec<String>) -> Result<ProcessResult> {
        self.runner.check(&self.git, &args, None, None, None).map_err(WorktreeError::Command)
    }

    // MARK: Repos

    pub fn managed_repo_dir(&self, name_with_owner: &str) -> PathBuf {
        self.root.join("repos").join(format!("{name_with_owner}.git"))
    }

    /// Returns the git dir to use, cloning a managed bare repo if needed.
    pub fn ensure_repo(&self, name_with_owner: &str, local_clone: Option<&Path>) -> Result<PathBuf> {
        if let Some(clone) = local_clone {
            let r = self.run(args!["-C", path_str(clone), "rev-parse", "--absolute-git-dir"])?;
            if r.status != 0 {
                return Err(WorktreeError::NotAGitRepo(path_str(clone)));
            }
            return Ok(PathBuf::from(trimmed(&r)));
        }
        let dir = self.managed_repo_dir(name_with_owner);
        if dir.join("HEAD").exists() {
            return Ok(dir);
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.check(concat(
            &self.credential_args,
            args![
                "clone",
                "--bare",
                "--filter=blob:none",
                "--quiet",
                (self.remote_url)(name_with_owner),
                path_str(&dir)
            ],
        ))?;
        Ok(dir)
    }

    fn worktree_path(&self, repo_dir: &Path, pr: &PRDetail, is_managed: bool) -> PathBuf {
        let r = &pr.pr_ref;
        if is_managed {
            return self.root.join("worktrees").join(&r.owner).join(&r.repo).join(format!("pr-{}", r.number));
        }
        repo_dir.join("review-buddy-worktrees").join(format!("pr-{}", r.number))
    }

    // MARK: Worktrees

    /// Fetches the PR head and base and creates (or updates) its worktree.
    pub fn ensure_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> Result<Worktree> {
        let repo_dir = self.ensure_repo(&pr.pr_ref.name_with_owner(), local_clone)?;
        let n = pr.pr_ref.number;
        // Not `refs/review-buddy/pr-N`: that would make the branch name `review-buddy/pr-N` ambiguous.
        let head_ref = format!("refs/review-buddy/head-{n}");
        self.fetch_pr(&repo_dir, pr)?;
        let path = self.worktree_path(&repo_dir, pr, local_clone.is_none());
        let branch = format!("review-buddy/pr-{n}");
        let wt = Worktree {
            path: path.clone(),
            branch: branch.clone(),
            repo_dir: repo_dir.clone(),
            remote_head_ref: head_ref.clone(),
        };
        if path.join(".git").exists() {
            // Fast-forward when there is nothing local to lose.
            if self.local_commits(&wt)?.is_empty() && !self.is_dirty(&wt)? {
                self.run(args!["-C", path_str(&path), "merge", "--ff-only", "--quiet", head_ref])?;
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _ = self.run(args!["--git-dir", path_str(&repo_dir), "worktree", "prune"]);
            self.check(args![
                "--git-dir",
                path_str(&repo_dir),
                "worktree",
                "add",
                "--quiet",
                "-B",
                branch,
                path_str(&path),
                head_ref
            ])?;
        }
        Ok(wt)
    }

    /// The PR's worktree if one already exists; never clones or fetches.
    pub fn ensure_existing_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> Result<Option<Worktree>> {
        let repo_dir = match local_clone {
            Some(_) => self.ensure_repo(&pr.pr_ref.name_with_owner(), local_clone)?,
            None => {
                let dir = self.managed_repo_dir(&pr.pr_ref.name_with_owner());
                if !dir.join("HEAD").exists() {
                    return Ok(None);
                }
                dir
            }
        };
        let path = self.worktree_path(&repo_dir, pr, local_clone.is_none());
        if !path.join(".git").exists() {
            return Ok(None);
        }
        let n = pr.pr_ref.number;
        Ok(Some(Worktree {
            path,
            branch: format!("review-buddy/pr-{n}"),
            repo_dir,
            remote_head_ref: format!("refs/review-buddy/head-{n}"),
        }))
    }

    /// Fetches the PR head and base. The head comes from the head repository's branch:
    /// GitHub updates `refs/pull/N/head` asynchronously, so right after a push it can still
    /// point at the old commit. Falls back to the pull ref when the branch is gone.
    pub fn fetch_pr(&self, repo_dir: &Path, pr: &PRDetail) -> Result<()> {
        let n = pr.pr_ref.number;
        let nwo = pr.pr_ref.name_with_owner();
        let fetch = |remote: String, refspec: String| {
            concat(
                &self.credential_args,
                args!["--git-dir", path_str(repo_dir), "fetch", "--quiet", "--no-tags", remote, refspec],
            )
        };
        self.check(fetch(
            (self.remote_url)(&nwo),
            format!("+refs/heads/{}:refs/review-buddy/base-{n}", pr.base_ref_name),
        ))?;
        if let Some(head_repo) = &pr.head_repo_name_with_owner {
            let r = self.run(fetch(
                (self.remote_url)(head_repo),
                format!("+refs/heads/{}:refs/review-buddy/head-{n}", pr.head_ref_name),
            ))?;
            if r.status == 0 {
                return Ok(());
            }
        }
        self.check(fetch((self.remote_url)(&nwo), format!("+refs/pull/{n}/head:refs/review-buddy/head-{n}")))?;
        Ok(())
    }

    pub fn is_dirty(&self, wt: &Worktree) -> Result<bool> {
        let r = self.check(args!["-C", path_str(&wt.path), "status", "--porcelain"])?;
        Ok(!r.stdout.is_empty())
    }

    /// Commits on the worktree branch that are not in the PR head on GitHub.
    pub fn local_commits(&self, wt: &Worktree) -> Result<Vec<CommitInfo>> {
        let r = self.check(args![
            "-C",
            path_str(&wt.path),
            "log",
            "--format=%H%x1f%s%x1f%an%x1f%aI",
            format!("{}..HEAD", wt.remote_head_ref)
        ])?;
        Ok(Self::parse_log(&r.stdout_string()))
    }

    /// Commits on the remote PR head that the worktree does not have (PR moved on).
    pub fn remote_ahead(&self, wt: &Worktree) -> Result<usize> {
        let r = self.check(args![
            "-C",
            path_str(&wt.path),
            "rev-list",
            "--count",
            format!("HEAD..{}", wt.remote_head_ref)
        ])?;
        Ok(trimmed(&r).parse().unwrap_or(0))
    }

    pub fn head_sha(&self, wt: &Worktree) -> Result<String> {
        Ok(trimmed(&self.check(args!["-C", path_str(&wt.path), "rev-parse", "HEAD"])?))
    }

    /// Parses `git log --format=%H%x1f%s%x1f%an%x1f%aI` output; malformed lines are skipped.
    pub fn parse_log(s: &str) -> Vec<CommitInfo> {
        s.split('\n')
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let f: Vec<&str> = line.split('\u{1F}').collect();
                if f.len() < 4 {
                    return None;
                }
                Some(CommitInfo {
                    sha: f[0].to_string(),
                    subject: f[1].to_string(),
                    author: f[2].to_string(),
                    date: chrono::DateTime::parse_from_rfc3339(f[3]).ok().map(|d| d.with_timezone(&chrono::Utc)),
                })
            })
            .collect()
    }

    /// Rebases local fix commits on top of the updated PR head; aborts the rebase on failure.
    pub fn rebase_onto_remote(&self, wt: &Worktree) -> Result<()> {
        let path = path_str(&wt.path);
        if let Err(e) = self.check(args!["-C", path.clone(), "rebase", "--quiet", wt.remote_head_ref.clone()]) {
            let _ = self.run(args!["-C", path, "rebase", "--abort"]);
            return Err(e);
        }
        Ok(())
    }

    /// Pushes the worktree HEAD to the PR head branch. Never force-pushes.
    pub fn push(&self, wt: &Worktree, pr: &PRDetail) -> Result<()> {
        if !pr.can_push_fixes() {
            return Err(WorktreeError::CannotPush(if pr.is_from_fork() {
                "This PR is from a fork and the author did not allow edits from maintainers.".to_string()
            } else {
                format!("You don't have push access to {}.", pr.pr_ref.name_with_owner())
            }));
        }
        let target =
            (self.remote_url)(&pr.head_repo_name_with_owner.clone().unwrap_or_else(|| pr.pr_ref.name_with_owner()));
        let path = path_str(&wt.path);
        self.check(concat(
            &self.credential_args,
            args!["-C", path.clone(), "push", "--quiet", target, format!("HEAD:refs/heads/{}", pr.head_ref_name)],
        ))?;
        // Our view of the remote head is now our HEAD.
        self.check(args!["-C", path, "update-ref", wt.remote_head_ref.clone(), "HEAD"])?;
        Ok(())
    }

    pub fn remove(&self, wt: &Worktree) -> Result<()> {
        let repo = path_str(&wt.repo_dir);
        self.check(args!["--git-dir", repo.clone(), "worktree", "remove", "--force", path_str(&wt.path)])?;
        let _ = self.run(args!["--git-dir", repo, "branch", "-D", wt.branch.clone()]);
        Ok(())
    }

    // MARK: Content

    /// Three-dot diff (merge-base), the same thing GitHub shows. No size limits.
    pub fn diff(&self, repo_dir: &Path, base: &str, head: &str) -> Result<Vec<u8>> {
        Ok(self
            .check(args![
                "--git-dir",
                path_str(repo_dir),
                "diff",
                "--no-color",
                "--no-ext-diff",
                "-M",
                format!("{base}...{head}")
            ])?
            .stdout)
    }

    /// Diff between two commits for one path, used to remap anchors.
    pub fn diff_path(&self, repo_dir: &Path, from: &str, to: &str, path: &str) -> Result<Vec<u8>> {
        Ok(self
            .check(args![
                "--git-dir",
                path_str(repo_dir),
                "diff",
                "--no-color",
                "--no-ext-diff",
                "-U0",
                from,
                to,
                "--",
                path
            ])?
            .stdout)
    }

    pub fn file_contents(&self, repo_dir: &Path, sha: &str, path: &str) -> Result<Vec<u8>> {
        Ok(self.check(args!["--git-dir", path_str(repo_dir), "show", format!("{sha}:{path}")])?.stdout)
    }

    pub fn commit_diff(&self, wt: &Worktree, sha: &str) -> Result<Vec<u8>> {
        Ok(self.check(args!["-C", path_str(&wt.path), "show", "--format=", "--no-color", sha])?.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PRRef;
    use std::collections::HashMap;

    /// Isolates git from the user's global config (signing, hooks, identity).
    struct EnvRunner {
        env: HashMap<String, String>,
    }

    impl CommandRunning for EnvRunner {
        fn run(
            &self,
            executable: &str,
            args: &[String],
            cwd: Option<&Path>,
            env: Option<&HashMap<String, String>>,
            stdin: Option<&[u8]>,
        ) -> std::result::Result<ProcessResult, String> {
            let mut merged = self.env.clone();
            if let Some(extra) = env {
                merged.extend(extra.clone());
            }
            ProcessRunner.run(executable, args, cwd, Some(&merged), stdin)
        }
    }

    fn sandbox_env() -> HashMap<String, String> {
        [
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "t@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "t@example.com"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// Real-git sandbox: an origin repo with `main`, a `feature` branch and `refs/pull/1/head`.
    struct GitSandbox {
        dir: PathBuf,
        origin: PathBuf,
        git_path: String,
        env: HashMap<String, String>,
    }

    impl GitSandbox {
        fn new() -> Self {
            // Canonical so paths compare equal to what git reports (/var → /private/var on macOS).
            let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
            let dir = tmp.join(format!("rb-git-{}", uuid::Uuid::new_v4()));
            let origin = dir.join("origin");
            std::fs::create_dir_all(&origin).unwrap();
            let sb = GitSandbox { dir, origin, git_path: ToolLocator::find("git").expect("git"), env: sandbox_env() };
            let o = sb.origin.clone();
            sb.git(&o, &["init", "-q", "-b", "main"]);
            sb.git(&o, &["config", "receive.denyCurrentBranch", "updateInstead"]);
            sb.write(&o, "a.txt", "one\ntwo\nthree\n");
            sb.git(&o, &["add", "."]);
            sb.git(&o, &["commit", "-q", "-m", "base"]);
            sb.git(&o, &["checkout", "-q", "-b", "feature"]);
            sb.write(&o, "a.txt", "one\nTWO\nthree\n");
            sb.write(&o, "b.txt", "new file\n");
            sb.git(&o, &["add", "."]);
            sb.git(&o, &["commit", "-q", "-m", "feature work"]);
            sb.git(&o, &["update-ref", "refs/pull/1/head", "HEAD"]);
            sb.git(&o, &["checkout", "-q", "main"]);
            sb
        }

        fn git(&self, cwd: &Path, a: &[&str]) -> String {
            let mut full = args!["-C", path_str(cwd)];
            full.extend(a.iter().map(|s| s.to_string()));
            let r = ProcessRunner.check(&self.git_path, &full, None, Some(&self.env), None).unwrap();
            trimmed(&r)
        }

        fn write(&self, cwd: &Path, name: &str, text: &str) {
            std::fs::write(cwd.join(name), text).unwrap();
        }

        fn manager(&self) -> WorktreeManager {
            let o = path_str(&self.origin);
            WorktreeManager::with_options(
                self.dir.join("support"),
                Arc::new(EnvRunner { env: self.env.clone() }),
                Some(self.git_path.clone()),
                Some(Box::new(move |_: &str| o.clone())),
            )
            .unwrap()
        }

        fn pr(&self, head: &str, can_push: bool) -> PRDetail {
            let mut pr = PRDetail::new(PRRef::new("o", "r", 1), "PR", "T", "main", "feature", "", head);
            pr.author = "alice".into();
            pr.head_repo_name_with_owner = Some("o/r".into());
            pr.viewer_login = "me".into();
            pr.viewer_can_push = can_push;
            pr
        }
    }

    impl Drop for GitSandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn text(b: Vec<u8>) -> String {
        String::from_utf8(b).unwrap()
    }

    /// Paths of the files in a `git diff` (from the `diff --git a/x b/x` headers).
    fn diff_paths(diff: &str) -> Vec<String> {
        let mut v: Vec<String> = diff
            .lines()
            .filter_map(|l| l.strip_prefix("diff --git a/"))
            .filter_map(|l| l.split(" b/").next().map(str::to_string))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn managed_clone_and_worktree() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let repo = m.ensure_repo("o/r", None).unwrap();
        assert_eq!(repo, m.managed_repo_dir("o/r"));
        assert!(repo.join("HEAD").exists());
        // Idempotent.
        assert_eq!(m.ensure_repo("o/r", None).unwrap(), repo);

        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        let wt = m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        assert_eq!(wt.branch, "review-buddy/pr-1");
        assert!(wt.path.ends_with("worktrees/o/r/pr-1"));
        assert_eq!(m.head_sha(&wt).unwrap(), head);
        assert_eq!(sb.git(&wt.path, &["rev-parse", "--abbrev-ref", "HEAD"]), "review-buddy/pr-1");
        assert_eq!(std::fs::read_to_string(wt.path.join("b.txt")).unwrap(), "new file\n");
        assert!(m.local_commits(&wt).unwrap().is_empty());
        assert!(!m.is_dirty(&wt).unwrap());

        // Diff base...head, same as GitHub.
        let diff = text(m.diff(&repo, "refs/review-buddy/base-1", "refs/review-buddy/head-1").unwrap());
        assert_eq!(diff_paths(&diff), vec!["a.txt", "b.txt"]);
        assert!(diff.contains("new file mode"), "{diff}");

        let content = m.file_contents(&repo, &head, "a.txt").unwrap();
        assert_eq!(text(content), "one\nTWO\nthree\n");

        let base = sb.git(&sb.origin, &["rev-parse", "main"]);
        let file_diff = text(m.diff_path(&repo, &base, &head, "a.txt").unwrap());
        assert_eq!(diff_paths(&file_diff), vec!["a.txt"]);
        assert!(file_diff.contains("@@ -2 +2 @@") && file_diff.contains("\n-two\n+TWO\n"), "{file_diff}");

        m.remove(&wt).unwrap();
        assert!(!wt.path.exists());
    }

    #[test]
    fn ensure_existing_worktree_never_clones() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        // Nothing on disk yet: None, and no clone was made.
        assert_eq!(m.ensure_existing_worktree(&sb.pr(&head, true), None).unwrap(), None);
        assert!(!m.managed_repo_dir("o/r").exists());
        let wt = m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        assert_eq!(m.ensure_existing_worktree(&sb.pr(&head, true), None).unwrap(), Some(wt.clone()));
        m.remove(&wt).unwrap();
        assert_eq!(m.ensure_existing_worktree(&sb.pr(&head, true), None).unwrap(), None);
    }

    #[test]
    fn local_clone_worktree_lives_inside_git_dir() {
        let sb = GitSandbox::new();
        let clone = sb.dir.join("clone");
        sb.git(&sb.dir, &["clone", "-q", &path_str(&sb.origin), &path_str(&clone)]);
        let m = sb.manager();
        let repo = m.ensure_repo("o/r", Some(&clone)).unwrap();
        assert_eq!(std::fs::canonicalize(&repo).unwrap(), std::fs::canonicalize(clone.join(".git")).unwrap());
        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        let wt = m.ensure_worktree(&sb.pr(&head, true), Some(&clone)).unwrap();
        assert!(path_str(&wt.path).contains(".git/review-buddy-worktrees/pr-1"));
        assert_eq!(m.head_sha(&wt).unwrap(), head);
        // User's working copy untouched.
        assert_eq!(sb.git(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
        // An existing local-clone worktree is found without fetching.
        assert_eq!(m.ensure_existing_worktree(&sb.pr(&head, true), Some(&clone)).unwrap(), Some(wt));
    }

    #[test]
    fn not_a_repo() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let bogus = sb.dir.join("nothing");
        std::fs::create_dir_all(&bogus).unwrap();
        assert_eq!(m.ensure_repo("o/r", Some(&bogus)), Err(WorktreeError::NotAGitRepo(path_str(&bogus))));
        assert_eq!(WorktreeError::NotAGitRepo("/x".into()).to_string(), "/x is not a git repository.");
    }

    #[test]
    fn local_commits_remote_ahead_fast_forward_and_rebase() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let o = sb.origin.clone();
        let mut head = sb.git(&o, &["rev-parse", "refs/pull/1/head"]);
        let wt = m.ensure_worktree(&sb.pr(&head, true), None).unwrap();

        // PR moves on remotely; no local commits → re-ensure fast-forwards.
        sb.git(&o, &["checkout", "-q", "feature"]);
        sb.write(&o, "c.txt", "c\n");
        sb.git(&o, &["add", "."]);
        sb.git(&o, &["commit", "-q", "-m", "remote 2"]);
        head = sb.git(&o, &["rev-parse", "HEAD"]);
        sb.git(&o, &["update-ref", "refs/pull/1/head", &head]);
        m.fetch_pr(&wt.repo_dir, &sb.pr(&head, true)).unwrap();
        assert_eq!(m.remote_ahead(&wt).unwrap(), 1);
        m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        assert_eq!(m.head_sha(&wt).unwrap(), head);
        assert_eq!(m.remote_ahead(&wt).unwrap(), 0);

        // Local fix commit.
        sb.write(&wt.path, "fix.txt", "fixed\n");
        assert!(m.is_dirty(&wt).unwrap());
        sb.git(&wt.path, &["add", "."]);
        sb.git(&wt.path, &["commit", "-q", "-m", "Fix the thing"]);
        let local = m.local_commits(&wt).unwrap();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].subject, "Fix the thing");
        assert_eq!(local[0].author, "Test");
        assert!(local[0].date.is_some());
        assert_eq!(local[0].short_sha().len(), 7);
        let show = text(m.commit_diff(&wt, &local[0].sha).unwrap());
        assert_eq!(diff_paths(&show), vec!["fix.txt"]);

        // Remote moves again; re-ensure must NOT fast-forward over local work.
        sb.write(&o, "d.txt", "d\n");
        sb.git(&o, &["add", "."]);
        sb.git(&o, &["commit", "-q", "-m", "remote 3"]);
        head = sb.git(&o, &["rev-parse", "HEAD"]);
        sb.git(&o, &["update-ref", "refs/pull/1/head", &head]);
        sb.git(&o, &["checkout", "-q", "main"]);
        m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        assert_eq!(m.remote_ahead(&wt).unwrap(), 1);
        assert_eq!(m.local_commits(&wt).unwrap().len(), 1);

        m.rebase_onto_remote(&wt).unwrap();
        assert_eq!(m.remote_ahead(&wt).unwrap(), 0);
        let after = m.local_commits(&wt).unwrap();
        assert!(after.len() == 1 && after[0].subject == "Fix the thing");
        assert!(wt.path.join("d.txt").exists());
    }

    #[test]
    fn refetch_after_push_sees_pushed_head_even_if_pull_ref_lags() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        let pr = sb.pr(&head, true);
        let wt = m.ensure_worktree(&pr, None).unwrap();
        sb.write(&wt.path, "fix.txt", "fixed\n");
        sb.git(&wt.path, &["add", "."]);
        sb.git(&wt.path, &["commit", "-q", "-m", "Fix"]);
        m.push(&wt, &pr).unwrap();
        // GitHub hasn't moved refs/pull/1/head yet; the branch has the new commit.
        assert_eq!(sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]), head);
        m.fetch_pr(&wt.repo_dir, &pr).unwrap();
        assert!(m.local_commits(&wt).unwrap().is_empty());
        assert_eq!(m.remote_ahead(&wt).unwrap(), 0);
    }

    #[test]
    fn fetch_falls_back_to_pull_ref_when_branch_is_gone() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        sb.git(&sb.origin, &["branch", "-D", "feature"]);
        let wt = m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        assert_eq!(m.head_sha(&wt).unwrap(), head);
    }

    #[test]
    fn push_updates_branch_and_remote_ref() {
        let sb = GitSandbox::new();
        let m = sb.manager();
        let head = sb.git(&sb.origin, &["rev-parse", "refs/pull/1/head"]);
        let wt = m.ensure_worktree(&sb.pr(&head, true), None).unwrap();
        sb.write(&wt.path, "fix.txt", "fixed\n");
        sb.git(&wt.path, &["add", "."]);
        sb.git(&wt.path, &["commit", "-q", "-m", "fix"]);
        let local = m.head_sha(&wt).unwrap();

        let err = m.push(&wt, &sb.pr(&head, false)).unwrap_err();
        assert_eq!(err, WorktreeError::CannotPush("You don't have push access to o/r.".into()));
        assert_eq!(sb.git(&sb.origin, &["rev-parse", "feature"]), head);

        m.push(&wt, &sb.pr(&head, true)).unwrap();
        assert_eq!(sb.git(&sb.origin, &["rev-parse", "feature"]), local);
        assert!(m.local_commits(&wt).unwrap().is_empty());
    }

    #[test]
    fn refusal_messages() {
        let mut pr = PRDetail::new(PRRef::new("o", "r", 1), "", "", "main", "f", "", "");
        pr.author = "a".into();
        pr.head_repo_name_with_owner = Some("x/r".into());
        assert!(!pr.can_push_fixes());
        pr.head_repo_name_with_owner = Some("o/r".into());
        assert!(!pr.can_push_fixes());
    }

    #[test]
    fn parse_log() {
        let s = "abc1234567\u{1F}Subject here\u{1F}Ann\u{1F}2026-01-02T03:04:05Z\nbad line\nd\u{1F}s\u{1F}a\u{1F}2026-01-02T05:04:05+02:00\n";
        let c = WorktreeManager::parse_log(s);
        assert_eq!(c.len(), 2);
        assert!(c[0].sha == "abc1234567" && c[0].subject == "Subject here" && c[0].author == "Ann");
        assert_eq!(c[0].date, c[1].date);
        assert!(c[0].date.is_some());
        assert_eq!(c[0].short_sha(), "abc1234");
        assert_eq!(c[1].short_sha(), "d");
    }
}
