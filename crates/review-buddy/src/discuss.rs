//! Drives one Discuss thread: a Claude session in the PR worktree plus its transcript.

use review_buddy_core::claude::prompts;
use review_buddy_core::claude::session::{ClaudeConfig, ClaudeError, ClaudeSessionApi, SessionOutput};
use review_buddy_core::claude::stream_events::ClaudeEvent;
use review_buddy_core::claude::transcript::{DiscussMessage, Role, TranscriptReducer};
use review_buddy_core::git::worktree::Worktree;
use review_buddy_core::*;

use crate::pr_store::{split_lines, with_pr, PRStore, TaskState};

pub struct DiscussController {
    pub thread_id: String,
    pub reducer: TranscriptReducer,
    pub status: Option<String>,
    pub draft_state: TaskState,
    pub input: String,
    pub wants_focus: bool,
    session: Option<Box<dyn ClaudeSessionApi>>,
    /// Set while a "Fix it" turn is running: (request id, HEAD before the fix).
    fix_baseline: Option<(String, String)>,
}

impl DiscussController {
    pub fn new(thread: &DiscussThread) -> Self {
        DiscussController {
            thread_id: thread.id.clone(),
            reducer: TranscriptReducer::new(thread.messages.clone(), thread.session_id.clone()),
            status: None,
            draft_state: TaskState::Idle,
            input: String::new(),
            wants_focus: false,
            session: None,
            fix_baseline: None,
        }
    }

    pub fn messages(&self) -> &[DiscussMessage] {
        self.reducer.messages()
    }
    pub fn is_running(&self) -> bool {
        self.reducer.is_running() || self.status.is_some()
    }
    pub fn is_fixing(&self) -> bool {
        self.fix_baseline.is_some()
    }

    // MARK: Sending

    pub fn send(&mut self, pr: &mut PRStore, raw: &str) {
        let text = raw.trim().to_string();
        if text.is_empty() {
            return;
        }
        let (Some(thread), Some(detail)) = (pr.discuss_threads.get(&self.thread_id).cloned(), pr.detail.clone()) else { return };
        let first = self.reducer.messages().is_empty() && self.reducer.session_id().is_none();
        self.reducer.user_sent(&text, None);
        self.save(pr);
        self.status = Some("Preparing checkout…".into());
        // Gathered now, on the UI thread; the surrounding lines are read in the worker.
        let selected = pr.lines_for(&thread.anchor);
        let on_file: Vec<ReviewThread> = pr.threads.values().filter(|t| t.path == thread.anchor.path).cloned().collect();
        let others: Vec<ReviewThread> = pr.threads.values().filter(|t| t.path != thread.anchor.path).cloned().collect();
        let tid = self.thread_id.clone();
        pr.ensure_worktree_then(move |s, wt| {
            let wt = match wt {
                Ok(w) => w,
                Err(e) => {
                    s.with_controller(&tid, |c, s| {
                        c.status = None;
                        c.reducer.process_exited(-1, &e);
                        c.save(s);
                    });
                    return;
                }
            };
            let env = s.env();
            let base_oid = detail.base_oid.clone();
            s.spawner.spawn(move || {
                let prompt = if first {
                    let surrounding = surrounding(&env, &wt, &thread.anchor, &base_oid);
                    prompts::discuss_context(&detail, &thread.anchor, &selected, surrounding.as_deref(), &on_file, &others, &text)
                } else {
                    text
                };
                with_pr(&env.pr_ref, move |s| {
                    s.with_controller(&tid, |c, s| {
                        c.status = None;
                        let r = c.start_session_if_needed(s, &wt, &detail).and_then(|_| {
                            let sess = c.session.as_ref().unwrap();
                            sess.set_turn_active(true);
                            sess.send(&prompt)
                        });
                        if let Err(e) = r {
                            c.reducer.process_exited(-1, &e.to_string());
                            c.save(s);
                        }
                    });
                })
            });
        });
    }

    /// "Fix it": ask the agent to implement and commit what was discussed.
    pub fn fix_it(&mut self, pr: &mut PRStore) {
        let Some(thread) = pr.discuss_threads.get(&self.thread_id).cloned() else { return };
        if self.is_running() {
            return;
        }
        if pr.is_fix_running(&self.thread_id) {
            pr.enqueue_fix(&self.thread_id, thread.anchor, false);
            return;
        }
        let id = pr.enqueue_fix(&self.thread_id, thread.anchor, true);
        self.run_fix(pr, &id);
    }

