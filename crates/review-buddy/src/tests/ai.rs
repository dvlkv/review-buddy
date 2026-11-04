//! Port of `Tests/ReviewBuddyUITests/AITests.swift`: Discuss threads, summary,
//! pre-review and the Discuss controller.

use std::sync::atomic::Ordering;

use review_buddy_core::claude::prompts;
use review_buddy_core::claude::session::{ClaudeError, SessionOutput};
use review_buddy_core::claude::stream_events::{ClaudeEvent, ContentBlock, ToolResult};
use review_buddy_core::claude::transcript::Role;
use review_buddy_core::diff::layout::InlineBlockKind;
use review_buddy_core::persistence::AppSettings;
use review_buddy_core::*;

use super::support::*;
use crate::discuss::clean_draft;
use crate::pr_store::TaskState;

// MARK: - Discuss threads on the store

#[test]
fn start_discuss_anchors_and_activates() {
    let mut h = Harness::new();
    h.load();
    let p1 = h.path(1);
    let s = h.s();
    s.select(1, DiffSide::Right, 12, false, None);
    s.select(1, DiffSide::Right, 13, true, None);
    let id = s.start_discuss(None, None).unwrap();
    let t = s.discuss_threads[&id].clone();
    assert_eq!(t.anchor, CodeAnchor::new(p1.clone(), DiffSide::Right, 12, 13));
    assert_eq!(t.commit_sha, s.detail.as_ref().unwrap().head_oid);
    assert_eq!(t.anchor_text_hash, DiscussThread::hash_lines(&s.lines_for(&t.anchor)));
    assert_eq!(s.active_discuss_id.as_deref(), Some(id.as_str()));
    assert!(s.selection.is_none());
    assert!(s.layout.as_ref().unwrap().blocks.iter().any(|b| b.id == id && b.kind == InlineBlockKind::Discuss));
    assert!(h.discuss_paths().contains(&p1));
    // Left-side anchors use the base commit.
    std::thread::sleep(std::time::Duration::from_millis(2)); // distinct created_at
    let s = h.s();
    let left = s.start_discuss(Some(CodeAnchor::new(p1, DiffSide::Left, 13, 13)), None).unwrap();
    assert_eq!(s.discuss_threads[&left].commit_sha, s.detail.as_ref().unwrap().base_oid);
    assert_eq!(s.sorted_discuss_threads().first().map(|t| t.id.clone()), Some(left));
    // Worktree is warmed in the background.
    h.drain();
    assert!(h.worktrees.calls().contains(&"ensureWorktree".into()));
}

#[test]
fn start_discuss_without_anchor_returns_none() {
    let mut h = Harness::new();
    h.load();
    assert!(h.s().start_discuss(None, None).is_none());
}

#[test]
fn delete_discuss_removes_everything() {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(0, 13);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    let s = h.s();
    s.controller(&id);
    s.delete_discuss(&id);
    assert!(!s.discuss_threads.contains_key(&id));
    assert!(!s.controllers.contains_key(&id));
    assert!(s.active_discuss_id.is_none());
    assert!(!s.layout.as_ref().unwrap().blocks.iter().any(|b| b.id == id));
    assert!(h.discuss_paths().is_empty());
}

fn seed_thread(h: &mut Harness, line: i64, text: &[String]) -> DiscussThread {
    let a = h.anchor(0, line);
    let t = DiscussThread::new(pr_ref(), a, "oldsha", DiscussThread::hash_lines(text));
    h.s().update_discuss_thread(t.clone());
    t
}

#[test]
fn remap_same_text_updates_commit() {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(0, 13);
    let lines = h.s().lines_for(&a);
    let t = seed_thread(&mut h, 13, &lines);
    h.act(|s| s.remap_discuss_threads());
    let s = h.s();
    assert_eq!(s.discuss_threads[&t.id].commit_sha, s.detail.as_ref().unwrap().head_oid);
    assert_eq!(s.discuss_threads[&t.id].status, DiscussStatus::Active);
}

