//! Scripted UI driver for visual checks and performance runs, one command per few frames:
//!   `--ui-script "wait:1;select:0:12:14;snap:select;comment;snap:composer;quit" --snap-dir <dir>`
//!   `--perf-run <out.json> [--perf-only scroll|jump|toggle|composer]`
//! Snapshots use egui's viewport screenshot (no screen-recording permission needed).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use review_buddy_core::claude::transcript::{DiscussMessage, Role};
use review_buddy_core::diff::layout::DiffViewMode;
use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::review::keybindings::ReviewAction;
use review_buddy_core::support::perf::Perf;
use review_buddy_core::support::synthetic::{Shape, SyntheticDiff};
use review_buddy_core::*;

use super::ReviewBuddyApp;
use crate::pr_store::{LoadState, TaskState};

pub struct ScriptRunner {
    cmds: VecDeque<Vec<String>>,
    dir: PathBuf,
    no_pr: bool,
    started: bool,
    wait_until: f64,
    waiting_snap: Option<String>,
    /// Label the next frame's time is recorded under (perf).
    pub recording: Option<String>,
    samples: HashMap<String, Vec<f64>>,
    perf_out: Option<PathBuf>,
    report: serde_json::Map<String, serde_json::Value>,
    rng: u64,
}

const STEP: f64 = 0.35;

impl ScriptRunner {
    pub fn script(script: &str, dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let no_pr = script.starts_with("nopr;");
        let cmds = script
            .split(';')
            .filter(|c| !c.is_empty() && *c != "nopr")
            .map(|c| c.split(':').map(str::to_string).collect())
            .collect();
        ScriptRunner::new(cmds, dir, no_pr, None)
    }

    pub fn perf(out: PathBuf, only: Option<&str>) -> Self {
        let c = |s: &str| s.split(':').map(str::to_string).collect::<Vec<String>>();
        let mut cmds: Vec<Vec<String>> = vec![c("#begin")];
        match only {
            Some("scroll") => cmds.push(c("#scroll:1500:60:scrollUnified")),
            Some("jump") => cmds.push(c("#jump:400:jumpToFileMs")),
            Some("toggle") => cmds.push(c("#toggles:30:jumpToFileMs")),
            Some("composer") => cmds.push(c("#composers:40:addComposerMs")),
            _ => {
                cmds.push(c("#composers:9:addComposerMs"));
                cmds.push(c("#collapses:9:collapseFileMs"));
                cmds.push(c("file:0"));
                cmds.push(c("#scroll:400:60:scrollUnified"));
                cmds.push(c("#scroll:150:900:flingUnified"));
                cmds.push(c("#jump:40:jumpToFileMs"));
                cmds.push(c("file:5"));
                cmds.push(c("#toggle:split:toggleSplitMs"));
                cmds.push(c("#scroll:300:60:scrollSplit"));
                cmds.push(c("#toggle:unified:toggleUnifiedMs"));
            }
        }
        cmds.push(c("#write"));
        cmds.push(c("quit"));
        ScriptRunner::new(cmds.into(), std::env::temp_dir(), false, Some(out))
    }

    fn new(cmds: VecDeque<Vec<String>>, dir: PathBuf, no_pr: bool, perf_out: Option<PathBuf>) -> Self {
        ScriptRunner {
            cmds,
            dir,
            no_pr,
            started: false,
            wait_until: 0.0,
            waiting_snap: None,
            recording: None,
            samples: HashMap::new(),
            perf_out,
            report: serde_json::Map::new(),
            rng: 0x9E3779B97F4A7C15,
        }
    }

    pub fn record_frame(&mut self, ms: f64) {
        if let Some(l) = self.recording.take() {
            self.samples.entry(l).or_default().push(ms);
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % n.max(1) as u64) as usize
    }
}