    pub fn run_fix(&mut self, pr: &mut PRStore, request_id: &str) {
        let tid = self.thread_id.clone();
        let rid = request_id.to_string();
        pr.mark_fix(&rid, FixStatus::Running, vec![], None);
        self.status = Some("Preparing checkout…".into());
        pr.ensure_worktree_then(move |s, wt| match wt {
            Err(e) => {
                s.mark_fix(&rid, FixStatus::Failed, vec![], Some(e));
                s.with_controller(&tid, |c, _| c.status = None);
            }
            Ok(wt) => {
                let env = s.env();
                s.spawner.spawn(move || {
                    let head = env.worktrees.as_ref().and_then(|w| w.head_sha(&wt).ok()).unwrap_or_default();
                    with_pr(&env.pr_ref, move |s| {
                        s.with_controller(&tid, |c, s| {
                            c.status = None;
                            c.fix_baseline = Some((rid, head));
                            c.send(s, prompts::FIX_INSTRUCTION);
                        });
                    })
                });
            }
        });
    }

    pub fn stop(&mut self, pr: &mut PRStore) {
        if let Some(s) = &self.session {
            s.interrupt();
        }
        self.reducer.interrupted();
        self.save(pr);
    }

    // MARK: Convert to comment

    /// Forks the session to draft a review comment, then opens the inline composer.
    pub fn convert_to_comment(&mut self, pr: &mut PRStore) {
        let (Some(thread), Some(detail), Some(claude)) =
            (pr.discuss_threads.get(&self.thread_id).cloned(), pr.detail.clone(), pr.services.claude_path.clone())
        else {
            return;
        };
        self.draft_state = TaskState::Running("Drafting comment…".into());
        let session_id = self.reducer.session_id().map(str::to_string);
        let mut prompt = prompts::draft_comment(&thread.anchor);
        if session_id.is_none() {
            // No session yet: include the visible transcript.
            let t: Vec<String> = self
                .messages()
                .iter()
                .filter(|m| m.role == Role::User || m.role == Role::Assistant)
                .map(|m| format!("{}: {}", if m.role == Role::User { "user" } else { "assistant" }, m.text))
                .collect();
            prompt = format!("{}\n\n{}", t.join("\n\n"), prompt);
        }
        let model = pr.services.claude_model();
        let tid = self.thread_id.clone();
        pr.ensure_worktree_then(move |s, wt| {
            let wt = match wt {
                Ok(w) => w,
                Err(e) => {
                    s.with_controller(&tid, |c, _| c.draft_state = TaskState::Failed(e));
                    return;
                }
            };
            let mut cfg = ClaudeConfig::new(claude, wt.path.clone());
            cfg.fork_session = session_id.is_some();
            cfg.resume_session_id = session_id;
            cfg.append_system_prompt = Some(prompts::system_prompt(&detail));
            cfg.model = model;
            cfg.permission_mode = "plan".into();
            let env = s.env();
            s.spawner.spawn(move || {
                let r = env.one_shot.run(&cfg, &prompt);
                with_pr(&env.pr_ref, move |s| match r {
                    Ok(res) => {
                        let body = clean_draft(res.text.as_deref().unwrap_or(""));
                        s.with_controller(&tid, |c, _| c.draft_state = TaskState::Idle);
                        s.start_comment(Some(thread.anchor.clone()), &body, Some("Drafted from Discuss"));
                        s.scroll_to_anchor(&thread.anchor);
                    }
                    Err(e) => {
                        s.with_controller(&tid, |c, _| c.draft_state = TaskState::Failed(e.to_string()));
                    }
                })
            });
        });
    }

    // MARK: Session plumbing

    fn start_session_if_needed(&mut self, pr: &PRStore, wt: &Worktree, detail: &PRDetail) -> Result<(), ClaudeError> {
        if self.session.is_some() {
            return Ok(());
        }
        let claude = pr.services.claude_path.clone().ok_or(ClaudeError::NotInstalled)?;
        let mut cfg = ClaudeConfig::new(claude, wt.path.clone());
        cfg.resume_session_id = self.reducer.session_id().map(str::to_string);
        cfg.append_system_prompt = Some(prompts::system_prompt(detail));
        cfg.model = pr.services.claude_model();
        let spawner = pr.spawner.clone();
        let r = pr.pr_ref.clone();
        let tid = self.thread_id.clone();
        let sink = Box::new(move |out: SessionOutput| {
            let tid = tid.clone();
            spawner.post(with_pr(&r, move |s| s.handle_session_output(&tid, out)));
        });
        self.session = Some((pr.services.make_session)(cfg, sink));
        Ok(())
    }