#[test]
fn remap_moves_anchor_with_worktree_diff() {
    let mut h = Harness::new();
    h.load();
    h.ensure_worktree().unwrap();
    // Two lines inserted after line 5 → old line 13 is now 15.
    *h.worktrees.path_diff.lock().unwrap() = "--- a/x\n+++ b/x\n@@ -5,0 +6,2 @@\n+a\n+b\n".into();
    let t = seed_thread(&mut h, 13, &["something else".into()]);
    h.act(|s| s.remap_discuss_threads());
    let p0 = h.path(0);
    let s = h.s();
    assert_eq!(s.discuss_threads[&t.id].commit_sha, s.detail.as_ref().unwrap().head_oid);
    assert_eq!(s.discuss_threads[&t.id].anchor.start_line, 15);
    assert!(h.worktrees.calls().contains(&format!("diffPath:{p0}")));
}

#[test]
fn remap_marks_outdated_when_line_changed() {
    let mut h = Harness::new();
    h.load();
    h.ensure_worktree().unwrap();
    *h.worktrees.path_diff.lock().unwrap() = "--- a/x\n+++ b/x\n@@ -13 +13 @@\n-old\n+new\n".into();
    let t = seed_thread(&mut h, 13, &["something else".into()]);
    h.act(|s| s.remap_discuss_threads());
    let s = h.s();
    assert_eq!(s.discuss_threads[&t.id].status, DiscussStatus::Outdated);
    // Outdated threads render at the end of their file.
    let b = s.layout.as_ref().unwrap().blocks.iter().find(|b| b.id == t.id).unwrap();
    assert_eq!(b.line, -1);
}

#[test]
fn remap_without_worktree_leaves_thread() {
    let mut h = Harness::new();
    h.load();
    let t = seed_thread(&mut h, 13, &["different".into()]);
    h.act(|s| s.remap_discuss_threads());
    assert_eq!(h.s().discuss_threads[&t.id].commit_sha, "oldsha");
}

// MARK: - Summary and pre-review

#[test]
fn summarize_stores_summary() {
    let mut h = Harness::new();
    h.load();
    h.one_shot.set_structured(r#"{"summary":"Does things.","readingOrder":[{"path":"a","reason":"core"}],"risks":[]}"#);
    h.act(|s| s.start_summarize());
    let s = h.s();
    assert_eq!(s.summary_state, TaskState::Idle);
    assert_eq!(s.summary.as_ref().unwrap().summary, "Does things.");
    assert_eq!(s.summary.as_ref().unwrap().head_sha.as_ref(), Some(&s.detail.as_ref().unwrap().head_oid));
    let range = prompts::diff_range(s.detail.as_ref().unwrap());
    let cfg = h.one_shot.last_config();
    assert_eq!(cfg.json_schema.as_deref(), Some(prompts::SUMMARY_SCHEMA));
    assert_eq!(cfg.permission_mode, "plan");
    assert_eq!(cfg.cwd, h.worktrees.dir);
    assert!(h.one_shot.last_prompt().contains(&range));
}

#[test]
fn summarize_falls_back_to_text_and_failures() {
    let mut h = Harness::new();
    h.load();
    h.one_shot.set_text(Some(r#"{"summary":"T","readingOrder":[],"risks":[]}"#));
    h.act(|s| s.start_summarize());
    assert_eq!(h.s().summary.as_ref().unwrap().summary, "T");
    h.one_shot.set_error(Some("rate limited"));
    h.act(|s| s.start_summarize());
    assert_eq!(h.s().summary_state, TaskState::Failed("rate limited".into()));
    assert_eq!(h.s().summary.as_ref().unwrap().summary, "T"); // previous summary kept
    h.one_shot.set_error(None);
    h.one_shot.set_text(Some("not json"));
    h.act(|s| s.start_summarize());
    assert!(matches!(h.s().summary_state, TaskState::Failed(_)), "expected decode failure");
}

#[test]
fn missing_claude_fails() {
    let mut h = Harness::with(Opts { claude_path: None, ..Default::default() });
    h.load();
    h.act(|s| s.start_summarize());
    h.act(|s| s.start_pre_review());
    assert!(matches!(h.s().summary_state, TaskState::Failed(_)), "summary should fail");
    assert!(matches!(h.s().pre_review_state, TaskState::Failed(_)), "pre-review should fail");
    assert!(h.one_shot.configs().is_empty());
}

fn findings_json(h: &mut Harness) -> String {
    let (p0, p1) = (h.path(0), h.path(1));
    format!(
        r#"{{"findings":[
      {{"path":"{p0}","side":"RIGHT","startLine":13,"endLine":13,"severity":"low","title":"Low","body":"b"}},
      {{"path":"{p1}","side":"RIGHT","startLine":14,"endLine":13,"severity":"high","title":"High","body":"b","suggestion":"fixed()"}},
      {{"path":"not/in/diff.swift","side":"RIGHT","startLine":1,"endLine":1,"severity":"high","title":"Ghost","body":"b"}},
      {{"path":"{p0}","side":"LEFT","startLine":14,"endLine":14,"severity":"nit","title":"Nit","body":"b","suggestion":""}}
    ]}}"#
    )
}

