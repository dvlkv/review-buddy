//! State and actions for one open pull request.
//!
//! Async flows from the Swift app become: do the blocking part on a worker thread
//! (`Env` carries everything a worker needs), then post a job that applies the result
//! to this store on the UI thread. A load generation guards against stale results.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use review_buddy_core::claude::session::{ClaudeOneShotRunning, SessionOutput};
use review_buddy_core::diff::layout::{
    DiffLayout, DiffLayoutOptions, DiffViewMode, InlineBlockKind, InlineBlockRef, RowKind,
};
use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::diff::split::{SplitAligner, SplitPair};
use review_buddy_core::git::worktree::{CommitInfo, Worktree};
use review_buddy_core::github::client::{GitHubAPI, GitHubError, MergeResult};
use review_buddy_core::highlight::lexer::{DiffHighlighter, SyntaxToken};
use review_buddy_core::persistence::PRLocalState;
use review_buddy_core::review::keybindings::ReviewAction;
use review_buddy_core::support::perf::Perf;
use review_buddy_core::*;

use crate::discuss::DiscussController;
use crate::file_tree::FileTreeBuilder;
use crate::rt::{job, Job, Spawner};
use crate::services::{Services, WorktreeProviding};

// MARK: - Small types

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffSelection {
    pub file: usize,
    pub path: String,
    pub side: DiffSide,
    /// Line where the drag started; `end` follows the mouse.
    pub start: i64,
    pub end: i64,
    /// Byte offsets on the `start` / `end` lines for a text selection made by dragging in
    /// the code; None when whole lines are selected (gutter, keyboard).
    pub start_column: Option<usize>,
    pub end_column: Option<usize>,
}

