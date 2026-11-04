//! Discuss threads, the fix queue, the AI summary and the AI pre-review.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use review_buddy_core::claude::prompts;
use review_buddy_core::claude::session::{ClaudeConfig, ClaudeError};
use review_buddy_core::diff::line_mapper::LineMapper;
use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::*;

use crate::discuss::DiscussController;
use crate::pr_store::{with_pr, PRStore, TaskState};

impl PRStore {
    // MARK: Discuss

    /// Creates a Discuss thread on the selection (or anchor) and opens it in the panel.
    pub fn start_discuss(&mut self, anchor: Option<CodeAnchor>, seed: Option<String>) -> Option<String> {
        let anchor = anchor.or_else(|| self.selection.as_ref().map(|s| s.anchor()))?;
        let detail = self.detail.clone()?;
        let mut seed = seed;
        // A character selection points at something specific: start the question with it.
        if seed.is_none() {
            if let (Some(sel), Some(text)) = (&self.selection, self.selected_text()) {
                let t = text.trim();
                if sel.is_partial() && !t.is_empty() && !t.contains('\n') {
                    seed = Some(format!("`{t}` "));
                }
            }
        }
        let lines = self.lines_for(&anchor);
        let sha = if anchor.side == DiffSide::Right { detail.head_oid.clone() } else { detail.base_oid.clone() };
        let t = DiscussThread::new(self.pr_ref.clone(), anchor, sha, DiscussThread::hash_lines(&lines));
        let id = t.id.clone();
        self.update_discuss_thread(t);
        self.selection = None;
        self.active_discuss_id = Some(id.clone());
        self.rebuild_layout(true);
        let c = self.controller(&id);
        if let Some(seed) = seed {
            c.input = seed;
        }
        c.wants_focus = true;
        // Warm the checkout while the user types their question.
        self.ensure_worktree_then(|_, _| {});
        Some(id)
    }

    pub fn controller(&mut self, id: &str) -> &mut DiscussController {
        if !self.controllers.contains_key(id) {
            let t = self.discuss_threads[id].clone();
            self.controllers.insert(id.to_string(), DiscussController::new(&t));
        }
        self.controllers.get_mut(id).unwrap()
    }

    /// Runs `f` with the controller for `id` and the store (the controller is taken out
    /// of the map meanwhile so both can be borrowed mutably).
    pub fn with_controller<R>(&mut self, id: &str, f: impl FnOnce(&mut DiscussController, &mut PRStore) -> R) -> Option<R> {
        if !self.discuss_threads.contains_key(id) {
            return None;
        }
        self.controller(id);
        let mut c = self.controllers.remove(id)?;
        let r = f(&mut c, self);
        // A deleted thread must not get its controller back.
        if self.discuss_threads.contains_key(id) {
            self.controllers.insert(id.to_string(), c);
        } else {
            c.shutdown();
        }
        Some(r)
    }

    pub fn delete_discuss(&mut self, id: &str) {
        if let Some(c) = self.controllers.remove(id) {
            c.shutdown();
        }
        self.discuss_threads.remove(id);
        if self.active_discuss_id.as_deref() == Some(id) {
            self.active_discuss_id = None;
        }
        self.persist();
        self.rebuild_layout(true);
    }