fn titles(h: &mut Harness) -> Vec<String> {
    h.s().open_findings().into_iter().map(|f| f.title).collect()
}

#[test]
fn pre_review_filters_and_sorts() {
    let mut h = Harness::new();
    h.load();
    let j = findings_json(&mut h);
    h.one_shot.set_structured(&j);
    h.act(|s| s.start_pre_review());
    assert_eq!(h.s().pre_review_state, TaskState::Idle);
    assert_eq!(titles(&mut h), ["High", "Low", "Nit"]);
    let open = h.s().open_findings();
    assert!(open[0].start_line == 13 && open[0].end_line == 14); // swapped range normalized
    assert_eq!(open[2].suggestion, None); // empty suggestion dropped
    assert_eq!(h.one_shot.last_config().json_schema.as_deref(), Some(prompts::PRE_REVIEW_SCHEMA));
    let n = h.s().layout.as_ref().unwrap().blocks.iter().filter(|b| b.kind == InlineBlockKind::Finding).count();
    assert_eq!(n, 3);
}

#[test]
fn finding_actions() {
    let mut h = Harness::new();
    h.load();
    let j = findings_json(&mut h);
    h.one_shot.set_structured(&j);
    h.act(|s| s.start_pre_review());
    let find = |h: &mut Harness, t: &str| h.s().open_findings().into_iter().find(|f| f.title == t).unwrap();
    let high = find(&mut h, "High");
    h.act(|s| s.accept_finding(&high.id));
    let s = h.s();
    assert_eq!(s.findings[&high.id].state, FindingState::Accepted);
    let composer = s.composers.values().next().unwrap().clone();
    assert_eq!(composer.text, high.comment_body());
    assert!(composer.text.contains("```suggestion\nfixed()\n```"));
    assert_eq!(composer.origin.as_deref(), Some("From AI pre-review"));
    let low = find(&mut h, "Low");
    h.act(|s| s.dismiss_finding(&low.id));
    assert_eq!(h.s().findings[&low.id].state, FindingState::Dismissed);
    assert!(!h.s().layout.as_ref().unwrap().blocks.iter().any(|b| b.id == low.id));
    let nit = find(&mut h, "Nit");
    h.act(|s| s.discuss_finding(&nit.id));
    let s = h.s();
    assert_eq!(s.findings[&nit.id].state, FindingState::Discussed);
    let id = s.active_discuss_id.clone().unwrap();
    assert!(s.controller(&id).input.contains("Nit"));
    assert_eq!(s.discuss_threads[&id].anchor.side, DiffSide::Left);
    // Re-running keeps non-open findings and replaces open ones.
    h.act(|s| s.start_pre_review());
    assert_eq!(h.s().findings.values().filter(|f| f.state != FindingState::Open).count(), 3);
    assert_eq!(h.s().open_findings().len(), 3);
}