impl DiffSelection {
    /// Comments and discussions still anchor on whole lines: GitHub has no character ranges.
    pub fn anchor(&self) -> CodeAnchor {
        CodeAnchor::new(self.path.clone(), self.side, self.start, self.end)
    }
    pub fn contains(&self, side: DiffSide, line: i64) -> bool {
        side == self.side && line >= self.start.min(self.end) && line <= self.start.max(self.end)
    }
    pub fn is_partial(&self) -> bool {
        self.start_column.is_some() && self.end_column.is_some()
    }
    /// Selected bytes on `line`, or None for a whole-line selection. The upper bound is
    /// `usize::MAX` when the selection continues past the end of the line.
    pub fn column_range(&self, line: i64) -> Option<std::ops::Range<usize>> {
        let (sc, ec) = (self.start_column?, self.end_column?);
        let (lo, lo_col, hi, hi_col) =
            if (self.start, sc) <= (self.end, ec) { (self.start, sc, self.end, ec) } else { (self.end, ec, self.start, sc) };
        if line < lo || line > hi {
            return None;
        }
        let a = if line == lo { lo_col } else { 0 };
        let b = if line == hi { hi_col } else { usize::MAX };
        Some(a..a.max(b))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerKind {
    NewThread(CodeAnchor),
    Reply { thread_id: String },
    Edit { thread_id: String, comment_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    pub id: String,
    pub kind: ComposerKind,
    pub text: String,
    pub is_submitting: bool,
    pub error: Option<String>,
    /// Lines under the anchor, used for "Insert suggestion" and previews.
    pub anchor_lines: Vec<String>,
    /// Set when the draft came from an AI finding or Discuss conversation.
    pub origin: Option<String>,
    /// Ask the view to focus the text box once.
    pub wants_focus: bool,
}

impl Composer {
    pub fn new(kind: ComposerKind, text: impl Into<String>) -> Self {
        Composer {
            id: new_id(),
            kind,
            text: text.into(),
            is_submitting: false,
            error: None,
            anchor_lines: vec![],
            origin: None,
            wants_focus: true,
        }
    }
    pub fn anchor(&self) -> Option<&CodeAnchor> {
        match &self.kind {
            ComposerKind::NewThread(a) => Some(a),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading(String),
    Loaded,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TaskState {
    #[default]
    Idle,
    Running(String),
    Failed(String),
}

impl TaskState {
    pub fn is_running(&self) -> bool {
        matches!(self, TaskState::Running(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum WorktreeState {
    #[default]
    Idle,
    Preparing,
    Ready(Worktree),
    Failed(String),
}

impl WorktreeState {
    pub fn worktree(&self) -> Option<&Worktree> {
        match self {
            WorktreeState::Ready(w) => Some(w),
            _ => None,
        }
    }
}

/// The PR's worktree, shared with worker threads so concurrent requests for it
/// (summary, Discuss, large-diff load) prepare it once.
#[derive(Default)]
pub struct WorktreeCell {
    state: Mutex<WorktreeState>,
    cv: Condvar,
}

impl WorktreeCell {
    pub fn state(&self) -> WorktreeState {
        self.state.lock().unwrap().clone()
    }
    pub fn set(&self, s: WorktreeState) {
        *self.state.lock().unwrap() = s;
        self.cv.notify_all();
    }
    /// Returns the ready worktree, waits for one being prepared, or prepares it with `make`.
    /// The bool is true when this call created it.
    pub fn ensure(&self, make: impl FnOnce() -> Result<Worktree, String>) -> Result<(Worktree, bool), String> {
        let mut g = self.state.lock().unwrap();
        loop {
            match &*g {
                WorktreeState::Ready(w) => return Ok((w.clone(), false)),
                WorktreeState::Preparing => g = self.cv.wait(g).unwrap(),
                _ => break,
            }
        }
        *g = WorktreeState::Preparing;
        drop(g);
        let r = make();
        let mut g = self.state.lock().unwrap();
        *g = match &r {
            Ok(w) => WorktreeState::Ready(w.clone()),
            Err(e) => WorktreeState::Failed(e.clone()),
        };
        drop(g);
        self.cv.notify_all();
        r.map(|w| (w, true))
    }
}

/// Syntax tokens per file, highlighted on first draw (big files on a worker thread).
#[derive(Default)]
pub struct HighlightCache {
    tokens: Vec<Option<Arc<Vec<Vec<SyntaxToken>>>>>,
    pending: HashSet<usize>,
    generation: u64,
}

const BACKGROUND_HIGHLIGHT_LINES: usize = 4000;

/// Everything a worker thread needs to act for this PR.
#[derive(Clone)]
pub struct Env {
    pub pr_ref: PRRef,
    pub github: Arc<dyn GitHubAPI>,
    pub worktrees: Option<Arc<dyn WorktreeProviding>>,
    pub wt: Arc<WorktreeCell>,
    pub spawner: Spawner,
    pub local_clone: Option<PathBuf>,
    pub one_shot: Arc<dyn ClaudeOneShotRunning>,
    pub generation: Arc<AtomicU64>,
}

impl Env {
    /// Prepares (or reuses) the PR's worktree. When this call created it, the store
    /// refreshes its git state and remaps Discuss anchors.
    pub fn ensure_worktree(&self, detail: &PRDetail) -> Result<Worktree, String> {
        let w = self.worktrees.clone().ok_or_else(|| "git was not found".to_string())?;
        let clone = self.local_clone.clone();
        let r = self.wt.ensure(|| w.ensure_worktree(detail, clone.as_deref()));
        self.spawner.repaint();
        match r {
            Ok((wt, created)) => {
                if created {
                    self.spawner.post(with_pr(&self.pr_ref, |s| {
                        s.refresh_git_state(false);
                        s.remap_discuss_threads();
                    }));
                }
                Ok(wt)
            }
            Err(e) => Err(e),
        }
    }

    pub fn post(&self, f: impl FnOnce(&mut PRStore) + Send + 'static) {
        self.spawner.post(with_pr(&self.pr_ref, f));
    }
}

/// A job that runs `f` on the store for `r`, if it is still open.
pub fn with_pr(r: &PRRef, f: impl FnOnce(&mut PRStore) + Send + 'static) -> Job {
    let r = r.clone();
    job(move |app| {
        if let Some(s) = app.prs.get_mut(&r) {
            f(s);
        }
    })
}

/// Splits file text into lines the way the diff numbers them: a final newline ends the
/// last line rather than starting an empty one. "\r\n" endings lose their "\r".
pub fn split_lines(data: &[u8]) -> Vec<String> {
    let mut lines: Vec<String> = data
        .split(|&b| b == b'\n')
        .map(|l| String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)).into_owned())
        .collect();
    if lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

// MARK: - Store

pub struct PRStore {
    pub pr_ref: PRRef,
    pub services: Arc<Services>,
    pub spawner: Spawner,

    // Loaded data
    pub detail: Option<PRDetail>,
    pub diff: Option<Arc<DiffSet>>,
    pub layout: Option<DiffLayout>,
    pub layout_version: u64,
    pub load_state: LoadState,
    pub diff_source: String,

    // View state
    pub options: DiffLayoutOptions,
    pub selection: Option<DiffSelection>,
    pub current_file: usize,
    pub file_filter: String,
    /// The Summary screen is showing instead of the diff. Every jump to code switches back.
    pub show_overview: bool,
    /// The user's "Review first" list; None follows the AI summary's reading order.
    pub custom_review_order: Option<Vec<String>>,
    pub banner: Option<String>,
    /// Written by the diff canvas every frame.
    pub top_visible_row: usize,
    /// Consumed by the diff canvas: scroll so this row is at the top.
    pub pending_scroll: Option<usize>,
    /// Asks the canvas to take keyboard focus.
    pub wants_focus: bool,

    // Review
    pub threads: HashMap<String, ReviewThread>,
    pub open_thread_count_by_path: HashMap<String, usize>,
    pub composers: HashMap<String, Composer>,
    pub pending_review_id: Option<String>,
    pub viewed: HashSet<String>,
    pub review_body: String,
    pub submit_state: TaskState,

    // AI
    pub discuss_threads: HashMap<String, DiscussThread>,
    pub controllers: HashMap<String, DiscussController>,
    pub active_discuss_id: Option<String>,
    pub findings: HashMap<String, AIFinding>,
    /// PR head the current findings were produced for.
    pub findings_head_sha: Option<String>,
    /// The review thread last reached with ]/[ — target of `r` (reply).
    pub focused_thread_id: Option<String>,
    pub summary: Option<AISummary>,
    pub summary_state: TaskState,
    pub pre_review_state: TaskState,
    /// Bumped to cancel a running summary / pre-review (its late result is dropped).
    pub summary_token: u64,
    pub pre_review_token: u64,
    pub fix_requests: Vec<FixRequest>,

    // Git
    pub wt: Arc<WorktreeCell>,
    pub local_commits: Vec<CommitInfo>,
    pub remote_ahead: usize,
    /// PR head on GitHub as of the last update check; differs from `detail.head_oid` once pushed to.
    pub remote_head_oid: Option<String>,
    last_update_check: Option<Instant>,
    pub push_state: TaskState,

    pub highlights: HighlightCache,
    /// Side-by-side pairing for every file, computed off the UI thread after load.
    pub split_pairs: Option<Arc<Vec<Vec<SplitPair>>>>,
    pub file_content_cache: HashMap<String, Vec<String>>,
    file_content_loads: HashSet<String>,
    /// Expand requests waiting for a file's content.
    pending_expands: Vec<(usize, Expand)>,
    /// Wait before re-asking GitHub for a diff that doesn't match the PR's files.
    pub stale_diff_retry_delay: Duration,
    generation: Arc<AtomicU64>,
}

#[derive(Clone, Copy, Debug)]
enum Expand {
    Above { hunk: usize, by: i64 },
    Below { by: i64 },
}

impl PRStore {
    pub fn new(pr_ref: PRRef, services: Arc<Services>, spawner: Spawner) -> Self {
        let settings = services.settings();
        let mut options = DiffLayoutOptions::with_mode(settings.view_mode);
        options.large_file_threshold = settings.large_file_threshold.max(1) as usize;
        options.show_expand_footer = true;
        PRStore {
            pr_ref,
            services,
            spawner,
            detail: None,
            diff: None,
            layout: None,
            layout_version: 0,
            load_state: LoadState::Idle,
            diff_source: String::new(),
            options,
            selection: None,
            current_file: 0,
            file_filter: String::new(),
            show_overview: false,
            custom_review_order: None,
            banner: None,
            top_visible_row: 0,
            pending_scroll: None,
            wants_focus: false,
            threads: HashMap::new(),
            open_thread_count_by_path: HashMap::new(),
            composers: HashMap::new(),
            pending_review_id: None,
            viewed: HashSet::new(),
            review_body: String::new(),
            submit_state: TaskState::Idle,
            discuss_threads: HashMap::new(),
            controllers: HashMap::new(),
            active_discuss_id: None,
            findings: HashMap::new(),
            findings_head_sha: None,
            focused_thread_id: None,
            summary: None,
            summary_state: TaskState::Idle,
            pre_review_state: TaskState::Idle,
            summary_token: 0,
            pre_review_token: 0,
            fix_requests: vec![],
            wt: Arc::new(WorktreeCell::default()),
            local_commits: vec![],
            remote_ahead: 0,
            remote_head_oid: None,
            last_update_check: None,
            push_state: TaskState::Idle,
            highlights: HighlightCache::default(),
            split_pairs: None,
            file_content_cache: HashMap::new(),
            file_content_loads: HashSet::new(),
            pending_expands: vec![],
            stale_diff_retry_delay: Duration::from_millis(1500),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn env(&self) -> Env {
        Env {
            pr_ref: self.pr_ref.clone(),
            github: self.services.github.clone(),
            worktrees: self.services.worktrees.clone(),
            wt: self.wt.clone(),
            spawner: self.spawner.clone(),
            local_clone: self.services.local_clone(&self.pr_ref.name_with_owner()),
            one_shot: self.services.one_shot.clone(),
            generation: self.generation.clone(),
        }
    }

    pub fn worktree_state(&self) -> WorktreeState {
        self.wt.state()
    }

    pub fn worktree(&self) -> Option<Worktree> {
        self.wt.state().worktree().cloned()
    }

    pub fn files(&self) -> &[FileDiff] {
        self.diff.as_ref().map(|d| d.files.as_slice()).unwrap_or(&[])
    }

    // MARK: - Loading

    /// Loads (or reloads) the PR. Overlapping calls are fine: only the latest applies
    /// its results, so a slow earlier response can't overwrite a newer diff.
    pub fn load(&mut self) {
        self.load_then(|_| {});
    }

    /// `load`, then `then` once the new diff is installed (or the load failed).
    pub fn load_then(&mut self, then: impl FnOnce(&mut PRStore) + Send + 'static) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.load_state = LoadState::Loading("Loading pull request…".into());
        let local = self.services.store.load(&self.pr_ref);
        self.custom_review_order = local.review_order.clone();
        self.apply_local(local);
        let env = self.env();
        let retry_delay = self.stale_diff_retry_delay;
        let start = Instant::now();
        let then: Box<dyn FnOnce(&mut PRStore) + Send> = Box::new(then);
        let then = Arc::new(Mutex::new(Some(then)));
        let run_then = move |s: &mut PRStore| {
            if let Some(f) = then.lock().unwrap().take() {
                f(s);
            }
        };
        self.spawner.spawn(move || {
            let superseded = || env.generation.load(Ordering::SeqCst) != generation;
            let (detail, diff) = std::thread::scope(|sc| {
                let d = sc.spawn(|| env.github.diff(&env.pr_ref));
                let detail = env.github.pull_request(&env.pr_ref);
                (detail, d.join().unwrap_or(Err(GitHubError::InvalidResponse)))
            });
            let fail = |e: String| -> Job {
                let run_then = run_then.clone();
                with_pr(&env.pr_ref, move |s| {
                    if s.generation.load(Ordering::SeqCst) == generation {
                        s.load_state = LoadState::Failed(e);
                    }
                    run_then(s);
                })
            };
            let detail = match detail {
                Ok(d) => d,
                Err(e) => return fail(e.to_string()),
            };
            if superseded() {
                return job(|_| {});
            }
            // Show the PR's header and threads while the diff is still on its way.
            {
                let d = detail.clone();
                env.post(move |s| {
                    if s.generation.load(Ordering::SeqCst) == generation {
                        s.apply_detail(d);
                    }
                });
            }
            let mut source = "GitHub API".to_string();
            let data = match diff {
                Ok(d) => d,
                Err(GitHubError::DiffTooLarge) => {
                    env.post(|s| s.load_state = LoadState::Loading("Large diff — preparing local checkout…".into()));
                    let existed = env.wt.state().worktree().is_some();
                    let wt = match env.ensure_worktree(&detail) {
                        Ok(w) => w,
                        Err(e) => return fail(e),
                    };
                    let w = env.worktrees.clone().unwrap();
                    // A checkout made earlier has the head it was made with; get the current one.
                    if existed {
                        if let Err(e) = w.fetch_pr(&wt.repo_dir, &detail) {
                            return fail(e);
                        }
                    }
                    // Pinned to the head `detail` describes; the branch tip may have moved on since.
                    let base = format!("refs/review-buddy/base-{}", env.pr_ref.number);
                    source = "local git".into();
                    match w.diff(&wt.repo_dir, &base, &detail.head_oid) {
                        Ok(d) => d,
                        Err(_) => match w.diff(&wt.repo_dir, &base, &wt.remote_head_ref) {
                            Ok(d) => d,
                            Err(e) => return fail(e),
                        },
                    }
                }
                Err(e) => return fail(e.to_string()),
            };
            let mut parsed = Perf::shared().measure("diff.parse", || UnifiedDiffParser::parse_bytes(data));
            if superseded() {
                return job(|_| {});
            }
            // Right after a push GitHub's diff endpoint can still serve the previous head
            // while GraphQL already has the new one. Their file lists then disagree: retry once.
            if source == "GitHub API" && !diff_matches(&parsed, &detail) {
                std::thread::sleep(retry_delay);
                if let Ok(again) = env.github.diff(&env.pr_ref) {
                    parsed = UnifiedDiffParser::parse_bytes(again);
                }
            }
            let diff = Arc::new(parsed);
            let pairs: Vec<Vec<SplitPair>> = diff.files.iter().map(SplitAligner::align_file).collect();
            with_pr(&env.pr_ref, move |s| {
                if s.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                s.apply_detail(detail);
                s.diff_source = source;
                s.install_diff(diff);
                s.split_pairs = Some(Arc::new(pairs));
                s.apply_file_order();
                Perf::shared().record("pr.load.total", start.elapsed().as_secs_f64() * 1000.0);
                s.load_state = LoadState::Loaded;
                s.remap_discuss_threads();
                s.clean_up_worktree_if_closed();
                s.auto_run_ai();
                run_then(s);
            })
        });
    }

    fn apply_detail(&mut self, detail: PRDetail) {
        self.remote_head_oid = None;
        self.pending_review_id = detail.pending_review_id.clone();
        self.set_threads(detail.threads.iter().map(|t| (t.id.clone(), t.clone())).collect());
        self.viewed = detail.files.iter().filter(|f| f.viewed == ViewedState::Viewed).map(|f| f.path.clone()).collect();
        self.detail = Some(detail);
    }

    /// Installs a diff directly (synthetic mode, tests) with an optional detail.
    pub fn install(&mut self, diff: DiffSet, detail: Option<PRDetail>) {
        if let Some(d) = detail {
            self.apply_detail(d);
        }
        self.install_diff(Arc::new(diff));
        self.load_state = LoadState::Loaded;
    }

    fn install_diff(&mut self, diff: Arc<DiffSet>) {
        // Everything keyed by file/hunk index or holding file text belongs to the old diff.
        self.layout = None;
        self.split_pairs = None;
        self.file_content_cache.clear();
        self.file_content_loads.clear();
        self.pending_expands.clear();
        self.options.expanded_above.clear();
        self.options.expanded_below.clear();
        self.options.force_loaded.clear();
        self.selection = None;
        self.reset_highlights(diff.files.len());
        // Viewed files start collapsed, like GitHub.
        self.options.collapsed =
            (0..diff.files.len()).filter(|&i| self.viewed.contains(diff.files[i].path())).collect();
        self.diff = Some(diff);
        self.rebuild_layout(false);
        self.load_state = LoadState::Loaded;
    }

    pub fn apply_local(&mut self, local: PRLocalState) {
        self.discuss_threads = local.discuss_threads.into_iter().map(|t| (t.id.clone(), t)).collect();
        self.summary = local.summary;
        self.findings_head_sha = local.pre_review.as_ref().and_then(|p| p.head_sha.clone());
        self.findings = local.pre_review.map(|p| p.findings).unwrap_or_default().into_iter().map(|f| (f.id.clone(), f)).collect();
        self.fix_requests = local
            .fix_requests
            .into_iter()
            .map(|mut f| {
                if matches!(f.status, FixStatus::Running | FixStatus::Queued) {
                    f.status = FixStatus::Failed;
                    f.error = Some("Interrupted".into());
                }
                f
            })
            .collect();
    }

    /// Reloads the PR from GitHub and brings the local checkout up to date with it.
    pub fn refresh(&mut self) {
        // Rows are renumbered by the new diff: come back to the same file, same distance in.
        let anchor = self.layout.as_ref().and_then(|l| {
            let top = self.top_visible_row;
            if top >= l.row_count() {
                return None;
            }
            let f = l.file_index_for_row(top);
            self.files().get(f).map(|fd| (fd.path().to_string(), top - l.file_start_row[f]))
        });
        self.load_then(move |s| {
            if let (Some((path, off)), Some(l), Some(d)) = (anchor, s.layout.as_ref(), s.diff.as_ref()) {
                if let Some(f) = d.file_index(&path) {
                    let r = l.row_range_for_file(f);
                    s.pending_scroll = Some((r.start + off).min(r.end.saturating_sub(1)));
                }
            }
            s.refresh_git_state(true);
        });
    }

    /// New commits on GitHub that this view (diff, checkout) doesn't have yet.
    pub fn is_outdated(&self) -> bool {
        match (&self.remote_head_oid, &self.detail) {
            (Some(r), Some(d)) => *r != d.head_oid,
            _ => false,
        }
    }

    /// Polls GitHub's PR head (cheap) so the UI can offer a refresh; at most every 30 s unless forced.
    pub fn check_for_updates(&mut self, force: bool) {
        self.check_for_updates_then(force, |_| {});
    }

    fn check_for_updates_then(&mut self, force: bool, then: impl FnOnce(&mut PRStore) + Send + 'static) {
        if self.load_state != LoadState::Loaded || self.detail.is_none() {
            return;
        }
        if !force && self.last_update_check.map(|t| t.elapsed() < Duration::from_secs(30)).unwrap_or(false) {
            return;
        }
        self.last_update_check = Some(Instant::now());
        let env = self.env();
        self.spawner.spawn(move || {
            let oid = env.github.head_oid(&env.pr_ref);
            with_pr(&env.pr_ref, move |s| {
                if let Ok(oid) = oid {
                    if s.load_state == LoadState::Loaded && s.remote_head_oid.as_ref() != Some(&oid) {
                        s.remote_head_oid = Some(oid);
                    }
                }
                then(s);
            })
        });
    }

    /// Reloads when GitHub's PR head is now the checkout's HEAD, i.e. it was pushed from
    /// here (e.g. `git push` run by a Discuss agent) and the diff still shows the old head.
    pub fn refresh_if_pushed_from_checkout(&mut self) {
        let (Some(wt), Some(w)) = (self.worktree(), self.services.worktrees.clone()) else { return };
        self.check_for_updates_then(true, move |s| {
            if !s.is_outdated() {
                return;
            }
            let remote = s.remote_head_oid.clone();
            let env = s.env();
            s.spawner.spawn(move || {
                let head = w.head_sha(&wt).ok();
                with_pr(&env.pr_ref, move |s| {
                    if head.is_some() && head == remote {
                        s.refresh();
                    }
                })
            });
        });
    }

    pub fn set_threads(&mut self, threads: HashMap<String, ReviewThread>) {
        self.threads = threads;
        self.recompute_thread_counts();
    }

    pub fn recompute_thread_counts(&mut self) {
        let mut counts = HashMap::new();
        for t in self.threads.values().filter(|t| !t.is_resolved) {
            *counts.entry(t.path.clone()).or_insert(0) += 1;
        }
        self.open_thread_count_by_path = counts;
    }

    // MARK: - Layout

    pub fn blocks_by_file(&self) -> HashMap<usize, Vec<InlineBlockRef>> {
        let mut out: HashMap<usize, Vec<InlineBlockRef>> = HashMap::new();
        let Some(diff) = &self.diff else { return out };
        let mut add = |path: &str, b: InlineBlockRef| {
            if let Some(f) = diff.file_index(path) {
                out.entry(f).or_default().push(b);
            }
        };
        let mut threads: Vec<&ReviewThread> = self.threads.values().collect();
        threads.sort_by(|a, b| {
            let ka = a.comments.first().map(|c| c.created_at);
            let kb = b.comments.first().map(|c| c.created_at);
            ka.cmp(&kb).then(a.id.cmp(&b.id))
        });
        for t in threads {
            // Outdated threads have no current line; -1 sends them to the end of the file.
            add(&t.path, InlineBlockRef::new(t.id.clone(), InlineBlockKind::ReviewThread, t.side, t.line.unwrap_or(-1)));
        }
        let mut findings: Vec<&AIFinding> = self.findings.values().filter(|f| f.state == FindingState::Open).collect();
        findings.sort_by(|a, b| (a.severity, &a.path, a.start_line).cmp(&(b.severity, &b.path, b.start_line)));
        for f in findings {
            add(&f.path, InlineBlockRef::new(f.id.clone(), InlineBlockKind::Finding, f.side, f.end_line));
        }
        let mut ds: Vec<&DiscussThread> = self.discuss_threads.values().collect();
        ds.sort_by_key(|d| d.created_at);
        for d in ds {
            let line = if d.status == DiscussStatus::Outdated { -1 } else { d.anchor.end_line };
            add(&d.anchor.path, InlineBlockRef::new(d.id.clone(), InlineBlockKind::Discuss, d.anchor.side, line));
        }
        let mut cs: Vec<&Composer> = self.composers.values().collect();
        cs.sort_by(|a, b| a.id.cmp(&b.id));
        for c in cs {
            if let Some(a) = c.anchor() {
                add(&a.path, InlineBlockRef::new(c.id.clone(), InlineBlockKind::Composer, a.side, a.end_line));
            }
        }
        out
    }

    pub fn rebuild_layout(&mut self, _preserve_scroll: bool) {
        let Some(diff) = self.diff.clone() else { return };
        if let Some(l) = &self.layout {
            if l.options.mode == DiffViewMode::Split && !l.split_pairs.is_empty() {
                self.split_pairs = Some(l.split_pairs.clone());
            }
        }
        let reuse = self.split_pairs.clone().filter(|p| p.len() == diff.files.len());
        let blocks = self.blocks_by_file();
        let options = self.options.clone();
        self.layout = Some(Perf::shared().measure("layout.build", || DiffLayout::with_blocks(diff, options, &blocks, reuse)));
        self.layout_version += 1;
    }

    pub fn set_mode(&mut self, mode: DiffViewMode) {
        if self.options.mode == mode {
            return;
        }
        self.options.mode = mode;
        self.services.update_settings(|s| s.view_mode = mode);
        self.rebuild_layout(true);
    }

    pub fn view_mode(&self) -> DiffViewMode {
        self.options.mode
    }

    pub fn apply_settings(&mut self) {
        let s = self.services.settings();
        self.options.large_file_threshold = s.large_file_threshold.max(1) as usize;
        self.options.mode = s.view_mode;
        self.rebuild_layout(true);
    }

    pub fn toggle_mode(&mut self) {
        let m = if self.options.mode == DiffViewMode::Split { DiffViewMode::Unified } else { DiffViewMode::Split };
        self.set_mode(m);
    }

    pub fn toggle_collapsed(&mut self, file: usize) {
        if !self.options.collapsed.remove(&file) {
            self.options.collapsed.insert(file);
        }
        self.rebuild_layout(true);
    }

    pub fn force_load(&mut self, file: usize) {
        self.options.force_loaded.insert(file);
        self.options.collapsed.remove(&file);
        self.rebuild_layout(true);
    }

    pub fn report_top_row(&mut self, row: usize) {
        self.top_visible_row = row;
        if let Some(l) = &self.layout {
            if row < l.row_count() {
                self.current_file = l.file_index_for_row(row);
            }
        }
    }

    // MARK: - Viewed

    pub fn is_viewed(&self, file: usize) -> bool {
        self.files().get(file).map(|f| self.viewed.contains(f.path())).unwrap_or(false)
    }

    pub fn toggle_viewed(&mut self, file: usize) {
        let Some(path) = self.files().get(file).map(|f| f.path().to_string()) else { return };
        let now_viewed = !self.viewed.contains(&path);
        if now_viewed {
            self.viewed.insert(path.clone());
            self.options.collapsed.insert(file);
        } else {
            self.viewed.remove(&path);
            self.options.collapsed.remove(&file);
        }
        self.rebuild_layout(true);
        if now_viewed {
            if let Some(start) = self.layout.as_ref().and_then(|l| l.file_start_row.get(file).copied()) {
                if start < self.top_visible_row {
                    self.pending_scroll = Some(start);
                }
            }
        }
        let Some(pr_id) = self.detail.as_ref().map(|d| d.node_id.clone()) else { return };
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.github.set_viewed(&pr_id, &path, now_viewed);
            with_pr(&env.pr_ref, move |s| {
                if let Err(e) = r {
                    if now_viewed {
                        s.viewed.remove(&path);
                    } else {
                        s.viewed.insert(path);
                    }
                    s.banner = Some(format!("Couldn't update viewed state: {e}"));
                }
            })
        });
    }

    pub fn viewed_count(&self) -> usize {
        self.files().iter().filter(|f| self.viewed.contains(f.path())).count()
    }

    // MARK: - Expand context

    /// Lines between hunk `hunk` and whatever is shown above it that are still hidden.
    pub fn hidden_above(&self, file: usize, hunk: usize) -> i64 {
        let Some(f) = self.files().get(file) else { return 0 };
        let Some(h) = f.hunks.get(hunk) else { return 0 };
        let prev_end = if hunk > 0 { f.hunks[hunk - 1].new_end() as i64 } else { 1 };
        let expanded = self.options.expanded_above.get(&DiffLayoutOptions::key(file, hunk)).copied().unwrap_or(0);
        (h.new_first() as i64 - expanded - prev_end).max(0)
    }

    pub fn can_expand_above(&self, file: usize, hunk: usize) -> bool {
        self.hidden_above(file, hunk) > 0
    }

    pub fn can_expand_below(&self, file: usize) -> bool {
        let Some(f) = self.files().get(file) else { return false };
        let Some(last) = f.hunks.last() else { return false };
        if f.status == FileStatus::Deleted {
            return false;
        }
        if let Some(lines) = self.file_content_cache.get(f.path()) {
            let shown = last.new_end() as i64 - 1 + self.options.expanded_below.get(&file).copied().unwrap_or(0);
            return shown < lines.len() as i64;
        }
        true
    }

    pub fn expand_above(&mut self, file: usize, hunk: usize, by: i64) {
        if self.diff.is_none() || self.hidden_above(file, hunk) <= 0 {
            return;
        }
        self.request_expand(file, Expand::Above { hunk, by });
    }

    pub fn expand_below(&mut self, file: usize, by: i64) {
        if self.diff.is_none() || !self.can_expand_below(file) {
            return;
        }
        self.request_expand(file, Expand::Below { by });
    }

    fn request_expand(&mut self, file: usize, e: Expand) {
        let Some(path) = self.files().get(file).map(|f| f.path().to_string()) else { return };
        if self.file_content_cache.contains_key(&path) {
            self.apply_expand(file, e);
        } else {
            self.pending_expands.push((file, e));
            self.ensure_file_content(file);
        }
    }

    fn apply_expand(&mut self, file: usize, e: Expand) {
        match e {
            Expand::Above { hunk, by } => {
                // Re-check: a second click may have closed the gap already.
                let more = by.min(self.hidden_above(file, hunk));
                if more <= 0 {
                    return;
                }
                *self.options.expanded_above.entry(DiffLayoutOptions::key(file, hunk)).or_insert(0) += more;
            }
            Expand::Below { by } => {
                let f = &self.files()[file];
                let (Some(lines), Some(last)) = (self.file_content_cache.get(f.path()), f.hunks.last()) else { return };
                let shown_end = last.new_end() as i64 - 1 + self.options.expanded_below.get(&file).copied().unwrap_or(0);
                let more = by.min(lines.len() as i64 - shown_end);
                if more <= 0 {
                    return;
                }
                *self.options.expanded_below.entry(file).or_insert(0) += more;
            }
        }
        self.rebuild_layout(true);
    }

    /// Loads the head version of `file` (checkout first, then GitHub) for expanded context.
    pub fn ensure_file_content(&mut self, file: usize) {
        let (Some(detail), Some(diff)) = (self.detail.clone(), self.diff.clone()) else { return };
        let Some(f) = diff.files.get(file) else { return };
        let path = f.path().to_string();
        let new_path = f.new_path.clone();
        if self.file_content_cache.contains_key(&path) {
            return;
        }
        let key = format!("{}:{}", detail.head_oid, path);
        if !self.file_content_loads.insert(key.clone()) {
            return;
        }
        let env = self.env();
        let wt = self.worktree();
        self.spawner.spawn(move || {
            let mut lines = None;
            if let (Some(wt), Some(w)) = (&wt, &env.worktrees) {
                if let Ok(d) = w.file_contents(&wt.repo_dir, &detail.head_oid, &new_path) {
                    lines = Some(split_lines(&d));
                }
            }
            let mut error = None;
            if lines.is_none() {
                // No checkout, or it doesn't have this commit yet (pushed since the last fetch).
                let owner: Vec<String> = detail
                    .head_repo_name_with_owner
                    .clone()
                    .map(|s| s.split('/').map(str::to_string).collect())
                    .filter(|v: &Vec<String>| v.len() == 2)
                    .unwrap_or_else(|| vec![env.pr_ref.owner.clone(), env.pr_ref.repo.clone()]);
                match env.github.file_contents(&owner[0], &owner[1], &new_path, &detail.head_oid) {
                    Ok(d) => lines = Some(split_lines(&d)),
                    Err(e) => error = Some(e.to_string()),
                }
            }
            with_pr(&env.pr_ref, move |s| {
                s.file_content_loads.remove(&key);
                // The PR was reloaded meanwhile: this content may belong to an older head.
                let same = s.diff.as_ref().map(|d| Arc::ptr_eq(d, &diff)).unwrap_or(false)
                    && s.detail.as_ref().map(|d| d.head_oid == detail.head_oid).unwrap_or(false);
                if let Some(e) = error {
                    s.banner = Some(format!("Couldn't load {path}: {e}"));
                }
                let Some(lines) = lines else {
                    s.pending_expands.retain(|(f, _)| *f != file);
                    return;
                };
                if !same {
                    return;
                }
                s.file_content_cache.insert(path, lines);
                let pending: Vec<_> = s.pending_expands.iter().filter(|(f, _)| *f == file).map(|(_, e)| *e).collect();
                s.pending_expands.retain(|(f, _)| *f != file);
                for e in pending {
                    s.apply_expand(file, e);
                }
                s.layout_version += 1;
            })
        });
    }

    // MARK: - Highlighting

    fn reset_highlights(&mut self, files: usize) {
        let g = self.highlights.generation + 1;
        self.highlights = HighlightCache { tokens: vec![None; files], pending: HashSet::new(), generation: g };
    }

    /// Tokens for `file`, highlighting small files now and big ones in the background.
    pub fn tokens(&mut self, file: usize) -> Option<Arc<Vec<Vec<SyntaxToken>>>> {
        let diff = self.diff.clone()?;
        if let Some(Some(t)) = self.highlights.tokens.get(file) {
            return Some(t.clone());
        }
        let f = diff.files.get(file)?;
        if f.lines.len() <= BACKGROUND_HIGHLIGHT_LINES {
            let t = Arc::new(DiffHighlighter::highlight(f, &diff));
            if file < self.highlights.tokens.len() {
                self.highlights.tokens[file] = Some(t.clone());
            }
            return Some(t);
        }
        if self.highlights.pending.insert(file) {
            let generation = self.highlights.generation;
            let r = self.pr_ref.clone();
            self.spawner.spawn(move || {
                let t = Arc::new(DiffHighlighter::highlight(&diff.files[file], &diff));
                with_pr(&r, move |s| {
                    if s.highlights.generation == generation && file < s.highlights.tokens.len() {
                        s.highlights.tokens[file] = Some(t);
                        s.highlights.pending.remove(&file);
                    }
                })
            });
        }
        None
    }

    // MARK: - Selection

    /// `column` makes it a character selection (drag in the code); None selects whole lines.
    pub fn select(&mut self, file: usize, side: DiffSide, line: i64, extend: bool, column: Option<usize>) {
        if file >= self.files().len() || line <= 0 {
            return;
        }
        if extend {
            if let Some(s) = &mut self.selection {
                if s.file == file && s.side == side {
                    s.end = line;
                    if s.start_column.is_some() {
                        s.end_column = Some(column.unwrap_or(usize::MAX));
                    }
                    return;
                }
            }
        }
        let path = self.files()[file].path().to_string();
        self.selection = Some(DiffSelection { file, path, side, start: line, end: line, start_column: column, end_column: column });
    }

    /// A click in the code that didn't drag: nothing is selected.
    pub fn is_empty_text_selection(&self) -> bool {
        match &self.selection {
            Some(s) => s.is_partial() && s.start == s.end && s.start_column == s.end_column,
            None => false,
        }
    }

    /// The selected text: whole lines, or just the dragged-over characters.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let a = sel.anchor();
        let texts = self.lines_for(&a);
        if !sel.is_partial() || texts.len() as i64 != a.end_line - a.start_line + 1 {
            return Some(texts.join("\n"));
        }
        let parts: Vec<String> = (a.start_line..=a.end_line)
            .zip(texts.iter())
            .map(|(n, t)| match sel.column_range(n) {
                None => t.clone(),
                Some(r) => {
                    let lo = floor_char(t, r.start.min(t.len()));
                    let hi = floor_char(t, r.end.min(t.len()));
                    t[lo..hi.max(lo)].to_string()
                }
            })
            .collect();
        Some(parts.join("\n"))
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Text of the lines in an anchor, from the diff (and expanded context).
    pub fn lines_for(&self, anchor: &CodeAnchor) -> Vec<String> {
        let Some(diff) = &self.diff else { return vec![] };
        let Some(fi) = diff.file_index(&anchor.path) else { return vec![] };
        let f = &diff.files[fi];
        let mut by_line: HashMap<i64, String> = HashMap::new();
        for l in f.lines.iter().filter(|l| l.kind != LineKind::NoNewline) {
            let n = l.line_number(anchor.side) as i64;
            let on_side = if anchor.side == DiffSide::Right { l.kind != LineKind::Deletion } else { l.kind != LineKind::Addition };
            if n >= anchor.start_line && n <= anchor.end_line && on_side {
                by_line.insert(n, diff.text_of(l).to_string());
            }
        }
        if anchor.side == DiffSide::Right {
            if let Some(content) = self.file_content_cache.get(&anchor.path) {
                for n in anchor.start_line..=anchor.end_line {
                    if !by_line.contains_key(&n) && n >= 1 && n as usize <= content.len() {
                        by_line.insert(n, content[n as usize - 1].clone());
                    }
                }
            }
        }
        (anchor.start_line..=anchor.end_line).filter_map(|n| by_line.remove(&n)).collect()
    }

    // MARK: - Comments

    pub fn start_comment(&mut self, anchor: Option<CodeAnchor>, text: &str, origin: Option<&str>) {
        let Some(anchor) = anchor.or_else(|| self.selection.as_ref().map(|s| s.anchor())) else { return };
        // Reuse an open composer on the same anchor.
        if let Some(existing) = self.composers.values_mut().find(|c| c.anchor() == Some(&anchor)) {
            if !text.is_empty() {
                existing.text = text.to_string();
            }
            existing.wants_focus = true;
            return;
        }
        let mut c = Composer::new(ComposerKind::NewThread(anchor.clone()), text);
        c.anchor_lines = self.lines_for(&anchor);
        c.origin = origin.map(str::to_string);
        self.composers.insert(c.id.clone(), c);
        self.selection = None;
        self.rebuild_layout(true);
    }

    /// `r`: reply to the focused thread (reached with ]/[), else the first thread on screen.
    pub fn reply_to_focused_thread(&mut self) {
        if let Some(id) = self.focused_thread_id.clone() {
            if self.threads.contains_key(&id) {
                self.start_reply(&id);
                return;
            }
        }
        let Some(l) = &self.layout else { return };
        let found = l.rows.iter().skip(self.top_visible_row).find_map(|r| {
            if r.kind != RowKind::Block {
                return None;
            }
            let b = &l.blocks[r.a as usize];
            (b.kind == InlineBlockKind::ReviewThread).then(|| b.id.clone())
        });
        if let Some(id) = found {
            self.start_reply(&id);
        }
    }

    pub fn start_reply(&mut self, thread_id: &str) {
        if self.composer_for_reply(thread_id).is_some() {
            return;
        }
        let c = Composer::new(ComposerKind::Reply { thread_id: thread_id.into() }, "");
        self.composers.insert(c.id.clone(), c);
        self.rebuild_layout(true);
    }

    pub fn start_edit(&mut self, thread_id: &str, comment: &ReviewComment) {
        let c = Composer::new(
            ComposerKind::Edit { thread_id: thread_id.into(), comment_id: comment.id.clone() },
            comment.body.clone(),
        );
        self.composers.insert(c.id.clone(), c);
        self.rebuild_layout(true);
    }

    pub fn composer_for_reply(&self, thread_id: &str) -> Option<&Composer> {
        self.composers.values().find(|c| matches!(&c.kind, ComposerKind::Reply { thread_id: t } if t == thread_id))
    }

    pub fn composer_for_edit(&self, comment_id: &str) -> Option<&Composer> {
        self.composers.values().find(|c| matches!(&c.kind, ComposerKind::Edit { comment_id: x, .. } if x == comment_id))
    }

    pub fn update_composer(&mut self, id: &str, text: String) {
        if let Some(c) = self.composers.get_mut(id) {
            c.text = text;
        }
    }

    pub fn cancel_composer(&mut self, id: &str) {
        self.composers.remove(id);
        self.rebuild_layout(true);
    }

    /// Posts the composer. `single` publishes it right away instead of adding it to the
    /// pending review (only offered while there is no pending review, as on GitHub).
    pub fn submit_composer(&mut self, id: &str, single: bool) {
        let Some(c) = self.composers.get_mut(id) else { return };
        if c.text.trim().is_empty() || c.is_submitting {
            return;
        }
        c.is_submitting = true;
        c.error = None;
        let c = c.clone();
        let Some(detail) = self.detail.clone() else { return };
        let pending = self.pending_review_id.clone();
        let first_db_id = match &c.kind {
            ComposerKind::Reply { thread_id } => {
                self.threads.get(thread_id).and_then(|t| t.comments.first()).and_then(|c| c.database_id)
            }
            _ => None,
        };
        let env = self.env();
        let id = id.to_string();
        self.spawner.spawn(move || {
            let gh = &env.github;
            let ensure_review = |pending: &Option<String>| -> Result<(String, bool), GitHubError> {
                match pending {
                    Some(id) => Ok((id.clone(), false)),
                    None => gh.start_review(&detail.node_id).map(|id| (id, true)),
                }
            };
            #[allow(clippy::large_enum_variant)]
            enum Done {
                Reload(PRDetail),
                Thread(ReviewThread, Option<String>),
                Reply(String, ReviewComment, Option<String>),
                Edited(String, ReviewComment),
            }
            let r: Result<Done, GitHubError> = (|| match &c.kind {
                ComposerKind::NewThread(anchor) if single => {
                    gh.add_single_comment(&env.pr_ref, &detail.head_oid, anchor, &c.text)?;
                    Ok(Done::Reload(gh.pull_request(&env.pr_ref)?))
                }
                ComposerKind::Reply { thread_id } if single => match first_db_id {
                    Some(db) => {
                        gh.reply_single(&env.pr_ref, thread_id, db, &c.text)?;
                        Ok(Done::Reload(gh.pull_request(&env.pr_ref)?))
                    }
                    // No REST id (shouldn't happen for GitHub data): a reply outside a review.
                    None => Ok(Done::Reply(thread_id.clone(), gh.reply(thread_id, None, &c.text)?, None)),
                },
                ComposerKind::NewThread(anchor) => {
                    let (review, new) = ensure_review(&pending)?;
                    let t = gh.add_thread(&detail.node_id, &review, anchor, &c.text)?;
                    Ok(Done::Thread(t, new.then_some(review)))
                }
                ComposerKind::Reply { thread_id } => {
                    let (review, new) = ensure_review(&pending)?;
                    let comment = gh.reply(thread_id, Some(&review), &c.text)?;
                    Ok(Done::Reply(thread_id.clone(), comment, new.then_some(review)))
                }
                ComposerKind::Edit { thread_id, comment_id } => {
                    Ok(Done::Edited(thread_id.clone(), gh.update_comment(comment_id, &c.text)?))
                }
            })();
            with_pr(&env.pr_ref, move |s| match r {
                Ok(done) => {
                    match done {
                        Done::Reload(d) => s.apply_reloaded_threads(d),
                        Done::Thread(t, review) => {
                            if review.is_some() {
                                s.pending_review_id = review;
                            }
                            s.threads.insert(t.id.clone(), t);
                            s.recompute_thread_counts();
                        }
                        Done::Reply(tid, comment, review) => {
                            if review.is_some() {
                                s.pending_review_id = review;
                            }
                            if let Some(t) = s.threads.get_mut(&tid) {
                                t.comments.push(comment);
                            }
                        }
                        Done::Edited(tid, updated) => {
                            if let Some(t) = s.threads.get_mut(&tid) {
                                if let Some(c) = t.comments.iter_mut().find(|c| c.id == updated.id) {
                                    *c = updated;
                                }
                            }
                        }
                    }
                    s.composers.remove(&id);
                    s.rebuild_layout(true);
                }
                Err(e) => {
                    if let Some(c) = s.composers.get_mut(&id) {
                        c.is_submitting = false;
                        c.error = Some(e.to_string());
                    }
                }
            })
        });
    }

    /// Applies a fresh fetch after a single comment: its thread, and the conversation.
    fn apply_reloaded_threads(&mut self, d: PRDetail) {
        self.set_threads(d.threads.iter().map(|t| (t.id.clone(), t.clone())).collect());
        if let Some(det) = &mut self.detail {
            det.threads = d.threads.clone();
        }
        self.apply_conversation(&d);
    }

    fn apply_conversation(&mut self, d: &PRDetail) {
        if let Some(det) = &mut self.detail {
            if let Some(c) = &d.conversation {
                det.conversation = Some(c.clone());
            }
            if let Some(s) = &d.status {
                det.status = Some(s.clone());
            }
        }
    }

    pub fn delete_comment(&mut self, thread_id: &str, comment_id: &str) {
        let env = self.env();
        let (tid, cid) = (thread_id.to_string(), comment_id.to_string());
        self.spawner.spawn(move || {
            let r = env.github.delete_comment(&cid);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => {
                    let empty = if let Some(t) = s.threads.get_mut(&tid) {
                        t.comments.retain(|c| c.id != cid);
                        t.comments.is_empty()
                    } else {
                        false
                    };
                    if empty {
                        s.threads.remove(&tid);
                    }
                    s.recompute_thread_counts();
                    s.rebuild_layout(true);
                }
                Err(e) => s.banner = Some(format!("Couldn't delete comment: {e}")),
            })
        });
    }

    pub fn set_resolved(&mut self, thread_id: &str, resolved: bool) {
        let before = self.threads.get(thread_id).map(|t| t.is_resolved);
        if let Some(t) = self.threads.get_mut(thread_id) {
            t.is_resolved = resolved;
        }
        self.recompute_thread_counts();
        self.layout_version += 1;
        let env = self.env();
        let tid = thread_id.to_string();
        self.spawner.spawn(move || {
            let r = env.github.set_resolved(&tid, resolved);
            with_pr(&env.pr_ref, move |s| {
                match r {
                    Ok(v) => {
                        if let Some(t) = s.threads.get_mut(&tid) {
                            t.is_resolved = v;
                        }
                    }
                    Err(e) => {
                        if let Some(t) = s.threads.get_mut(&tid) {
                            t.is_resolved = before.unwrap_or(!resolved);
                        }
                        s.banner = Some(format!("Couldn't update thread: {e}"));
                    }
                }
                s.recompute_thread_counts();
                s.layout_version += 1;
            })
        });
    }

    pub fn pending_comment_count(&self) -> usize {
        self.threads.values().map(|t| t.comments.iter().filter(|c| c.is_pending).count()).sum()
    }

    /// Submits the pending review; `done(true)` on success.
    pub fn submit_review(&mut self, event: ReviewEvent, done: impl FnOnce(&mut PRStore, bool) + Send + 'static) {
        self.submit_state = TaskState::Running("Submitting…".into());
        let Some(detail) = self.detail.clone() else { return };
        let pending = self.pending_review_id.clone();
        let body = self.review_body.clone();
        let env = self.env();
        self.spawner.spawn(move || {
            let r = (|| {
                let id = match pending {
                    Some(id) => id,
                    None => env.github.start_review(&detail.node_id)?,
                };
                env.github.submit_review(&id, event, &body)
            })();
            // Show the review in the Summary's history and update the review status.
            let fresh = if r.is_ok() { env.github.pull_request(&env.pr_ref).ok() } else { None };
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => {
                    s.pending_review_id = None;
                    s.review_body.clear();
                    for t in s.threads.values_mut() {
                        for c in &mut t.comments {
                            c.is_pending = false;
                        }
                    }
                    s.submit_state = TaskState::Idle;
                    s.banner = Some(if event == ReviewEvent::Approve { "Approved ✓" } else { "Review submitted" }.into());
                    if let Some(d) = fresh {
                        s.apply_conversation(&d);
                    }
                    s.layout_version += 1;
                    done(s, true);
                }
                Err(e) => {
                    s.submit_state = TaskState::Failed(e.to_string());
                    done(s, false);
                }
            })
        });
    }

    pub fn discard_pending_review(&mut self) {
        let Some(id) = self.pending_review_id.clone() else { return };
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.github.delete_review(&id);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => {
                    s.pending_review_id = None;
                    s.submit_state = TaskState::Idle;
                    s.threads.retain(|_, t| !t.is_pending());
                    for t in s.threads.values_mut() {
                        t.comments.retain(|c| !c.is_pending);
                    }
                    s.recompute_thread_counts();
                    s.rebuild_layout(true);
                }
                Err(e) => s.banner = Some(format!("Couldn't discard review: {e}")),
            })
        });
    }

    // MARK: - Navigation

    pub fn navigate(&mut self, action: ReviewAction) {
        let Some(l) = &self.layout else { return };
        let top = self.top_visible_row;
        let rows = &l.rows;
        let target: Option<usize> = match action {
            ReviewAction::NextFile => {
                let f = l.file_index_for_row(top);
                l.file_start_row.get(f + 1).copied()
            }
            ReviewAction::PreviousFile => {
                let f = l.file_index_for_row(top);
                let start = l.file_start_row[f];
                if top > start { Some(start) } else if f > 0 { Some(l.file_start_row[f - 1]) } else { None }
            }
            ReviewAction::NextHunk => (top + 1..rows.len()).find(|&i| rows[i].kind == RowKind::HunkHeader),
            ReviewAction::PreviousHunk => (0..top.min(rows.len())).rev().find(|&i| rows[i].kind == RowKind::HunkHeader),
            ReviewAction::NextThread | ReviewAction::PreviousThread => {
                let t = if action == ReviewAction::NextThread {
                    (top + 4..rows.len()).find(|&i| rows[i].kind == RowKind::Block)
                } else {
                    (0..(top + 3).min(rows.len())).rev().find(|&i| rows[i].kind == RowKind::Block)
                };
                if let Some(r) = t {
                    let b = &l.blocks[rows[r].a as usize];
                    self.focused_thread_id = (b.kind == InlineBlockKind::ReviewThread).then(|| b.id.clone());
                }
                t.map(|r| r.saturating_sub(3))
            }
            _ => None,
        };
        if let Some(r) = target {
            self.pending_scroll = Some(r);
        }
    }

    // MARK: - Review order

    /// Files shown first, in order: the user's list, else the AI reading order. Only paths in the diff.
    pub fn review_order(&self) -> Vec<String> {
        let wanted: Vec<String> = match &self.custom_review_order {
            Some(o) => o.clone(),
            None => self.summary.as_ref().map(|s| s.reading_order.iter().map(|r| r.path.clone()).collect()).unwrap_or_default(),
        };
        let mut seen = HashSet::new();
        wanted
            .into_iter()
            .filter(|p| self.diff.as_ref().map(|d| d.file_index(p).is_some()).unwrap_or(false) && seen.insert(p.clone()))
            .collect()
    }

    pub fn review_first_count(&self) -> usize {
        self.review_order().len()
    }

    pub fn add_to_review_order(&mut self, path: &str) {
        let mut o: Vec<String> = self.review_order().into_iter().filter(|p| p != path).collect();
        o.push(path.into());
        self.set_review_order(o);
    }

    pub fn remove_from_review_order(&mut self, path: &str) {
        let o = self.review_order().into_iter().filter(|p| p != path).collect();
        self.set_review_order(o);
    }

    pub fn move_in_review_order(&mut self, path: &str, delta: i64) {
        let mut o = self.review_order();
        let Some(i) = o.iter().position(|p| p == path) else { return };
        let j = (i as i64 + delta).clamp(0, o.len() as i64 - 1) as usize;
        if i == j {
            return;
        }
        let p = o.remove(i);
        o.insert(j, p);
        self.set_review_order(o);
    }

    /// Back to the AI summary's reading order.
    pub fn reset_review_order(&mut self) {
        self.custom_review_order = None;
        self.persist();
        self.apply_file_order();
    }

    fn set_review_order(&mut self, paths: Vec<String>) {
        self.custom_review_order = Some(paths);
        self.persist();
        self.apply_file_order();
    }

    /// Reorders the diff's files to `review_order` + tree order. Index-keyed view state
    /// (collapsed, expanded context, split pairs) moves with its file.
    pub fn apply_file_order(&mut self) {
        let Some(old) = self.diff.clone() else { return };
        let paths: Vec<String> = old.files.iter().map(|f| f.path().to_string()).collect();
        let priority: Vec<usize> = self.review_order().iter().filter_map(|p| old.file_index(p)).collect();
        let order = FileTreeBuilder::display_order(&paths, &priority);
        if order.iter().copied().eq(0..paths.len()) || order.len() != paths.len() {
            return;
        }
        let mut new_index = vec![0usize; order.len()];
        for (n, &o) in order.iter().enumerate() {
            new_index[o] = n;
        }
        // Stay on the file being read; from the very top, show the new first files.
        let top_path = if self.current_file > 0 { self.files().get(self.current_file).map(|f| f.path().to_string()) } else { None };

        self.options.collapsed = self.options.collapsed.iter().map(|&i| new_index[i]).collect();
        self.options.force_loaded = self.options.force_loaded.iter().map(|&i| new_index[i]).collect();
        self.options.expanded_below = self.options.expanded_below.iter().map(|(&k, &v)| (new_index[k], v)).collect();
        self.options.expanded_above = self
            .options
            .expanded_above
            .iter()
            .map(|(&k, &v)| (DiffLayoutOptions::key(new_index[(k >> 32) as usize], (k & 0xFFFF_FFFF) as usize), v))
            .collect();
        self.split_pairs = self
            .split_pairs
            .take()
            .filter(|p| p.len() == order.len())
            .map(|p| Arc::new(order.iter().map(|&o| p[o].clone()).collect()));
        // File-keyed caches move too.
        let old_tokens = std::mem::take(&mut self.highlights.tokens);
        self.highlights.generation += 1;
        self.highlights.pending.clear();
        self.highlights.tokens = order.iter().map(|&o| old_tokens.get(o).cloned().flatten()).collect();
        self.selection = None;
        let new = Arc::new(DiffSet::new(old.buffer.clone(), order.iter().map(|&o| old.files[o].clone()).collect()));
        self.diff = Some(new.clone());
        self.layout = None; // its split pairs are in the old order
        self.rebuild_layout(false);
        let row = top_path
            .and_then(|p| new.file_index(&p))
            .and_then(|f| self.layout.as_ref().and_then(|l| l.file_start_row.get(f).copied()))
            .unwrap_or(0);
        self.pending_scroll = Some(row);
    }

    // MARK: - Conversation, merge, close

    /// Posts a PR comment (GitHub's Conversation tab). `done` gets an error message on failure.
    pub fn add_conversation_comment(&mut self, body: &str, done: impl FnOnce(&mut PRStore, Option<String>) + Send + 'static) {
        let text = body.trim().to_string();
        let Some(d) = self.detail.clone() else { return };
        if text.is_empty() {
            return;
        }
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.github.add_comment(&d.node_id, &text);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(c) => {
                    if let Some(det) = &mut s.detail {
                        det.conversation.get_or_insert_with(Default::default).comments.push(c);
                    }
                    done(s, None);
                }
                Err(e) => done(s, Some(e.to_string())),
            })
        });
    }

    /// Merges the PR on GitHub, then reloads it.
    pub fn merge_pull_request(
        &mut self,
        method: MergeMethod,
        bypass_rules: bool,
        done: impl FnOnce(&mut PRStore, Option<String>) + Send + 'static,
    ) {
        let Some(d) = self.detail.clone() else { return };
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.github.merge_pull_request(&env.pr_ref, &d.node_id, method, &d.head_oid, bypass_rules);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(m) => {
                    match m {
                        MergeResult::Merged => {}
                        MergeResult::Enqueued => s.banner = Some("Added to the merge queue".into()),
                        MergeResult::Pending => {
                            s.banner = Some(format!("GitHub is still merging #{}; refresh in a moment", s.pr_ref.number))
                        }
                    }
                    s.refresh();
                    done(s, None);
                }
                Err(e) => done(s, Some(e.to_string())),
            })
        });
    }

    /// Closes or reopens the PR on GitHub, then reloads it.
    pub fn set_closed(&mut self, closed: bool, done: impl FnOnce(&mut PRStore, Option<String>) + Send + 'static) {
        let Some(d) = self.detail.clone() else { return };
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.github.set_pull_request_closed(&d.node_id, closed);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => {
                    s.refresh();
                    done(s, None);
                }
                Err(e) => done(s, Some(e.to_string())),
            })
        });
    }

    // MARK: - Scrolling

    pub fn scroll_to_file(&mut self, file: usize) {
        self.show_overview = false;
        if let Some(r) = self.layout.as_ref().and_then(|l| l.file_start_row.get(file).copied()) {
            self.pending_scroll = Some(r);
        }
    }

    pub fn scroll_to_block(&mut self, id: &str) {
        self.show_overview = false;
        let Some(l) = &self.layout else { return };
        let Some(bi) = l.blocks.iter().position(|b| b.id == id) else { return };
        if let Some(r) = l.rows.iter().position(|r| r.kind == RowKind::Block && r.a == bi as i32) {
            self.pending_scroll = Some(r.saturating_sub(4));
        }
    }

    pub fn scroll_to_anchor(&mut self, anchor: &CodeAnchor) {
        self.show_overview = false;
        let Some(f) = self.diff.as_ref().and_then(|d| d.file_index(&anchor.path)) else { return };
        if self.options.collapsed.remove(&f) {
            self.rebuild_layout(true);
        }
        let Some(l) = &self.layout else { return };
        if let Some(r) = l.row(f, anchor.side, anchor.start_line).or_else(|| l.file_start_row.get(f).copied()) {
            self.pending_scroll = Some(r.saturating_sub(4));
        }
    }

    // MARK: - Worktree

    /// Prepares the checkout in the background; `then` runs with the result.
    pub fn ensure_worktree_then(&mut self, then: impl FnOnce(&mut PRStore, Result<Worktree, String>) + Send + 'static) {
        if let Some(wt) = self.worktree() {
            then(self, Ok(wt));
            return;
        }
        // Failures are delivered as a job, not synchronously: callers run inside
        // `with_controller`, where a nested `with_controller` would find no controller.
        let Some(detail) = self.detail.clone() else {
            let e = GitHubError::NotFound.to_string();
            self.spawner.post(with_pr(&self.pr_ref, move |s| then(s, Err(e))));
            return;
        };
        if self.services.worktrees.is_none() {
            self.spawner.post(with_pr(&self.pr_ref, move |s| then(s, Err("git was not found".into()))));
            return;
        }
        let env = self.env();
        self.spawner.spawn(move || {
            let r = env.ensure_worktree(&detail);
            with_pr(&env.pr_ref, move |s| then(s, r))
        });
    }

    /// Spec §5.1: when the PR is merged or closed, its worktree is removed unless it
    /// holds unpushed fix commits.
    pub fn clean_up_worktree_if_closed(&mut self) {
        let Some(detail) = self.detail.clone() else { return };
        if detail.state != "MERGED" && detail.state != "CLOSED" {
            return;
        }
        let Some(w) = self.services.worktrees.clone() else { return };
        // Never pull the checkout out from under a Claude run that is preparing/using it.
        if self.worktree_state() != WorktreeState::Idle {
            return;
        }
        let env = self.env();
        self.spawner.spawn(move || {
            if let Ok(Some(wt)) = w.ensure_existing_worktree(&detail, env.local_clone.as_deref()) {
                let has_commits = w.local_commits(&wt).map(|c| !c.is_empty()).unwrap_or(true);
                if !has_commits && env.wt.state() == WorktreeState::Idle {
                    let _ = w.remove(&wt);
                }
            }
            job(|_| {})
        });
    }

    /// Fetches the PR into the checkout. With `update_checkout`, also fast-forwards it to the
    /// new head when that loses nothing (no local fix commits, no uncommitted changes).
    pub fn refresh_git_state(&mut self, update_checkout: bool) {
        self.refresh_git_state_then(update_checkout, |_| {});
    }

    pub fn refresh_git_state_then(&mut self, update_checkout: bool, then: impl FnOnce(&mut PRStore) + Send + 'static) {
        let (Some(wt), Some(w)) = (self.worktree(), self.services.worktrees.clone()) else {
            then(self);
            return;
        };
        let detail = self.detail.clone();
        let env = self.env();
        self.spawner.spawn(move || {
            if let Some(d) = &detail {
                if update_checkout {
                    let _ = w.ensure_worktree(d, env.local_clone.as_deref());
                } else {
                    let _ = w.fetch_pr(&wt.repo_dir, d);
                }
            }
            let commits = w.local_commits(&wt).unwrap_or_default();
            let ahead = w.remote_ahead(&wt).unwrap_or(0);
            with_pr(&env.pr_ref, move |s| {
                s.local_commits = commits;
                s.remote_ahead = ahead;
                then(s);
            })
        });
    }

    pub fn push(&mut self, done: impl FnOnce(&mut PRStore, bool) + Send + 'static) {
        let (Some(wt), Some(w), Some(detail)) = (self.worktree(), self.services.worktrees.clone(), self.detail.clone()) else {
            done(self, false);
            return;
        };
        self.push_state = TaskState::Running("Pushing…".into());
        let env = self.env();
        self.spawner.spawn(move || {
            let r = w.push(&wt, &detail);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => {
                    s.push_state = TaskState::Idle;
                    let n = s.local_commits.len();
                    s.banner = Some(format!("Pushed {n} commit{} to {}", if n == 1 { "" } else { "s" }, detail.head_ref_name));
                    // The PR's head is now these commits: reload so the diff shows them.
                    s.refresh();
                    done(s, true);
                }
                Err(e) => {
                    s.push_state = TaskState::Failed(e);
                    done(s, false);
                }
            })
        });
    }

    pub fn rebase_onto_remote(&mut self) {
        let (Some(wt), Some(w)) = (self.worktree(), self.services.worktrees.clone()) else { return };
        let env = self.env();
        self.spawner.spawn(move || {
            let r = w.rebase_onto_remote(&wt);
            with_pr(&env.pr_ref, move |s| match r {
                Ok(()) => s.refresh_git_state(false),
                Err(e) => s.banner = Some(format!("Rebase failed (aborted): {e}")),
            })
        });
    }

    pub fn can_push(&self) -> bool {
        self.detail.as_ref().map(|d| d.can_push_fixes()).unwrap_or(false)
    }

    pub fn push_disabled_reason(&self) -> Option<String> {
        let Some(d) = &self.detail else { return Some("Not loaded".into()) };
        if d.can_push_fixes() {
            return None;
        }
        Some(if d.is_from_fork() {
            "This PR comes from a fork and the author doesn't allow edits from maintainers.".into()
        } else {
            format!("You don't have push access to {}.", self.pr_ref.name_with_owner())
        })
    }

    // MARK: - Persistence

    pub fn persist(&self) {
        let mut ds: Vec<DiscussThread> = self.discuss_threads.values().cloned().collect();
        ds.sort_by_key(|d| d.created_at);
        let mut findings: Vec<AIFinding> = self.findings.values().cloned().collect();
        findings.sort_by(|a, b| a.id.cmp(&b.id));
        let state = PRLocalState {
            discuss_threads: ds,
            fix_requests: self.fix_requests.clone(),
            summary: self.summary.clone(),
            pre_review: Some(PreReviewResult {
                findings,
                head_sha: self.findings_head_sha.clone().or_else(|| self.detail.as_ref().map(|d| d.head_oid.clone())),
                created_at: Some(Utc::now()),
            }),
            last_seen_head: self.detail.as_ref().map(|d| d.head_oid.clone()),
            review_order: self.custom_review_order.clone(),
        };
        self.services.store.save(&self.pr_ref, state);
    }

    pub fn update_discuss_thread(&mut self, t: DiscussThread) {
        self.discuss_threads.insert(t.id.clone(), t);
        self.persist();
    }

    pub fn findings_are_outdated(&self) -> bool {
        match (&self.findings_head_sha, &self.detail) {
            (Some(h), Some(d)) => *h != d.head_oid,
            _ => false,
        }
    }

    /// Routes a Claude session's output to its Discuss controller.
    pub fn handle_session_output(&mut self, thread_id: &str, out: SessionOutput) {
        let Some(mut c) = self.controllers.remove(thread_id) else { return };
        c.handle(self, out);
        self.controllers.insert(thread_id.to_string(), c);
    }
}

/// Whether the diff covers the files GitHub lists for the PR (unknown lists match).
pub fn diff_matches(diff: &DiffSet, detail: &PRDetail) -> bool {
    if detail.files.is_empty() || detail.files.len() as i64 != detail.changed_files {
        return true;
    }
    let a: HashSet<&str> = diff.files.iter().map(|f| f.path()).collect();
    let b: HashSet<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
    a == b
}

pub fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