    pub fn sorted_discuss_threads(&self) -> Vec<DiscussThread> {
        let mut v: Vec<DiscussThread> = self.discuss_threads.values().cloned().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    /// Moves Discuss anchors to the current head; marks them outdated if their lines changed.
    pub fn remap_discuss_threads(&mut self) {
        let Some(detail) = self.detail.clone() else { return };
        if self.diff.is_none() {
            return;
        }
        let candidates: Vec<DiscussThread> = self
            .discuss_threads
            .values()
            .filter(|t| t.anchor.side == DiffSide::Right && t.commit_sha != detail.head_oid && t.status == DiscussStatus::Active)
            .cloned()
            .collect();
        for t in candidates {
            // Fast path: the anchored text is still at the same lines.
            let lines = self.lines_for(&t.anchor);
            if !lines.is_empty() && DiscussThread::hash_lines(&lines) == t.anchor_text_hash {
                let mut u = t.clone();
                u.commit_sha = detail.head_oid.clone();
                self.update_discuss_thread(u);
                continue;
            }
            let (Some(wt), Some(w)) = (self.worktree(), self.services.worktrees.clone()) else { continue };
            let env = self.env();
            let head = detail.head_oid.clone();
            self.spawner.spawn(move || {
                let data = w.diff_path(&wt.repo_dir, &t.commit_sha, &head, &t.anchor.path).ok();
                let parsed = data.map(UnifiedDiffParser::parse_bytes);
                let fd = parsed.as_ref().and_then(|d| d.files.first());
                let mut u = t.clone();
                match LineMapper::new(fd).map_range(t.anchor.start_line..=t.anchor.end_line) {
                    Some(r) => {
                        u.anchor = CodeAnchor::new(t.anchor.path.clone(), DiffSide::Right, *r.start(), *r.end());
                        u.commit_sha = head;
                    }
                    None => u.status = DiscussStatus::Outdated,
                }
                with_pr(&env.pr_ref, move |s| {
                    if s.discuss_threads.contains_key(&u.id) {
                        s.update_discuss_thread(u);
                        s.rebuild_layout(true);
                    }
                })
            });
        }
        self.rebuild_layout(true);
    }

    // MARK: Fix queue (one fix at a time per PR, since threads share the worktree)

    pub fn is_fix_running(&self, except: &str) -> bool {
        self.fix_requests.iter().any(|f| f.status == FixStatus::Running && f.thread_id != except)
    }

    pub fn enqueue_fix(&mut self, thread_id: &str, anchor: CodeAnchor, start: bool) -> String {
        let mut r = FixRequest::new(thread_id, anchor, prompts::FIX_INSTRUCTION);
        if start {
            r.status = FixStatus::Running;
        }
        let id = r.id.clone();
        self.fix_requests.push(r);
        self.persist();
        id
    }

    pub fn mark_fix(&mut self, id: &str, status: FixStatus, commits: Vec<String>, error: Option<String>) {
        let Some(f) = self.fix_requests.iter_mut().find(|f| f.id == id) else { return };
        f.status = status;
        if !commits.is_empty() {
            f.commits = commits;
        }
        f.error = error;
        self.persist();
    }

    pub fn run_next_queued_fix(&mut self) {
        if self.fix_requests.iter().any(|f| f.status == FixStatus::Running) {
            return;
        }
        let Some(next) = self.fix_requests.iter().find(|f| f.status == FixStatus::Queued).cloned() else { return };
        self.with_controller(&next.thread_id, |c, s| c.run_fix(s, &next.id));
    }

    pub fn fix_status(&self, thread_id: &str) -> Option<&FixRequest> {
        self.fix_requests.iter().rev().find(|f| f.thread_id == thread_id)
    }

    // MARK: Summary & pre-review

    pub fn auto_run_ai(&mut self) {
        let settings = self.services.settings();
        let head = self.detail.as_ref().map(|d| d.head_oid.clone());
        if settings.auto_summarize && (self.summary.is_none() || self.summary.as_ref().and_then(|s| s.head_sha.clone()) != head) {
            self.start_summarize();
        }
        let max = settings.auto_pre_review_max_lines;
        if max > 0 {
            if let Some(d) = &self.detail {
                if d.additions + d.deletions <= max && self.findings.is_empty() {
                    self.start_pre_review();
                }
            }
        }
    }

    pub fn set_summary(&mut self, s: Option<AISummary>, state: TaskState) {
        let order = |x: &Option<AISummary>| x.as_ref().map(|s| s.reading_order.clone());
        let changed = order(&s) != order(&self.summary);
        self.summary = s;
        self.summary_state = state;
        // A new reading order moves its files to the top (unless the user set their own).
        if changed && self.custom_review_order.is_none() {
            self.apply_file_order();
        }
    }

    fn one_shot_config(&self, cwd: std::path::PathBuf, detail: &PRDetail, schema: Option<&str>) -> Option<ClaudeConfig> {
        let mut cfg = ClaudeConfig::new(self.services.claude_path.clone()?, cwd);
        cfg.append_system_prompt = Some(prompts::system_prompt(detail));
        cfg.model = self.services.claude_model();
        cfg.permission_mode = "plan".into();
        cfg.json_schema = schema.map(str::to_string);
        Some(cfg)
    }

    /// Starts a summary in the background unless one is already running.
    pub fn start_summarize(&mut self) {
        if self.summary_state.is_running() {
            return;
        }
        let Some(detail) = self.detail.clone() else { return };
        if self.services.claude_path.is_none() {
            let s = self.summary.clone();
            self.set_summary(s, TaskState::Failed(ClaudeError::NotInstalled.to_string()));
            return;
        }
        self.summary_token += 1;
        let token = self.summary_token;
        let s = self.summary.clone();
        self.set_summary(s, TaskState::Running("Preparing checkout…".into()));
        self.ensure_worktree_then(move |s, wt| {
            if s.summary_token != token {
                return;
            }
            let wt = match wt {
                Ok(w) => w,
                Err(e) => {
                    let cur = s.summary.clone();
                    s.set_summary(cur, TaskState::Failed(e));
                    return;
                }
            };
            let cur = s.summary.clone();
            s.set_summary(cur, TaskState::Running("Claude is reading the diff…".into()));
            let Some(cfg) = s.one_shot_config(wt.path.clone(), &detail, Some(prompts::SUMMARY_SCHEMA)) else { return };
            let env = s.env();
            s.spawner.spawn(move || {
                let r = env
                    .one_shot
                    .run(&cfg, &prompts::summary(&detail))
                    .and_then(|res| res.decode_output::<AISummary>());
                with_pr(&env.pr_ref, move |s| {
                    // Cancelled by the user: `cancel_summarize` already reset the state.
                    if s.summary_token != token {
                        return;
                    }
                    match r {
                        Ok(mut sum) => {
                            sum.head_sha = Some(detail.head_oid.clone());
                            sum.created_at = Some(Utc::now());
                            s.set_summary(Some(sum), TaskState::Idle);
                            s.persist();
                        }
                        Err(e) => {
                            let cur = s.summary.clone();
                            s.set_summary(cur, TaskState::Failed(e.to_string()));
                        }
                    }
                })
            });
        });
    }

    pub fn cancel_summarize(&mut self) {
        self.summary_token += 1;
        let s = self.summary.clone();
        self.set_summary(s, TaskState::Idle);
    }

    /// Starts a pre-review in the background unless one is already running.
    pub fn start_pre_review(&mut self) {
        if self.pre_review_state.is_running() {
            return;
        }
        let Some(detail) = self.detail.clone() else { return };
        if self.services.claude_path.is_none() {
            self.pre_review_state = TaskState::Failed(ClaudeError::NotInstalled.to_string());
            return;
        }
        self.pre_review_token += 1;
        let token = self.pre_review_token;
        self.pre_review_state = TaskState::Running("Preparing checkout…".into());
        self.ensure_worktree_then(move |s, wt| {
            if s.pre_review_token != token {
                return;
            }
            let wt = match wt {
                Ok(w) => w,
                Err(e) => {
                    s.pre_review_state = TaskState::Failed(e);
                    return;
                }
            };
            s.pre_review_state = TaskState::Running("Claude is reviewing…".into());
            let Some(cfg) = s.one_shot_config(wt.path.clone(), &detail, Some(prompts::PRE_REVIEW_SCHEMA)) else { return };
            let env = s.env();
            s.spawner.spawn(move || {
                let r = env
                    .one_shot
                    .run(&cfg, &prompts::pre_review(&detail))
                    .and_then(|res| res.decode_output::<PreReviewResult>());
                with_pr(&env.pr_ref, move |s| {
                    if s.pre_review_token != token {
                        return;
                    }
                    match r {
                        Ok(result) => s.apply_pre_review(result.findings),
                        Err(e) => s.pre_review_state = TaskState::Failed(e.to_string()),
                    }
                })
            });
        });
    }

    /// Keeps only findings that land on files in this diff; triaged ones survive a re-run.
    pub fn apply_pre_review(&mut self, new: Vec<AIFinding>) {
        let mut valid: Vec<AIFinding> =
            new.into_iter().filter(|f| self.diff.as_ref().map(|d| d.file_index(&f.path).is_some()).unwrap_or(false)).collect();
        valid.sort_by_key(|f| f.severity);
        let kept: Vec<AIFinding> = self.findings.values().filter(|f| f.state != FindingState::Open).cloned().collect();
        self.findings = kept.into_iter().chain(valid).map(|f| (f.id.clone(), f)).collect::<HashMap<_, _>>();
        self.findings_head_sha = self.detail.as_ref().map(|d| d.head_oid.clone());
        self.pre_review_state = TaskState::Idle;
        self.persist();
        self.rebuild_layout(true);
    }

    pub fn cancel_pre_review(&mut self) {
        self.pre_review_token += 1;
        self.pre_review_state = TaskState::Idle;
    }

    pub fn open_findings(&self) -> Vec<AIFinding> {
        let mut v: Vec<AIFinding> = self.findings.values().filter(|f| f.state == FindingState::Open).cloned().collect();
        v.sort_by(|a, b| (a.severity, &a.path, a.start_line).cmp(&(b.severity, &b.path, b.start_line)));
        v
    }

    fn set_finding_state(&mut self, id: &str, state: FindingState) -> Option<AIFinding> {
        let f = self.findings.get_mut(id)?;
        f.state = state;
        let f = f.clone();
        self.persist();
        Some(f)
    }

    pub fn accept_finding(&mut self, id: &str) {
        if let Some(f) = self.set_finding_state(id, FindingState::Accepted) {
            self.start_comment(Some(f.anchor()), &f.comment_body(), Some("From AI pre-review"));
            self.rebuild_layout(true);
        }
    }

    pub fn dismiss_finding(&mut self, id: &str) {
        if self.set_finding_state(id, FindingState::Dismissed).is_some() {
            self.rebuild_layout(true);
        }
    }

    pub fn discuss_finding(&mut self, id: &str) {
        if let Some(f) = self.set_finding_state(id, FindingState::Discussed) {
            let seed = format!("The AI pre-review flagged this: **{}** — {}\n\nIs this a real problem? ", f.title, f.body);
            self.start_discuss(Some(f.anchor()), Some(seed));
        }
    }
}

pub type SharedDiff = Arc<DiffSet>;