#[test]
fn pre_review_failure() {
    let mut h = Harness::new();
    h.load();
    h.one_shot.set_error(Some("nope"));
    h.act(|s| s.start_pre_review());
    assert_eq!(h.s().pre_review_state, TaskState::Failed("nope".into()));
}

#[test]
fn auto_run_ai_when_enabled() {
    let settings = AppSettings { auto_summarize: true, auto_pre_review_max_lines: 100_000, ..Default::default() };
    let mut h = Harness::with(Opts { settings, ..Default::default() });
    h.one_shot.set_structured(r#"{"summary":"auto","readingOrder":[],"risks":[],"findings":[]}"#);
    h.load();
    assert_eq!(h.one_shot.configs().len(), 2);
    assert_eq!(h.s().summary.as_ref().unwrap().summary, "auto");
}

// MARK: - Discuss controller

fn setup() -> (Harness, String) {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(0, 13);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    (h, id)
}

fn send(h: &mut Harness, id: &str, text: &str) {
    let text = text.to_string();
    h.ctl(id, move |c, s| c.send(s, &text));
}

fn roles(h: &mut Harness, id: &str) -> Vec<Role> {
    h.controller(id).messages().iter().map(|m| m.role).collect()
}

fn last_message(h: &mut Harness, id: &str) -> (Role, String) {
    let m = h.controller(id).messages().last().cloned().expect("no messages");
    (m.role, m.text)
}

#[test]
fn first_message_wraps_context_then_raw() {
    let (mut h, id) = setup();
    // Surrounding code is read from the worktree.
    let p0 = h.path(0);
    let path = h.worktrees.dir.join(&p0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, (1..=40).map(|i| format!("src {i}")).collect::<Vec<_>>().join("\n")).unwrap();
    send(&mut h, &id, "Why is this here?");
    assert_eq!(h.sessions.count(), 1);
    let session = h.sessions.get(0);
    let sent = session.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("<review_context>"));
    assert!(sent[0].contains("Why is this here?"));
    assert!(sent[0].contains(&p0));
    assert!(sent[0].contains("13\tsrc 13"));
    assert!(session.turn_active());
    assert!(session.config.append_system_prompt.as_deref().unwrap_or("").contains("Review Buddy"));
    assert_eq!(session.config.cwd, h.worktrees.dir);
    assert!(h.controller(&id).is_running());

    session.reply("Because reasons.", "sess-1");
    h.drain();
    assert!(!h.controller(&id).is_running());
    assert_eq!(roles(&mut h, &id), [Role::User, Role::Assistant]);
    assert_eq!(last_message(&mut h, &id).1, "Because reasons.");
    assert_eq!(h.controller(&id).reducer.session_id(), Some("sess-1"));
    assert!(!session.turn_active());
    // Persisted on the thread.
    assert_eq!(h.s().discuss_threads[&id].session_id.as_deref(), Some("sess-1"));
    assert_eq!(h.s().discuss_threads[&id].messages.len(), 2);

    send(&mut h, &id, "And then?");
    assert_eq!(session.sent().len(), 2);
    assert_eq!(session.sent()[1], "And then?");
    assert_eq!(h.sessions.count(), 1); // same process reused
    send(&mut h, &id, "   ");
    assert_eq!(session.sent().len(), 2);
}

#[test]
fn streaming_events_and_tool_calls() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    let s = h.sessions.get(0);
    s.event(ClaudeEvent::TextDelta { message_id: None, text: "Hel".into() });
    s.event(ClaudeEvent::TextDelta { message_id: None, text: "lo".into() });
    h.drain();
    let last = h.controller(&id).messages().last().cloned().unwrap();
    assert_eq!(last.text, "Hello");
    assert!(last.is_streaming);
    s.event(ClaudeEvent::Assistant {
        message_id: "m".into(),
        blocks: vec![
            ContentBlock::Text("Hello!".into()),
            ContentBlock::ToolUse { id: "t1".into(), name: "Read".into(), input: r#"{"file_path":"/a/b.go"}"#.into() },
        ],
    });
    s.event(ClaudeEvent::ToolResults(vec![ToolResult::new("t1", "file", false)]));
    h.drain();
    let msgs = h.controller(&id).messages().to_vec();
    assert!(msgs.iter().any(|m| m.role == Role::Tool && m.tool_result.as_deref() == Some("file")));
    assert_eq!(msgs.iter().find(|m| m.role == Role::Assistant).unwrap().text, "Hello!");
}