impl ReviewBuddyApp {
    /// Runs the next script command when it's due. Called at the start of every frame.
    pub(super) fn drive_script(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.script.take() else { return };
        ctx.request_repaint();
        let now = ctx.input(|i| i.time);
        // A snapshot in flight: wait for the screenshot event.
        if let Some(name) = r.waiting_snap.clone() {
            let shot = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let _ = super::save_png(&r.dir.join(format!("{name}.png")).to_string_lossy(), &img);
                println!("snapshot {name}");
                r.waiting_snap = None;
                r.wait_until = now + STEP;
            }
            self.script = Some(r);
            return;
        }
        if !r.started {
            let ready = r.no_pr || self.state.current_pr().map(|s| s.load_state == LoadState::Loaded && s.layout.is_some()).unwrap_or(false);
            if !ready {
                self.script = Some(r);
                return;
            }
            r.started = true;
            r.wait_until = now + 0.5;
        }
        if now < r.wait_until {
            self.script = Some(r);
            return;
        }
        let Some(parts) = r.cmds.pop_front() else {
            self.script = Some(r);
            return;
        };
        let mut next = STEP;
        self.run_command(ctx, &mut r, &parts, &mut next);
        r.wait_until = now + next;
        self.script = Some(r);
    }

    pub(crate) fn run_command(&mut self, ctx: &egui::Context, r: &mut ScriptRunner, parts: &[String], next: &mut f64) {
        let arg = |i: usize| parts.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let rest = |from: usize| parts.get(from..).map(|v| v.join(":")).unwrap_or_default();
        let cmd = parts[0].as_str();
        let pr_ref = self.state.current.clone();
        // App-level commands.
        match cmd {
            "wait" => {
                *next = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                return;
            }
            "snap" => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                r.waiting_snap = Some(parts.get(1).cloned().unwrap_or_else(|| "snap".into()));
                return;
            }
            "quit" => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            "dark" => {
                ctx.set_theme(egui::Theme::Dark);
                return;
            }
            "light" => {
                ctx.set_theme(egui::Theme::Light);
                return;
            }
            "resize" => {
                let h = if parts.len() > 2 { arg(2) } else { 1000 };
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(arg(1) as f32, h as f32)));
                return;
            }
            "sidebar" => {
                self.state.ui.show_sidebar = parts.get(1).map(|s| s != "off").unwrap_or(!self.state.ui.show_sidebar);
                return;
            }
            "panel" => {
                self.state.ui.show_discuss_panel = parts.get(1).map(|s| s != "off").unwrap_or(true);
                return;
            }
            "palette" => {
                self.state.ui.show_palette = parts.get(1).map(|s| s != "off").unwrap_or(true);
                return;
            }
            "submit" => {
                self.state.ui.show_submit_review = parts.get(1).map(|s| s != "off").unwrap_or(true);
                return;
            }
            "settings" => {
                self.state.ui.show_settings = parts.get(1).map(|s| s != "off").unwrap_or(true);
                return;
            }
            "inbox" => {
                let lines: Vec<String> = self
                    .state
                    .inbox
                    .sections
                    .iter()
                    .map(|s| format!("{} loading={} items={} error={}", s.id, s.is_loading, s.items.len(), s.error.clone().unwrap_or("-".into())))
                    .collect();
                let _ = std::fs::write(r.dir.join("inbox.txt"), lines.join("\n"));
                return;
            }
            "key" => {
                // key:<action id> — performs a keyboard action, e.g. key:nextFile
                if let Some(a) = ReviewAction::ALL.iter().find(|a| a.id() == parts.get(1).map(String::as_str).unwrap_or("")) {
                    self.perform(*a);
                }
                return;
            }
            "#begin" => {
                let s = self.state.current_pr().unwrap();
                r.report.insert("loadMs".into(), Perf::shared().last("pr.load.total").unwrap_or(0.0).into());
                r.report.insert("rows".into(), s.layout.as_ref().map(|l| l.row_count()).unwrap_or(0).into());
                r.report.insert("files".into(), s.files().len().into());
                r.report.insert("lines".into(), s.diff.as_ref().map(|d| d.total_lines()).unwrap_or(0).into());
                Perf::shared().reset();
                return;
            }
            "#write" => {
                let stats = |v: &[f64]| {
                    let mut s = v.to_vec();
                    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let n = s.len();
                    if n == 0 {
                        return serde_json::json!({"n": 0, "p50": 0.0, "p95": 0.0, "max": 0.0, "mean": 0.0});
                    }
                    serde_json::json!({"n": n, "p50": s[n / 2], "p95": s[((n as f64 * 0.95) as usize).min(n - 1)], "max": s[n - 1], "mean": s.iter().sum::<f64>() / n as f64})
                };
                let median = |v: &[f64]| {
                    let mut s = v.to_vec();
                    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    s.get(s.len() / 2).copied().unwrap_or(0.0)
                };
                for (k, v) in &r.samples {
                    let is_stats = matches!(k.as_str(), "scrollUnified" | "scrollSplit" | "flingUnified" | "jumpToFileMs");
                    r.report.insert(k.clone(), if is_stats { stats(v) } else { median(v).into() });
                }
                for k in ["scrollUnified", "scrollSplit", "flingUnified", "jumpToFileMs"] {
                    r.report.entry(k.to_string()).or_insert_with(|| stats(&[]));
                }
                for k in ["toggleSplitMs", "toggleUnifiedMs", "addComposerMs", "collapseFileMs"] {
                    r.report.entry(k.to_string()).or_insert(0.0.into());
                }
                r.report.insert("memoryMB".into(), memory_mb().into());
                r.report.insert("counters".into(), Perf::shared().report().into());
                if let Some(out) = &r.perf_out {
                    let _ = std::fs::write(out, serde_json::to_string_pretty(&serde_json::Value::Object(r.report.clone())).unwrap());
                }
                return;
            }
            _ => {}
        }
        let Some(pr_ref) = pr_ref else { return };
        // Perf steps that repeat over frames re-queue themselves.
        if let Some(rest_cmd) = cmd.strip_prefix('#') {
            self.perf_step(r, rest_cmd, parts, &pr_ref, next);
            return;
        }
        if cmd == "demodiscuss" {
            self.state.ui.show_discuss_panel = true;
        }
        let Some(store) = self.state.prs.get_mut(&pr_ref) else { return };
        let first_addition = |s: &crate::pr_store::PRStore| {
            s.files().iter().enumerate().find_map(|(i, f)| f.lines.iter().find(|l| l.kind == LineKind::Addition).map(|l| (i, l.new_line as i64)))
        };
        match cmd {
            "select" => {
                let (f, a) = (arg(1) as usize, arg(2));
                let b = if arg(3) == 0 { a } else { arg(3) };
                store.select(f, DiffSide::Right, a, false, None);
                store.select(f, DiffSide::Right, b, true, None);
                if let Some(path) = store.files().get(f).map(|x| x.path().to_string()) {
                    store.scroll_to_anchor(&CodeAnchor::new(path, DiffSide::Right, a, a));
                }
            }
            "textsel" => {
                let f = arg(1) as usize;
                store.select(f, DiffSide::Right, arg(2), false, Some(arg(3) as usize));
                store.select(f, DiffSide::Right, arg(4), true, Some(arg(5) as usize));
                if let Some(path) = store.files().get(f).map(|x| x.path().to_string()) {
                    store.scroll_to_anchor(&CodeAnchor::new(path, DiffSide::Right, arg(2), arg(2)));
                }
            }
            "selectfirst" => {
                if let Some((f, l)) = first_addition(store) {
                    store.select(f, DiffSide::Right, l, false, None);
                    let path = store.files()[f].path().to_string();
                    store.scroll_to_anchor(&CodeAnchor::new(path, DiffSide::Right, l, l));
                }
            }
            "selectline" => {
                let f = arg(1) as usize;
                let adds: Vec<i64> = store.files().get(f).map(|x| x.lines.iter().filter(|l| l.kind == LineKind::Addition).map(|l| l.new_line as i64).collect()).unwrap_or_default();
                if let Some(&l) = adds.get((arg(2) as usize).min(adds.len().saturating_sub(1))) {
                    store.select(f, DiffSide::Right, l, false, None);
                    let path = store.files()[f].path().to_string();
                    store.scroll_to_anchor(&CodeAnchor::new(path, DiffSide::Right, l, l));
                }
            }
            "addfile" => {
                let extra = "diff --git a/zz_new.go b/zz_new.go\nnew file mode 100644\n--- /dev/null\n+++ b/zz_new.go\n@@ -0,0 +1,2 @@\n+package main\n+// new\n";
                let mut text = SyntheticDiff::make(Shape::SMALL);
                if parts.get(1).map(String::as_str) == Some("swap") {
                    if let Some(i) = text[1..].find("diff --git") {
                        text = text[i + 1..].to_string();
                    }
                }
                store.install(UnifiedDiffParser::parse(&format!("{text}\n{extra}")), None);
                let last = store.files().last().map(|f| f.path().to_string()).unwrap_or("-".into());
                let _ = std::fs::write(r.dir.join("files.txt"), format!("files={} last={last}", store.files().len()));
            }
            "order" => {
                let first: Vec<String> = store.files().iter().take(3).map(|f| f.path().rsplit('/').next().unwrap().to_string()).collect();
                let line = format!("first={:?} reviewFirst={} topRow={} currentFile={}\n", first, store.review_first_count(), store.top_visible_row, store.current_file);
                let path = r.dir.join("order.txt");
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                let _ = std::fs::write(path, old + &line);
            }
            "admin" => {
                if let Some(s) = store.detail.as_mut().and_then(|d| d.status.as_mut()) {
                    s.viewer_can_merge_as_admin = Some(true);
                }
            }
            "outdated" => store.remote_head_oid = Some("c".repeat(40)),
            "noconversation" => {
                if let Some(d) = store.detail.as_mut() {
                    d.conversation = None;
                }
            }
            "mergestate" => {
                if let Some(s) = store.detail.as_mut().and_then(|d| d.status.as_mut()) {
                    s.merge_state_status = parts.get(1).cloned().unwrap_or("CLEAN".into());
                }
            }
            "overview" => store.show_overview = parts.get(1).map(|s| s != "off").unwrap_or(true),
            "demosummary" => {
                let f: Vec<String> = store.files().iter().map(|f| f.path().to_string()).collect();
                if !f.is_empty() {
                    let s = AISummary {
                        summary: "Moves user fetching to `async`/`await` and adds a shared `DEFAULT_TIMEOUT`. `fetchUser` now returns `null` for missing users instead of throwing, and every call site is updated to handle it. Scoring moves into `Result` so it's computed once per item.".into(),
                        reading_order: vec![
                            ReadingItem { path: f[0].clone(), reason: "Defines the new `fetchUser` contract".into() },
                            ReadingItem { path: f[f.len() / 2].clone(), reason: "Main call site; shows the `null` handling".into() },
                            ReadingItem { path: f[f.len() - 1].clone(), reason: "Scoring moved into `Result`".into() },
                        ],
                        risks: vec![
                            Risk { path: Some(f[0].clone()), description: "`fetchUser` returns `null` for every non-OK status, so a 500 looks like a missing user.".into() },
                            Risk { path: None, description: "No tests cover the timeout path.".into() },
                        ],
                        head_sha: store.detail.as_ref().map(|d| d.head_oid.clone()),
                        created_at: None,
                    };
                    store.set_summary(Some(s), TaskState::Idle);
                }
            }
            "fakesummary" => {
                let f: Vec<String> = store.files().iter().map(|f| f.path().to_string()).collect();
                let n = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
                let s = AISummary {
                    summary: "This PR reworks the processing pipeline and changes how scores are computed. ".repeat(n),
                    reading_order: vec![
                        ReadingItem { path: f[f.len() - 1].clone(), reason: "Core change".into() },
                        ReadingItem { path: f[f.len() / 2].clone(), reason: "Call sites".into() },
                    ],
                    risks: vec![Risk { path: Some(f[0].clone()), description: "Edge case for `empty` input".into() }],
                    head_sha: None,
                    created_at: None,
                };
                store.set_summary(Some(s), TaskState::Idle);
            }
            "demodiscuss" => {
                if let (Some(d), Some((f, l))) = (store.detail.clone(), first_addition(store)) {
                    let anchor = CodeAnchor::new(store.files()[f].path(), DiffSide::Right, l, l + 2);
                    let mut t = DiscussThread::new(store.pr_ref.clone(), anchor, d.head_oid.clone(), 0);
                    let mut tool = DiscussMessage::new(Role::Tool, "");
                    tool.tool_name = Some("Grep".into());
                    tool.tool_input = Some(r#"{"pattern":"fetchUser\\("}"#.into());
                    tool.tool_result = Some("3 matches in 3 files".into());
                    let reply = if parts.get(1).map(String::as_str) == Some("table") { DEMO_TABLE_REPLY } else { DEMO_REPLY };
                    t.messages = vec![
                        DiscussMessage::new(Role::User, "Why return `null` here instead of throwing? Callers seem to expect a `User`."),
                        tool,
                        DiscussMessage::new(Role::Assistant, reply),
                    ];
                    let id = t.id.clone();
                    store.update_discuss_thread(t);
                    store.active_discuss_id = Some(id);
                    store.rebuild_layout(true);
                }
            }
            "settext" => {
                if let Some(id) = store.composers.keys().next().cloned() {
                    store.update_composer(&id, rest(1));
                }
            }
            "cancelall" => {
                for id in store.composers.keys().cloned().collect::<Vec<_>>() {
                    store.cancel_composer(&id);
                }
            }
            "typecomment" => {
                if let Some(id) = store.composers.values().find(|c| c.anchor().is_some()).map(|c| c.id.clone()) {
                    store.update_composer(&id, rest(1));
                    store.submit_composer(&id, false);
                }
            }
            "replyfirst" => {
                let mut ts: Vec<&ReviewThread> = store.threads.values().filter(|t| !t.is_resolved).collect();
                ts.sort_by(|a, b| a.id.cmp(&b.id));
                if let Some(tid) = ts.first().map(|t| t.id.clone()) {
                    store.start_reply(&tid);
                    if let Some(id) = store.composer_for_reply(&tid).map(|c| c.id.clone()) {
                        store.update_composer(&id, rest(1));
                        store.submit_composer(&id, false);
                    }
                }
            }
            "resolvefirst" => {
                let mut ts: Vec<String> = store.threads.values().filter(|t| !t.is_resolved).map(|t| t.id.clone()).collect();
                ts.sort();
                if let Some(t) = ts.first() {
                    store.set_resolved(t, true);
                }
            }
            "submitreview" => {
                store.review_body = rest(2);
                let ev = match parts.get(1).map(String::as_str) {
                    Some("APPROVE") => ReviewEvent::Approve,
                    Some("REQUEST_CHANGES") => ReviewEvent::RequestChanges,
                    _ => ReviewEvent::Comment,
                };
                store.submit_review(ev, |_, _| {});
            }
            "discard" => store.discard_pending_review(),
            "ensurewt" => store.ensure_worktree_then(|_, _| {}),
            "push" => store.push(|_, _| {}),
            "expandabove" => store.expand_above(arg(1) as usize, arg(2) as usize, 20),
            "expandbelow" => store.expand_below(arg(1) as usize, 20),
            "summarize" => store.start_summarize(),
            "prereview" => store.start_pre_review(),
            "convert" => {
                if let Some(id) = store.active_discuss_id.clone() {
                    store.with_controller(&id, |c, s| c.convert_to_comment(s));
                }
            }
            "fix" => {
                if let Some(id) = store.active_discuss_id.clone() {
                    store.with_controller(&id, |c, s| c.fix_it(s));
                }
            }
            "ask" => {
                if let Some(id) = store.active_discuss_id.clone() {
                    let q = rest(1);
                    store.with_controller(&id, |c, s| c.send(s, &q));
                }
            }
            "reopen" => store.active_discuss_id = store.sorted_discuss_threads().first().map(|t| t.id.clone()),
            "dump" => {
                let mut threads: Vec<String> = store
                    .threads
                    .values()
                    .map(|t| {
                        let cs: Vec<String> = t.comments.iter().map(|c| format!("[{}] {}", if c.is_pending { "pending" } else { "posted" }, c.body)).collect();
                        format!("{}:{} resolved={} {}", t.path, t.line.unwrap_or(-1), t.is_resolved, cs.join(" | "))
                    })
                    .collect();
                threads.sort();
                let mut viewed: Vec<String> = store.viewed.iter().cloned().collect();
                viewed.sort();
                let out = serde_json::json!({
                    "composers": store.composers.values().map(|c| c.text.clone()).collect::<Vec<_>>(),
                    "localCommits": store.local_commits.iter().map(|c| format!("{} {}", c.short_sha(), c.subject)).collect::<Vec<_>>(),
                    "findings": store.open_findings().iter().map(|f| format!("{} {}:{} {}", f.severity.as_str(), f.path, f.start_line, f.title)).collect::<Vec<_>>(),
                    "summary": store.summary.as_ref().map(|s| s.summary.clone()).unwrap_or_default(),
                    "readingOrder": store.summary.as_ref().map(|s| s.reading_order.iter().map(|r| r.path.clone()).collect::<Vec<_>>()).unwrap_or_default(),
                    "fixRequests": store.fix_requests.iter().map(|f| format!("{:?} {}", f.status, f.error.clone().unwrap_or_default())).collect::<Vec<_>>(),
                    "threads": threads,
                    "pendingReviewId": store.pending_review_id.clone().unwrap_or_default(),
                    "viewed": viewed,
                    "pushState": format!("{:?}", store.push_state),
                    "submitState": format!("{:?}", store.submit_state),
                    "banner": store.banner.clone().unwrap_or_default(),
                    "summaryState": format!("{:?}", store.summary_state),
                    "preReviewState": format!("{:?}", store.pre_review_state),
                    "mode": format!("{:?}", store.options.mode),
                    "expandedAbove": store.options.expanded_above.len(),
                    "expandedBelow": store.options.expanded_below.len(),
                    "selection": store.selection.as_ref().map(|s| format!("{}:{}-{}", s.path, s.start, s.end)),
                    "discussThreads": store.discuss_threads.len(),
                    "activeDiscuss": store.active_discuss_id.is_some(),
                });
                let name = parts.get(1).cloned().unwrap_or("state".into());
                let _ = std::fs::write(r.dir.join(format!("{name}.json")), serde_json::to_string_pretty(&out).unwrap());
            }
            "comment" => store.start_comment(None, "", None),
            "discuss" => {
                if store.start_discuss(None, None).is_some() {
                    self.state.ui.show_discuss_panel = true;
                }
            }
            "split" => store.set_mode(DiffViewMode::Split),
            "unified" => store.set_mode(DiffViewMode::Unified),
            "file" => store.scroll_to_file(arg(1) as usize),
            "viewed" => store.toggle_viewed(arg(1) as usize),
            "collapse" => store.toggle_collapsed(arg(1) as usize),
            "next" => store.navigate(ReviewAction::NextThread),
            _ => eprintln!("ui-script: unknown command {cmd}"),
        }
    }

    fn perf_step(&mut self, r: &mut ScriptRunner, cmd: &str, parts: &[String], pr_ref: &PRRef, next: &mut f64) {
        let n = |i: usize| parts.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let label = parts.last().cloned().unwrap_or_default();
        let requeue = |r: &mut ScriptRunner, count: i64| {
            if count > 1 {
                let mut p = parts.to_vec();
                p[1] = (count - 1).to_string();
                r.cmds.push_front(p);
            }
        };
        let canvas = self.view.canvases.entry(pr_ref.clone()).or_default();
        let Some(store) = self.state.prs.get_mut(pr_ref) else { return };
        let files = store.files().len().max(1);
        match cmd {
            "scroll" => {
                canvas.scroll_by(n(2) as f32);
                r.recording = Some(label);
                requeue(r, n(1));
                *next = 0.0;
            }
            "jump" => {
                let f = r.rand(files);
                store.scroll_to_file(f);
                r.recording = Some(label);
                requeue(r, n(1));
                *next = 0.016;
            }
            "toggles" => {
                let m = if n(1) % 2 == 0 { DiffViewMode::Split } else { DiffViewMode::Unified };
                store.set_mode(m);
                r.recording = Some(label);
                requeue(r, n(1));
                *next = 0.05;
            }
            "toggle" => {
                store.set_mode(if parts.get(1).map(String::as_str) == Some("split") { DiffViewMode::Split } else { DiffViewMode::Unified });
                r.recording = Some(label);
                *next = 0.3;
            }
            "composers" => {
                // Alternates: scroll to a file, then (next step) open a composer on it.
                let i = n(1);
                let f = ((10 + i * 3) as usize).min(files - 1);
                for id in store.composers.keys().cloned().collect::<Vec<_>>() {
                    store.cancel_composer(&id);
                }
                if let Some(l) = store.files()[f].lines.iter().find(|l| l.kind == LineKind::Addition).map(|l| l.new_line as i64) {
                    store.select(f, DiffSide::Right, l, false, None);
                    store.start_comment(None, "", None);
                    r.recording = Some(label);
                    store.scroll_to_file(f);
                }
                requeue(r, i);
                *next = 0.08;
            }
            "collapses" => {
                let i = n(1);
                let f = ((10 + i * 7) as usize).min(files - 1);
                for id in store.composers.keys().cloned().collect::<Vec<_>>() {
                    store.cancel_composer(&id);
                }
                store.toggle_collapsed(f);
                r.recording = Some(label);
                // Expand it again on the following step.
                r.cmds.push_front(vec!["collapse".into(), f.to_string()]);
                requeue(r, i);
                *next = 0.08;
            }
            _ => {}
        }
    }
}

fn memory_mb() -> f64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

const DEMO_REPLY: &str = "All three call sites (`ProfileView`, `Sidebar`, `SearchResults`) already check for `null`, so the change is safe for a missing user.\n\nThe catch: `!res.ok` also covers 500s and timeouts, which now look like \"user not found\". I'd return `null` only for `404` and throw otherwise.\n\nWant me to make that change and commit it to the PR branch?";

const DEMO_TABLE_REPLY: &str = "`fetchUser` has three callers:\n\n| Caller | File | Handles `null`? |\n|---|---|:---:|\n| ProfileView | `src/views/ProfileView.tsx:42` | yes |\n| Sidebar | `src/components/Sidebar.tsx:17` | yes |\n| SearchResults | `src/search/SearchResults.tsx:88` | **no**, renders \"Unknown user\" |\n\n1. Return `null` only for `404`.\n   - throw for 5xx and timeouts\n2. Add a test for the timeout path.\n\n```ts\nif (res.status === 404) return null\nif (!res.ok) throw new HttpError(res.status)\n```";
