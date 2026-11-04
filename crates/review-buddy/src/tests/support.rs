//! Test harness: an `AppState` on an inline spawner with fake git, GitHub and Claude.
//! Port of `Tests/ReviewBuddyUITests/TestSupport.swift`.
//!
//! Work runs synchronously on the test thread; its result jobs queue until `drain`
//! applies them (and whatever they queue in turn). Swift's `eventually {}` becomes `drain()`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use review_buddy_core::claude::session::{ClaudeConfig, ClaudeError, ClaudeOneShotRunning, ClaudeSessionApi, SessionOutput};
use review_buddy_core::claude::stream_events::{ClaudeEvent, ClaudeResult, ContentBlock};
use review_buddy_core::git::worktree::{CommitInfo, Worktree};
use review_buddy_core::github::client::{GitHubAPI, GitHubError, GitHubResult, MergeResult};
use review_buddy_core::persistence::{AppSettings, LocalStore};
use review_buddy_core::support::synthetic::Shape;
use review_buddy_core::*;
use serde_json::Value;

use crate::app::AppState;
use crate::demo::DemoGitHub;
use crate::pr_store::PRStore;
use crate::rt::{Job, Spawner};
use crate::services::{GitResult, OutputSink, Services, WorktreeProviding};

pub fn temp_dir(prefix: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("{prefix}-{}", new_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}

// MARK: - Fake git

pub struct FakeWorktrees {
    pub dir: PathBuf,
    pub calls: Mutex<Vec<String>>,
    pub local_commits: Mutex<Vec<CommitInfo>>,
    pub head: Mutex<String>,
    pub push_error: Mutex<Option<String>>,
    pub ensure_error: Mutex<Option<String>>,
    /// Unified diff returned by `diff_path`.
    pub path_diff: Mutex<String>,
    pub file_contents_text: Mutex<String>,
    /// `git show` fails, e.g. the commit isn't fetched yet.
    pub file_contents_error: Mutex<Option<String>>,
    /// Heads `git diff` can't resolve (a commit not fetched).
    pub unknown_heads: Mutex<HashSet<String>>,
    /// Whether a worktree "exists on disk" for `ensure_existing_worktree`.
    pub has_existing_worktree: AtomicBool,
}

impl FakeWorktrees {
    pub fn new() -> Self {
        FakeWorktrees {
            dir: temp_dir("rb-wt"),
            calls: Mutex::new(vec![]),
            local_commits: Mutex::new(vec![]),
            head: Mutex::new("base-head".into()),
            push_error: Mutex::new(None),
            ensure_error: Mutex::new(None),
            path_diff: Mutex::new(String::new()),
            file_contents_text: Mutex::new((1..=300).map(|i| format!("content line {i}")).collect::<Vec<_>>().join("\n")),
            file_contents_error: Mutex::new(None),
            unknown_heads: Mutex::new(HashSet::new()),
            has_existing_worktree: AtomicBool::new(false),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
    pub fn count(&self, call: &str) -> usize {
        self.calls().iter().filter(|c| *c == call).count()
    }
    pub fn set_head(&self, h: &str) {
        *lock(&self.head) = h.into();
    }
    pub fn set_commits(&self, c: Vec<CommitInfo>) {
        *lock(&self.local_commits) = c;
    }
    fn record(&self, s: impl Into<String>) {
        lock(&self.calls).push(s.into());
    }

    pub fn worktree(&self) -> Worktree {
        Worktree {
            path: self.dir.clone(),
            branch: "review-buddy/pr-1".into(),
            repo_dir: self.dir.join(".git"),
            remote_head_ref: "refs/review-buddy/head-1".into(),
        }
    }
}

pub fn commit(sha: &str, subject: &str) -> CommitInfo {
    CommitInfo { sha: sha.into(), subject: subject.into(), author: "me".into(), date: None }
}

impl WorktreeProviding for FakeWorktrees {
    fn ensure_worktree(&self, _pr: &PRDetail, _local_clone: Option<&Path>) -> GitResult<Worktree> {
        self.record("ensureWorktree");
        if let Some(e) = lock(&self.ensure_error).clone() {
            return Err(e);
        }
        Ok(self.worktree())
    }
    fn fetch_pr(&self, _repo_dir: &Path, _pr: &PRDetail) -> GitResult<()> {
        self.record("fetchPR");
        Ok(())
    }
    fn local_commits(&self, _wt: &Worktree) -> GitResult<Vec<CommitInfo>> {
        self.record("localCommits");
        Ok(lock(&self.local_commits).clone())
    }
    fn remote_ahead(&self, _wt: &Worktree) -> GitResult<usize> {
        self.record("remoteAhead");
        Ok(0)
    }
    fn rebase_onto_remote(&self, _wt: &Worktree) -> GitResult<()> {
        self.record("rebase");
        Ok(())
    }
    fn push(&self, _wt: &Worktree, _pr: &PRDetail) -> GitResult<()> {
        self.record("push");
        if let Some(e) = lock(&self.push_error).clone() {
            return Err(e);
        }
        lock(&self.local_commits).clear();
        Ok(())
    }
    fn diff(&self, _repo_dir: &Path, _base: &str, head: &str) -> GitResult<Vec<u8>> {
        self.record("diff");
        self.record(format!("diff:{head}"));
        if lock(&self.unknown_heads).contains(head) {
            return Err("bad revision".into());
        }
        Ok(vec![])
    }
    fn diff_path(&self, _repo_dir: &Path, _from: &str, _to: &str, path: &str) -> GitResult<Vec<u8>> {
        self.record(format!("diffPath:{path}"));
        Ok(lock(&self.path_diff).clone().into_bytes())
    }
    fn file_contents(&self, _repo_dir: &Path, _sha: &str, path: &str) -> GitResult<Vec<u8>> {
        self.record(format!("fileContents:{path}"));
        if let Some(e) = lock(&self.file_contents_error).clone() {
            return Err(e);
        }
        Ok(lock(&self.file_contents_text).clone().into_bytes())
    }
    fn head_sha(&self, _wt: &Worktree) -> GitResult<String> {
        self.record("headSHA");
        Ok(lock(&self.head).clone())
    }
    fn commit_diff(&self, _wt: &Worktree, _sha: &str) -> GitResult<Vec<u8>> {
        Ok(vec![])
    }
    fn ensure_existing_worktree(&self, _pr: &PRDetail, _local_clone: Option<&Path>) -> GitResult<Option<Worktree>> {
        self.record("ensureExisting");
        Ok(self.has_existing_worktree.load(Ordering::SeqCst).then(|| self.worktree()))
    }
    fn remove(&self, _wt: &Worktree) -> GitResult<()> {
        self.record("remove");
        Ok(())
    }
}

// MARK: - Fake Claude session

pub struct FakeSessionInner {
    pub config: ClaudeConfig,
    sink: OutputSink,
    pub sent: Mutex<Vec<String>>,
    pub interrupts: AtomicUsize,
    pub terminated: AtomicBool,
    pub turn_active: AtomicBool,
}

impl FakeSessionInner {
    pub fn sent(&self) -> Vec<String> {
        lock(&self.sent).clone()
    }
    pub fn turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }
    /// Delivers output the way the real session's reader thread does (queued as a job).
    pub fn emit(&self, out: SessionOutput) {
        (self.sink)(out);
    }
    pub fn event(&self, ev: ClaudeEvent) {
        self.emit(SessionOutput::Event(ev));
    }
    /// Emits a full assistant turn: init, text, result.
    pub fn reply(&self, text: &str, session_id: &str) {
        self.event(ClaudeEvent::Initialized { session_id: session_id.into(), model: Some("test".into()) });
        self.event(ClaudeEvent::Assistant { message_id: new_id(), blocks: vec![ContentBlock::Text(text.into())] });
        self.event(ClaudeEvent::Result(ClaudeResult {
            session_id: Some(session_id.into()),
            is_error: false,
            subtype: "success".into(),
            text: Some(text.into()),
            cost_usd: Some(0.0),
            structured_output: None,
        }));
    }
}

struct FakeSession(Arc<FakeSessionInner>);

impl ClaudeSessionApi for FakeSession {
    fn send(&self, text: &str) -> Result<(), ClaudeError> {
        lock(&self.0.sent).push(text.into());
        Ok(())
    }
    fn interrupt(&self) {
        self.0.interrupts.fetch_add(1, Ordering::SeqCst);
    }
    fn terminate(&self) {
        self.0.terminated.store(true, Ordering::SeqCst);
    }
    fn set_turn_active(&self, active: bool) {
        self.0.turn_active.store(active, Ordering::SeqCst);
    }
}

/// Keeps every session the store creates.
#[derive(Default)]
pub struct Sessions {
    list: Mutex<Vec<Arc<FakeSessionInner>>>,
}

impl Sessions {
    pub fn all(&self) -> Vec<Arc<FakeSessionInner>> {
        lock(&self.list).clone()
    }
    pub fn get(&self, i: usize) -> Arc<FakeSessionInner> {
        self.all()[i].clone()
    }
    pub fn count(&self) -> usize {
        lock(&self.list).len()
    }
}

// MARK: - Fake one-shot

#[derive(Default)]
pub struct FakeOneShot {
    pub configs: Mutex<Vec<ClaudeConfig>>,
    pub prompts: Mutex<Vec<String>>,
    pub structured: Mutex<Option<Value>>,
    pub text: Mutex<Option<String>>,
    pub error: Mutex<Option<String>>,
}

impl FakeOneShot {
    pub fn configs(&self) -> Vec<ClaudeConfig> {
        lock(&self.configs).clone()
    }
    pub fn last_config(&self) -> ClaudeConfig {
        self.configs().last().cloned().expect("no one-shot run")
    }
    pub fn last_prompt(&self) -> String {
        lock(&self.prompts).last().cloned().expect("no one-shot run")
    }
    pub fn set_structured(&self, json: &str) {
        *lock(&self.structured) = Some(serde_json::from_str(json).unwrap());
    }
    pub fn set_text(&self, t: Option<&str>) {
        *lock(&self.text) = t.map(str::to_string);
    }
    pub fn set_error(&self, e: Option<&str>) {
        *lock(&self.error) = e.map(str::to_string);
    }
}

impl ClaudeOneShotRunning for FakeOneShot {
    fn run(&self, config: &ClaudeConfig, prompt: &str) -> Result<ClaudeResult, ClaudeError> {
        lock(&self.configs).push(config.clone());
        lock(&self.prompts).push(prompt.into());
        if let Some(e) = lock(&self.error).clone() {
            return Err(ClaudeError::Failed(e));
        }
        Ok(ClaudeResult {
            session_id: Some("fork".into()),
            is_error: false,
            subtype: "success".into(),
            text: lock(&self.text).clone(),
            cost_usd: Some(0.0),
            structured_output: lock(&self.structured).clone(),
        })
    }
}

// MARK: - GitHub wrapper

/// Delegates to `DemoGitHub` but can fail selected calls.
pub struct FlakyGitHub {
    pub base: Arc<DemoGitHub>,
    pub fail_viewed: AtomicBool,
    pub fail_search: AtomicBool,
    pub fail_resolve: AtomicBool,
    /// The REST diff is too large: the store falls back to a local checkout.
    pub diff_too_large: AtomicBool,
    /// Served (in order) before the real diff, e.g. a stale one right after a push.
    pub queued_diffs: Mutex<Vec<Vec<u8>>>,
    /// Replaces file contents (path, ref) -> data.
    pub contents: Mutex<Option<Box<dyn Fn(&str, &str) -> Vec<u8> + Send + Sync>>>,
    pub content_calls: AtomicUsize,
}

impl FlakyGitHub {
    pub fn new(base: Arc<DemoGitHub>) -> Self {
        FlakyGitHub {
            base,
            fail_viewed: AtomicBool::new(false),
            fail_search: AtomicBool::new(false),
            fail_resolve: AtomicBool::new(false),
            diff_too_large: AtomicBool::new(false),
            queued_diffs: Mutex::new(vec![]),
            contents: Mutex::new(None),
            content_calls: AtomicUsize::new(0),
        }
    }
}

fn test_error(m: &str) -> GitHubError {
    GitHubError::Transport(m.into())
}

impl GitHubAPI for FlakyGitHub {
    fn search(&self, query: &str) -> GitHubResult<Vec<PRSummary>> {
        if self.fail_search.load(Ordering::SeqCst) && query.contains("author:@me") {
            return Err(GitHubError::GraphQL(vec!["search failed".into()]));
        }
        self.base.search(query)
    }
    fn pull_request(&self, pr: &PRRef) -> GitHubResult<PRDetail> {
        self.base.pull_request(pr)
    }
    fn diff(&self, pr: &PRRef) -> GitHubResult<Vec<u8>> {
        if self.diff_too_large.load(Ordering::SeqCst) {
            return Err(GitHubError::DiffTooLarge);
        }
        {
            let mut q = lock(&self.queued_diffs);
            if !q.is_empty() {
                return Ok(q.remove(0));
            }
        }
        self.base.diff(pr)
    }
    fn head_oid(&self, pr: &PRRef) -> GitHubResult<String> {
        self.base.head_oid(pr)
    }
    fn file_contents(&self, owner: &str, repo: &str, path: &str, git_ref: &str) -> GitHubResult<Vec<u8>> {
        self.content_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(f) = lock(&self.contents).as_ref() {
            return Ok(f(path, git_ref));
        }
        self.base.file_contents(owner, repo, path, git_ref)
    }
    fn start_review(&self, id: &str) -> GitHubResult<String> {
        self.base.start_review(id)
    }
    fn add_thread(&self, pr_id: &str, review_id: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<ReviewThread> {
        self.base.add_thread(pr_id, review_id, anchor, body)
    }
    fn reply(&self, thread_id: &str, review_id: Option<&str>, body: &str) -> GitHubResult<ReviewComment> {
        self.base.reply(thread_id, review_id, body)
    }
    fn add_single_comment(&self, pr: &PRRef, commit: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<()> {
        self.base.add_single_comment(pr, commit, anchor, body)
    }
    fn reply_single(&self, pr: &PRRef, thread_id: &str, db_id: i64, body: &str) -> GitHubResult<()> {
        self.base.reply_single(pr, thread_id, db_id, body)
    }
    fn add_comment(&self, subject_id: &str, body: &str) -> GitHubResult<ConversationComment> {
        self.base.add_comment(subject_id, body)
    }
    fn merge_pull_request(&self, pr: &PRRef, id: &str, m: MergeMethod, oid: &str, bypass: bool) -> GitHubResult<MergeResult> {
        self.base.merge_pull_request(pr, id, m, oid, bypass)
    }
    fn set_pull_request_closed(&self, id: &str, closed: bool) -> GitHubResult<()> {
        self.base.set_pull_request_closed(id, closed)
    }
    fn update_comment(&self, id: &str, body: &str) -> GitHubResult<ReviewComment> {
        self.base.update_comment(id, body)
    }
    fn delete_comment(&self, id: &str) -> GitHubResult<()> {
        self.base.delete_comment(id)
    }
    fn submit_review(&self, review_id: &str, event: ReviewEvent, body: &str) -> GitHubResult<()> {
        self.base.submit_review(review_id, event, body)
    }
    fn delete_review(&self, review_id: &str) -> GitHubResult<()> {
        self.base.delete_review(review_id)
    }
    fn set_resolved(&self, thread_id: &str, resolved: bool) -> GitHubResult<bool> {
        if self.fail_resolve.load(Ordering::SeqCst) {
            return Err(test_error("boom"));
        }
        self.base.set_resolved(thread_id, resolved)
    }
    fn set_viewed(&self, pr_id: &str, path: &str, viewed: bool) -> GitHubResult<()> {
        if self.fail_viewed.load(Ordering::SeqCst) {
            return Err(GitHubError::GraphQL(vec!["viewed failed".into()]));
        }
        self.base.set_viewed(pr_id, path, viewed)
    }
}

// MARK: - Harness

pub const TEST_SHAPE: Shape = Shape::new(6, 2, 12);

pub fn pr_ref() -> PRRef {
    PRRef::new("demo", "synthetic", 1)
}

pub struct Opts {
    pub claude_path: Option<String>,
    pub threads_every: usize,
    pub root: Option<PathBuf>,
    pub worktrees: bool,
    pub settings: AppSettings,
    /// Wrap the demo GitHub in a `FlakyGitHub`.
    pub flaky: bool,
    pub configure: Option<Box<dyn FnOnce(&FlakyGitHub)>>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            claude_path: Some("/usr/bin/true".into()),
            threads_every: 25,
            root: None,
            worktrees: true,
            settings: AppSettings::default(),
            flaky: false,
            configure: None,
        }
    }
}

pub struct Harness {
    pub demo: Arc<DemoGitHub>,
    pub flaky: Option<Arc<FlakyGitHub>>,
    pub services: Arc<Services>,
    pub worktrees: Arc<FakeWorktrees>,
    pub one_shot: Arc<FakeOneShot>,
    pub sessions: Arc<Sessions>,
    pub app: AppState,
    rx: Receiver<Job>,
}

impl Harness {
    pub fn new() -> Self {
        Self::with(Opts::default())
    }

    pub fn with(o: Opts) -> Self {
        let demo = Arc::new(DemoGitHub::synthetic(TEST_SHAPE, o.threads_every));
        let flaky = o.flaky.then(|| {
            let f = FlakyGitHub::new(demo.clone());
            if let Some(c) = o.configure {
                c(&f);
            }
            Arc::new(f)
        });
        let github: Arc<dyn GitHubAPI> = match &flaky {
            Some(f) => f.clone(),
            None => demo.clone(),
        };
        let root = o.root.unwrap_or_else(|| temp_dir("rb-ui"));
        let worktrees = Arc::new(FakeWorktrees::new());
        let one_shot = Arc::new(FakeOneShot::default());
        let sessions = Arc::new(Sessions::default());
        let mut services = Services::new(
            github,
            Arc::new(LocalStore::new(root)),
            o.settings,
            o.worktrees.then(|| worktrees.clone() as Arc<dyn WorktreeProviding>),
            o.claude_path,
        );
        let list = sessions.clone();
        services.make_session = Arc::new(move |config, sink| {
            let inner = Arc::new(FakeSessionInner {
                config,
                sink,
                sent: Mutex::new(vec![]),
                interrupts: AtomicUsize::new(0),
                terminated: AtomicBool::new(false),
                turn_active: AtomicBool::new(false),
            });
            lock(&list.list).push(inner.clone());
            Box::new(FakeSession(inner)) as Box<dyn ClaudeSessionApi>
        });
        services.one_shot = one_shot.clone();
        let services = Arc::new(services);
        let (spawner, rx) = Spawner::inline();
        let mut app = AppState::new(services.clone(), spawner.clone());
        let mut store = PRStore::new(pr_ref(), services.clone(), spawner);
        store.stale_diff_retry_delay = Duration::ZERO;
        app.prs.insert(pr_ref(), store);
        app.current = Some(pr_ref());
        Harness { demo, flaky, services, worktrees, one_shot, sessions, app, rx }
    }

    pub fn flaky(&self) -> &FlakyGitHub {
        self.flaky.as_deref().expect("harness built without flaky GitHub")
    }

    pub fn s(&mut self) -> &mut PRStore {
        self.app.prs.get_mut(&pr_ref()).expect("store")
    }

    /// Applies queued jobs (and the jobs they queue) until none are left.
    pub fn drain(&mut self) {
        let mut n = 0;
        while let Ok(j) = self.rx.try_recv() {
            j(&mut self.app);
            n += 1;
            assert!(n < 100_000, "job loop does not settle");
        }
    }

    /// Takes queued jobs without applying them (to apply later, out of order).
    pub fn take_jobs(&mut self) -> Vec<Job> {
        let mut v = vec![];
        while let Ok(j) = self.rx.try_recv() {
            v.push(j);
        }
        v
    }

    pub fn apply(&mut self, jobs: Vec<Job>) {
        for j in jobs {
            j(&mut self.app);
        }
        self.drain();
    }

    /// Runs `f` on the store, then drains.
    pub fn act<R>(&mut self, f: impl FnOnce(&mut PRStore) -> R) -> R {
        let r = f(self.s());
        self.drain();
        r
    }

    pub fn load(&mut self) {
        self.act(|s| s.load());
    }

    pub fn demo_calls(&self) -> Vec<String> {
        self.demo.calls()
    }
    pub fn demo_count(&self, call: &str) -> usize {
        self.demo.calls().iter().filter(|c| *c == call).count()
    }

    /// `ensureWorktree()`: prepares the checkout and returns the result.
    pub fn ensure_worktree(&mut self) -> Result<Worktree, String> {
        let out = Arc::new(Mutex::new(None));
        let o = out.clone();
        self.act(move |s| s.ensure_worktree_then(move |_, r| *lock(&o) = Some(r)));
        let r = lock(&out).take().expect("ensure_worktree_then did not finish");
        r
    }

    /// Runs an action taking a `done` callback, drains, and returns what `done` got.
    pub fn await_done<T: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut PRStore, Box<dyn FnOnce(&mut PRStore, T) + Send>),
    ) -> T {
        let out = Arc::new(Mutex::new(None));
        let o = out.clone();
        self.act(move |s| f(s, Box::new(move |_, v| *lock(&o) = Some(v))));
        let r = lock(&out).take().expect("action did not call done");
        r
    }

    /// Runs `f` on the Discuss controller `id`, then drains.
    pub fn ctl<R>(&mut self, id: &str, f: impl FnOnce(&mut crate::discuss::DiscussController, &mut PRStore) -> R) -> R {
        let r = self.s().with_controller(id, f).expect("no such discuss thread");
        self.drain();
        r
    }

    pub fn controller(&mut self, id: &str) -> &crate::discuss::DiscussController {
        self.s().controller(id)
    }

    /// Consumes the scroll the canvas would perform, moving the top row there like the view does.
    pub fn take_scroll(&mut self) -> Option<usize> {
        let s = self.s();
        let r = s.pending_scroll.take();
        if let Some(r) = r {
            s.top_visible_row = r;
        }
        r
    }

    pub fn path(&mut self, f: usize) -> String {
        self.s().files()[f].path().to_string()
    }

    pub fn anchor(&mut self, f: usize, line: i64) -> CodeAnchor {
        CodeAnchor::new(self.path(f), DiffSide::Right, line, line)
    }

    /// Paths with Discuss threads (Swift's `discussPaths`).
    pub fn discuss_paths(&mut self) -> Vec<String> {
        let mut v: Vec<String> = self.s().discuss_threads.values().map(|t| t.anchor.path.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.worktrees.dir);
    }
}

/// First addition line (new number) in file `f`.
pub fn first_addition(s: &PRStore, f: usize) -> i64 {
    s.files()[f].lines.iter().find(|l| l.kind == LineKind::Addition).unwrap().new_line as i64
}