#[test]
fn process_exit_shows_error() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    h.sessions.get(0).emit(SessionOutput::Exited { status: 1, stderr: "auth expired".into() });
    h.drain();
    assert_eq!(last_message(&mut h, &id), (Role::Error, "auth expired".into()));
    assert!(!h.controller(&id).is_running());
    // Next send starts a new session (resume with the old id when known).
    send(&mut h, &id, "again");
    assert_eq!(h.sessions.count(), 2);
}

#[test]
fn send_fails_when_worktree_fails() {
    let mut h = Harness::new();
    *h.worktrees.ensure_error.lock().unwrap() = Some("no git".into());
    h.load();
    let a = h.anchor(0, 13);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    send(&mut h, &id, "q");
    assert_eq!(last_message(&mut h, &id), (Role::Error, "no git".into()));
    assert!(!h.controller(&id).is_running());
}

/// Not in the Swift suite: without git at all the checkout fails synchronously, while the
/// controller is checked out of the store; the error must still reach it.
#[test]
fn send_fails_when_git_is_missing() {
    let mut h = Harness::with(Opts { worktrees: false, ..Default::default() });
    h.load();
    let a = h.anchor(0, 13);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    send(&mut h, &id, "q");
    assert_eq!(last_message(&mut h, &id), (Role::Error, "git was not found".into()));
    assert!(!h.controller(&id).is_running());
}

#[test]
fn stop_interrupts() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    h.ctl(&id, |c, s| c.stop(s));
    assert_eq!(h.sessions.get(0).interrupts.load(Ordering::SeqCst), 1);
    assert_eq!(last_message(&mut h, &id).0, Role::System);
    assert!(!h.controller(&id).reducer.is_running());
}

#[test]
fn fix_it_commits_new_commits() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    let s = h.sessions.get(0);
    s.reply("Found it.", "sess-1");
    h.drain();
    assert!(!h.controller(&id).is_running());
    h.worktrees.set_head("base");
    h.worktrees.set_commits(vec![commit("old1", "earlier")]);
    h.act(|s| s.refresh_git_state(false));
    h.ctl(&id, |c, s| c.fix_it(s));
    assert_eq!(s.sent().last().map(String::as_str), Some(prompts::FIX_INSTRUCTION));
    assert_eq!(h.s().fix_status(&id).unwrap().status, FixStatus::Running);
    assert!(h.controller(&id).is_fixing());
    // The agent commits: HEAD moves and a new commit appears on top.
    h.worktrees.set_head("new1");
    h.worktrees.set_commits(vec![commit("new1", "fix"), commit("base", "earlier")]);
    s.reply("Committed.", "sess-1");
    h.drain();
    let f = h.s().fix_status(&id).unwrap().clone();
    assert_eq!(f.status, FixStatus::Committed);
    assert_eq!(f.commits, ["new1"]);
    assert_eq!(h.s().discuss_threads[&id].fix_commits, ["new1"]);
    assert_eq!(h.s().local_commits.len(), 2);
    assert!(!h.controller(&id).is_fixing());
}

#[test]
fn fix_it_without_commit_fails() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    let s = h.sessions.get(0);
    s.reply("ok", "sess-1");
    h.drain();
    assert!(!h.controller(&id).is_running());
    h.ctl(&id, |c, s| c.fix_it(s));
    s.reply("I could not fix it.", "sess-1");
    h.drain();
    let f = h.s().fix_status(&id).unwrap().clone();
    assert_eq!(f.status, FixStatus::Failed);
    assert_eq!(f.error.as_deref(), Some("No commit was created."));
}