    pub fn handle(&mut self, pr: &mut PRStore, out: SessionOutput) {
        match out {
            SessionOutput::Event(ev) => {
                self.reducer.apply(&ev);
                match ev {
                    ClaudeEvent::Result(_) => {
                        if let Some(s) = &self.session {
                            s.set_turn_active(false);
                        }
                        self.save(pr);
                        if self.fix_baseline.is_some() {
                            self.finish_fix(pr);
                        } else {
                            // The agent may have pushed (asked to, despite the prompt).
                            pr.refresh_if_pushed_from_checkout();
                        }
                    }
                    ClaudeEvent::Assistant { .. } => self.save(pr),
                    _ => {}
                }
            }
            SessionOutput::Exited { status, stderr } => {
                self.reducer.process_exited(status, &stderr);
                self.session = None;
                self.save(pr);
                if self.fix_baseline.is_some() {
                    self.finish_fix(pr);
                }
            }
        }
    }

    fn finish_fix(&mut self, pr: &mut PRStore) {
        let Some((request_id, before)) = self.fix_baseline.take() else { return };
        let tid = self.thread_id.clone();
        pr.refresh_git_state_then(false, move |s| {
            let env = s.env();
            let wt = s.worktree();
            let commits = s.local_commits.clone();
            s.spawner.spawn(move || {
                let after = match (&wt, &env.worktrees) {
                    (Some(wt), Some(w)) => w.head_sha(wt).ok(),
                    _ => None,
                };
                // local_commits is newest-first; keep the ones created after the baseline.
                let produced: Vec<String> = match after {
                    Some(a) if a != before => commits.iter().take_while(|c| c.sha != before).map(|c| c.sha.clone()).collect(),
                    _ => vec![],
                };
                with_pr(&env.pr_ref, move |s| {
                    if produced.is_empty() {
                        s.mark_fix(&request_id, FixStatus::Failed, vec![], Some("No commit was created.".into()));
                    } else {
                        s.mark_fix(&request_id, FixStatus::Committed, produced.clone(), None);
                        if let Some(mut t) = s.discuss_threads.get(&tid).cloned() {
                            for c in produced {
                                if !t.fix_commits.contains(&c) {
                                    t.fix_commits.push(c);
                                }
                            }
                            s.update_discuss_thread(t);
                        }
                    }
                    s.run_next_queued_fix();
                })
            });
        });
    }

    fn save(&self, pr: &mut PRStore) {
        let Some(mut t) = pr.discuss_threads.get(&self.thread_id).cloned() else { return };
        t.messages = self.reducer.messages().to_vec();
        t.session_id = self.reducer.session_id().map(str::to_string);
        pr.update_discuss_thread(t);
    }

    pub fn shutdown(&self) {
        if let Some(s) = &self.session {
            s.terminate();
        }
    }
}

/// ±20 lines around the anchor, numbered, from the worktree (new side) or base commit.
fn surrounding(env: &crate::pr_store::Env, wt: &Worktree, anchor: &CodeAnchor, base_oid: &str) -> Option<String> {
    let lines: Vec<String> = if anchor.side == DiffSide::Right {
        let s = std::fs::read_to_string(wt.path.join(&anchor.path)).ok()?;
        s.split('\n').map(str::to_string).collect()
    } else {
        let d = env.worktrees.as_ref()?.file_contents(&wt.repo_dir, base_oid, &anchor.path).ok()?;
        split_lines(&d)
    };
    let lo = (anchor.start_line - 20).max(1);
    let hi = (anchor.end_line + 20).min(lines.len() as i64);
    if lo > hi {
        return None;
    }
    Some((lo..=hi).map(|n| format!("{}\t{}", n, lines[n as usize - 1])).collect::<Vec<_>>().join("\n"))
}

pub fn clean_draft(s: &str) -> String {
    let mut t = s.trim().to_string();
    // Models sometimes wrap the whole answer in ```markdown fences.
    if t.starts_with("```markdown") || t.starts_with("```md") {
        t = t.split('\n').skip(1).collect::<Vec<_>>().join("\n");
        if t.ends_with("```") {
            t.truncate(t.len() - 3);
        }
    }
    t.trim().to_string()
}
