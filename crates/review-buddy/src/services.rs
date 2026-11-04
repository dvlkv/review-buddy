//! Dependency container shared by all stores, and the traits that let tests swap
//! git and claude for fakes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use review_buddy_core::claude::session::{
    ClaudeConfig, ClaudeOneShot, ClaudeOneShotRunning, ClaudeSession, ClaudeSessionApi, SessionOutput,
};
use review_buddy_core::git::process::ToolLocator;
use review_buddy_core::git::worktree::{CommitInfo, Worktree, WorktreeManager};
use review_buddy_core::github::client::{GitHubAPI, GitHubClient};
use review_buddy_core::persistence::{AppSettings, LocalStore};
use review_buddy_core::PRDetail;

pub type GitResult<T> = Result<T, String>;

/// Git operations the stores need; `WorktreeManager` in production, fakes in tests.
pub trait WorktreeProviding: Send + Sync {
    fn ensure_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> GitResult<Worktree>;
    fn fetch_pr(&self, repo_dir: &Path, pr: &PRDetail) -> GitResult<()>;
    fn local_commits(&self, wt: &Worktree) -> GitResult<Vec<CommitInfo>>;
    fn remote_ahead(&self, wt: &Worktree) -> GitResult<usize>;
    fn rebase_onto_remote(&self, wt: &Worktree) -> GitResult<()>;
    fn push(&self, wt: &Worktree, pr: &PRDetail) -> GitResult<()>;
    fn diff(&self, repo_dir: &Path, base: &str, head: &str) -> GitResult<Vec<u8>>;
    fn diff_path(&self, repo_dir: &Path, from: &str, to: &str, path: &str) -> GitResult<Vec<u8>>;
    fn file_contents(&self, repo_dir: &Path, sha: &str, path: &str) -> GitResult<Vec<u8>>;
    fn head_sha(&self, wt: &Worktree) -> GitResult<String>;
    #[allow(dead_code)] // the push sheet lists commits; diffs per commit are not shown yet
    fn commit_diff(&self, wt: &Worktree, sha: &str) -> GitResult<Vec<u8>>;
    /// The PR's worktree if it already exists on disk (never clones or fetches).
    fn ensure_existing_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> GitResult<Option<Worktree>>;
    fn remove(&self, wt: &Worktree) -> GitResult<()>;
}

fn s<T, E: std::fmt::Display>(r: Result<T, E>) -> GitResult<T> {
    r.map_err(|e| e.to_string())
}

impl WorktreeProviding for WorktreeManager {
    fn ensure_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> GitResult<Worktree> {
        s(WorktreeManager::ensure_worktree(self, pr, local_clone))
    }
    fn fetch_pr(&self, repo_dir: &Path, pr: &PRDetail) -> GitResult<()> {
        s(WorktreeManager::fetch_pr(self, repo_dir, pr))
    }
    fn local_commits(&self, wt: &Worktree) -> GitResult<Vec<CommitInfo>> {
        s(WorktreeManager::local_commits(self, wt))
    }
    fn remote_ahead(&self, wt: &Worktree) -> GitResult<usize> {
        s(WorktreeManager::remote_ahead(self, wt))
    }
    fn rebase_onto_remote(&self, wt: &Worktree) -> GitResult<()> {
        s(WorktreeManager::rebase_onto_remote(self, wt))
    }
    fn push(&self, wt: &Worktree, pr: &PRDetail) -> GitResult<()> {
        s(WorktreeManager::push(self, wt, pr))
    }
    fn diff(&self, repo_dir: &Path, base: &str, head: &str) -> GitResult<Vec<u8>> {
        s(WorktreeManager::diff(self, repo_dir, base, head))
    }
    fn diff_path(&self, repo_dir: &Path, from: &str, to: &str, path: &str) -> GitResult<Vec<u8>> {
        s(WorktreeManager::diff_path(self, repo_dir, from, to, path))
    }
    fn file_contents(&self, repo_dir: &Path, sha: &str, path: &str) -> GitResult<Vec<u8>> {
        s(WorktreeManager::file_contents(self, repo_dir, sha, path))
    }
    fn head_sha(&self, wt: &Worktree) -> GitResult<String> {
        s(WorktreeManager::head_sha(self, wt))
    }
    fn commit_diff(&self, wt: &Worktree, sha: &str) -> GitResult<Vec<u8>> {
        s(WorktreeManager::commit_diff(self, wt, sha))
    }
    fn ensure_existing_worktree(&self, pr: &PRDetail, local_clone: Option<&Path>) -> GitResult<Option<Worktree>> {
        s(WorktreeManager::ensure_existing_worktree(self, pr, local_clone))
    }
    fn remove(&self, wt: &Worktree) -> GitResult<()> {
        s(WorktreeManager::remove(self, wt))
    }
}

pub type OutputSink = Box<dyn Fn(SessionOutput) + Send + Sync>;
pub type SessionFactory = Arc<dyn Fn(ClaudeConfig, OutputSink) -> Box<dyn ClaudeSessionApi> + Send + Sync>;

pub struct Services {
    pub github: Arc<dyn GitHubAPI>,
    pub store: Arc<LocalStore>,
    settings: RwLock<AppSettings>,
    pub worktrees: Option<Arc<dyn WorktreeProviding>>,
    pub claude_path: Option<String>,
    pub make_session: SessionFactory,
    pub one_shot: Arc<dyn ClaudeOneShotRunning>,
}

pub fn live_session_factory() -> SessionFactory {
    Arc::new(|cfg, sink| Box::new(ClaudeSession::new(cfg, sink)) as Box<dyn ClaudeSessionApi>)
}

impl Services {
    pub fn new(
        github: Arc<dyn GitHubAPI>,
        store: Arc<LocalStore>,
        settings: AppSettings,
        worktrees: Option<Arc<dyn WorktreeProviding>>,
        claude_path: Option<String>,
    ) -> Self {
        Services {
            github,
            store,
            settings: RwLock::new(settings),
            worktrees,
            claude_path,
            make_session: live_session_factory(),
            one_shot: Arc::new(ClaudeOneShot::default()),
        }
    }

    pub fn live() -> Self {
        let store = Arc::new(LocalStore::default());
        let settings = store.load_settings();
        let worktrees = WorktreeManager::new(store.root().to_path_buf())
            .ok()
            .map(|w| Arc::new(w) as Arc<dyn WorktreeProviding>);
        Services::new(Arc::new(GitHubClient::new()), store, settings, worktrees, ToolLocator::find("claude"))
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.read().unwrap().clone()
    }

    /// Changes settings and saves them in the background.
    pub fn update_settings(&self, f: impl FnOnce(&mut AppSettings)) {
        f(&mut self.settings.write().unwrap());
        self.save_settings();
    }

    pub fn save_settings(&self) {
        let store = self.store.clone();
        let s = self.settings();
        std::thread::spawn(move || {
            let _ = store.save_settings(&s);
        });
    }

    pub fn local_clone(&self, name_with_owner: &str) -> Option<PathBuf> {
        self.settings.read().unwrap().local_clone(name_with_owner)
    }

    pub fn claude_model(&self) -> Option<String> {
        let m = self.settings.read().unwrap().claude_model.trim().to_string();
        if m.is_empty() { None } else { Some(m) }
    }
}