#[test]
fn second_fix_is_queued_until_first_finishes() {
    let (mut h, id1) = setup();
    let a = h.anchor(1, 13);
    let id2 = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    send(&mut h, &id1, "a");
    h.sessions.get(0).reply("A", "s1");
    send(&mut h, &id2, "b");
    h.sessions.get(1).reply("B", "s2");
    h.drain();
    assert!(!h.controller(&id1).is_running() && !h.controller(&id2).is_running());
    h.ctl(&id1, |c, s| c.fix_it(s));
    h.ctl(&id2, |c, s| c.fix_it(s));
    assert_eq!(h.s().fix_status(&id1).unwrap().status, FixStatus::Running);
    assert_eq!(h.s().fix_status(&id2).unwrap().status, FixStatus::Queued);
    assert_ne!(h.sessions.get(1).sent().last().map(String::as_str), Some(prompts::FIX_INSTRUCTION));
    h.worktrees.set_head("x1");
    h.worktrees.set_commits(vec![commit("x1", "fix")]);
    h.sessions.get(0).reply("done", "s1");
    h.drain();
    assert_eq!(h.sessions.get(1).sent().last().map(String::as_str), Some(prompts::FIX_INSTRUCTION));
    assert_eq!(h.s().fix_status(&id2).unwrap().status, FixStatus::Running);
}

#[test]
fn convert_to_comment_forks_session() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    h.sessions.get(0).reply("answer", "S42");
    h.drain();
    assert!(!h.controller(&id).is_running());
    h.one_shot.set_text(Some("```markdown\nPlease add a test.\n```"));
    h.ctl(&id, |c, s| c.convert_to_comment(s));
    let anchor = h.s().discuss_threads[&id].anchor.clone();
    let cfg = h.one_shot.last_config();
    assert_eq!(cfg.resume_session_id.as_deref(), Some("S42"));
    assert!(cfg.fork_session);
    assert_eq!(cfg.permission_mode, "plan");
    assert_eq!(h.one_shot.last_prompt(), prompts::draft_comment(&anchor));
    assert_eq!(h.controller(&id).draft_state, TaskState::Idle);
    let composer = h.s().composers.values().next().unwrap().clone();
    assert_eq!(composer.text, "Please add a test.");
    assert_eq!(composer.origin.as_deref(), Some("Drafted from Discuss"));
    assert_eq!(composer.anchor(), Some(&anchor));
}

#[test]
fn convert_without_session_includes_transcript_and_failure() {
    let (mut h, id) = setup();
    h.one_shot.set_text(Some("Draft"));
    h.ctl(&id, |c, s| c.convert_to_comment(s));
    assert!(!h.one_shot.last_config().fork_session);
    assert_eq!(h.one_shot.last_config().resume_session_id, None);
    h.one_shot.set_error(Some("fail"));
    h.ctl(&id, |c, s| c.convert_to_comment(s));
    assert_eq!(h.controller(&id).draft_state, TaskState::Failed("fail".into()));
}

#[test]
fn clean_draft_strips_markdown_fences() {
    assert_eq!(clean_draft("```markdown\nHi\n```"), "Hi");
    assert_eq!(clean_draft("```md\nHi there\n```\n"), "Hi there");
    assert_eq!(clean_draft("  plain  "), "plain");
    assert_eq!(clean_draft("```suggestion\nx\n```"), "```suggestion\nx\n```");
}

#[test]
fn missing_claude_reports_not_installed() {
    let mut h = Harness::with(Opts { claude_path: None, ..Default::default() });
    h.load();
    let a = h.anchor(0, 13);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    send(&mut h, &id, "q");
    assert_eq!(last_message(&mut h, &id), (Role::Error, ClaudeError::NotInstalled.to_string()));
}

#[test]
fn shutdown_terminates_session() {
    let (mut h, id) = setup();
    send(&mut h, &id, "q");
    h.act(|s| s.delete_discuss(&id));
    assert!(h.sessions.get(0).terminated.load(Ordering::SeqCst));
}
