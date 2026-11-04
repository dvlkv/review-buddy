//! Port of `Tests/ReviewBuddyUITests/PRStoreTests.swift`.

use std::collections::HashSet;
use std::time::Duration;

use review_buddy_core::diff::layout::{DiffViewMode, InlineBlockKind, RowKind};
use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::github::client::GitHubAPI;
use review_buddy_core::persistence::{LocalStore, PRLocalState};
use review_buddy_core::review::keybindings::ReviewAction;
use review_buddy_core::*;

use super::support::*;
use crate::file_tree::FileTreeBuilder;
use crate::pr_store::{LoadState, TaskState, WorktreeState};

// MARK: - Load

#[test]
fn load_installs_everything() {
    let mut h = Harness::new();
    let v0 = h.s().layout_version;
    h.load();
    let s = h.s();
    assert_eq!(s.load_state, LoadState::Loaded);
    assert_eq!(s.detail.as_ref().unwrap().pr_ref, pr_ref());
    assert_eq!(s.files().len(), 6);
    assert!(s.layout.as_ref().unwrap().row_count() > 0);
    assert!(s.threads.contains_key("T0"));
    assert!(s.viewed.is_empty());
    assert!(s.pending_review_id.is_none());
    assert_eq!(s.diff_source, "GitHub API");
    assert!(s.layout_version > v0);
    let calls = h.demo_calls();
    assert!(calls.contains(&"pullRequest".into()) && calls.contains(&"diff".into()));
}

#[test]
fn install_with_detail_marks_viewed_collapsed() {
    let mut h = Harness::new();
    let mut d = h.demo.pull_request(&pr_ref()).unwrap();
    d.files[2].viewed = ViewedState::Viewed;
    d.pending_review_id = Some("R9".into());
    let diff = UnifiedDiffParser::parse_bytes(h.demo.diff(&pr_ref()).unwrap());
    let s = h.s();
    s.install(diff, Some(d));
    assert!(s.is_viewed(2));
    assert_eq!(s.options.collapsed, HashSet::from([2]));
    assert_eq!(s.pending_review_id.as_deref(), Some("R9"));
    assert_eq!(s.viewed_count(), 1);
    assert_eq!(s.layout.as_ref().unwrap().row_range_for_file(2).len(), 1);
}

#[test]
fn layout_rebuilds_on_mode_collapse_force_load() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    let v0 = s.layout_version;
    s.set_mode(DiffViewMode::Split);
    assert!(s.view_mode() == DiffViewMode::Split && s.layout.as_ref().unwrap().options.mode == DiffViewMode::Split);
    assert_eq!(s.services.settings().view_mode, DiffViewMode::Split);
    assert!(s.layout.as_ref().unwrap().rows.iter().any(|r| r.kind == RowKind::Pair));
    s.toggle_mode();
    assert_eq!(s.view_mode(), DiffViewMode::Unified);
    s.toggle_collapsed(1);
    assert_eq!(s.layout.as_ref().unwrap().row_range_for_file(1).len(), 1);
    s.toggle_collapsed(1);
    assert!(s.layout.as_ref().unwrap().row_range_for_file(1).len() > 1);
    s.options.collapsed.insert(3);
    s.force_load(3);
    assert!(s.options.force_loaded.contains(&3) && !s.options.collapsed.contains(&3));
    assert!(s.layout_version > v0 + 3);
}

#[test]
fn apply_settings_uses_threshold() {
    let mut h = Harness::new();
    h.load();
    h.services.update_settings(|s| s.large_file_threshold = 1);
    h.s().apply_settings();
    assert!(h.s().layout.as_ref().unwrap().rows.iter().any(|r| r.kind == RowKind::Placeholder));
}

// MARK: - Viewed

#[test]
fn toggle_viewed_is_optimistic_and_calls_github() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    // Rust scrolls back only when the file header is above the viewport (reading inside file 1).
    let start1 = s.layout.as_ref().unwrap().file_start_row[1];
    s.top_visible_row = start1 + 2;
    s.toggle_viewed(1);
    assert!(s.is_viewed(1));
    assert!(s.options.collapsed.contains(&1));
    assert_eq!(s.pending_scroll, Some(s.layout.as_ref().unwrap().file_start_row[1]));
    h.drain();
    assert!(h.demo_calls().contains(&"setViewed:true".into()));
    h.s().toggle_viewed(1);
    assert!(!h.s().is_viewed(1) && !h.s().options.collapsed.contains(&1));
    h.drain();
    assert!(h.demo_calls().contains(&"setViewed:false".into()));
}

#[test]
fn viewed_reverts_on_failure() {
    let mut h = Harness::with(Opts { flaky: true, configure: Some(Box::new(|f| f.fail_viewed.store(true, std::sync::atomic::Ordering::SeqCst))), ..Default::default() });
    h.load();
    h.s().toggle_viewed(0);
    assert!(h.s().is_viewed(0));
    h.drain();
    assert!(!h.s().is_viewed(0));
    assert!(h.s().banner.as_deref().unwrap_or("").contains("viewed failed"));
}

#[test]
fn out_of_range_is_ignored() {
    let mut h = Harness::new();
    h.load();
    h.act(|s| s.toggle_viewed(99));
    assert!(!h.s().is_viewed(99));
}

// MARK: - Selection

#[test]
fn select_extend_and_lines() {
    let mut h = Harness::new();
    h.load();
    let p0 = h.path(0);
    let s = h.s();
    s.select(0, DiffSide::Right, 10, false, None);
    s.select(0, DiffSide::Right, 13, true, None);
    let sel = s.selection.clone().unwrap();
    assert_eq!(sel.anchor(), CodeAnchor::new(p0.clone(), DiffSide::Right, 10, 13));
    assert!(sel.contains(DiffSide::Right, 12) && !sel.contains(DiffSide::Left, 12));
    let lines = s.lines_for(&sel.anchor());
    assert_eq!(lines.len(), 4);
    assert!(lines[3].ends_with("// v2"), "{lines:?}"); // line 13 is the first addition
    // Different side starts a new selection.
    s.select(0, DiffSide::Left, 14, true, None);
    assert!(s.selection.as_ref().unwrap().side == DiffSide::Left && s.selection.as_ref().unwrap().start == 14);
    // Left side excludes additions.
    assert!(s.lines_for(&CodeAnchor::new(p0.clone(), DiffSide::Left, 13, 15)).iter().all(|l| !l.ends_with("// v2")));
    s.select(0, DiffSide::Right, 0, false, None); // invalid line ignored
    assert_eq!(s.selection.as_ref().unwrap().start, 14);
    s.clear_selection();
    assert!(s.selection.is_none());
    assert!(s.lines_for(&CodeAnchor::new("nope", DiffSide::Right, 1, 2)).is_empty());
}

#[test]
fn text_selection_within_and_across_lines() {
    let mut h = Harness::new();
    h.load();
    let p0 = h.path(0);
    let s = h.s();
    let line10 = s.lines_for(&CodeAnchor::new(p0, DiffSide::Right, 10, 10))[0].clone();
    // Within one line, dragged right to left.
    s.select(0, DiffSide::Right, 10, false, Some(6));
    s.select(0, DiffSide::Right, 10, true, Some(2));
    assert!(s.selection.as_ref().unwrap().is_partial());
    assert_eq!(s.selection.as_ref().unwrap().column_range(10), Some(2..6));
    assert_eq!(s.selected_text().unwrap(), line10[2..6]);
    // Comments still anchor on whole lines.
    let a = s.selection.as_ref().unwrap().anchor();
    assert!(a.start_line == 10 && a.end_line == 10);
    // Across lines: the tail of the first line, the head of the last.
    s.select(0, DiffSide::Right, 10, false, Some(3));
    s.select(0, DiffSide::Right, 12, true, Some(4));
    let sel = s.selection.clone().unwrap();
    assert_eq!(sel.column_range(10), Some(3..usize::MAX));
    assert_eq!(sel.column_range(11), Some(0..usize::MAX));
    assert_eq!(sel.column_range(12), Some(0..4));
    assert_eq!(sel.column_range(13), None);
    let text: Vec<String> = s.selected_text().unwrap().split('\n').map(str::to_string).collect();
    assert!(text.len() == 3 && text[0] == line10[3..] && text[2].len() == 4, "{text:?}");
    // A click without a drag is an empty selection (the canvas clears it).
    s.select(0, DiffSide::Right, 10, false, Some(5));
    assert!(s.is_empty_text_selection());
    s.select(0, DiffSide::Right, 10, true, Some(7));
    assert!(!s.is_empty_text_selection());
    // Whole-line selections (gutter) are never "empty".
    s.select(0, DiffSide::Right, 10, false, None);
    assert!(!s.is_empty_text_selection() && s.selected_text().as_deref() == Some(line10.as_str()));
}

#[test]
fn double_click_word_range() {
    use crate::ui::diff_canvas::word_range;
    let t = "  const res = await fetch(`/api/${id}`);";
    let word = |col: usize| t[word_range(t, col)].to_string();
    assert_eq!(word(4), "const");
    assert_eq!(word(2), "const"); // first character
    assert_eq!(word(9), "res");
    assert_eq!(word(18), "await");
    assert_eq!(word(12), "="); // punctuation selects one character
    assert_eq!(word(32), "$"); // `$` joins identifiers, but `{` does not
    assert_eq!(word(35), "id");
    assert_eq!(word(999), ";"); // past the end: last character
    assert_eq!(word_range("", 3), 0..0);
}

// MARK: - Review order

#[test]
fn reading_order_moves_files_to_top_and_can_be_edited() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    let paths = |s: &crate::pr_store::PRStore| s.files().iter().map(|f| f.path().to_string()).collect::<Vec<_>>();
    let original = paths(s);
    // The diff follows the tree's order once loaded.
    let tree: Vec<String> = FileTreeBuilder::display_order(&original, &[]).into_iter().map(|i| original[i].clone()).collect();
    assert_eq!(original, tree);
    let a = original[original.len() - 1].clone();
    let b = original[original.len() / 2].clone();
    let sum = AISummary {
        summary: "x".into(),
        reading_order: vec![
            ReadingItem { path: a.clone(), reason: "core".into() },
            ReadingItem { path: b.clone(), reason: "uses".into() },
        ],
        risks: vec![],
        head_sha: None,
        created_at: None,
    };
    s.set_summary(Some(sum), TaskState::Idle);
    assert_eq!(paths(s)[..2], [a.clone(), b.clone()]);
    assert_eq!(s.review_first_count(), 2);
    assert_eq!(paths(s).len(), original.len());
    assert_eq!(paths(s).into_iter().collect::<HashSet<_>>(), original.iter().cloned().collect::<HashSet<_>>());
    // Viewed state follows the path, not the index.
    s.toggle_viewed(0);
    let ia = s.diff.as_ref().unwrap().file_index(&a).unwrap();
    assert!(s.is_viewed(ia));
    s.move_in_review_order(&b, -1);
    assert_eq!(paths(s)[..2], [b.clone(), a.clone()]);
    let c = original[1].clone();
    s.add_to_review_order(&c);
    assert_eq!(paths(s)[..3], [b.clone(), a.clone(), c.clone()]);
    s.remove_from_review_order(&a);
    assert_eq!(paths(s)[..2], [b.clone(), c.clone()]);
    assert_eq!(s.review_first_count(), 2);
    s.reset_review_order();
    assert_eq!(paths(s)[..2], [a, b]);
    h.drain();
}

// MARK: - Conversation, single comments, updates, merge

#[test]
fn posts_and_appends_comment() {
    let mut h = Harness::new();
    h.load();
    let count = |h: &mut Harness| h.s().detail.as_ref().unwrap().conversation.as_ref().map(|c| c.comments.len()).unwrap_or(0);
    let before = count(&mut h);
    h.act(|s| s.add_conversation_comment("  ", |_, _| panic!("blank comment must not be sent")));
    assert_eq!(h.demo_count("addComment"), 0);
    let r = h.await_done(|s, done| s.add_conversation_comment("Looks good", done));
    assert_eq!(r, None);
    assert_eq!(count(&mut h), before + 1);
    let last = h.s().detail.as_ref().unwrap().conversation.as_ref().unwrap().comments.last().unwrap().body.clone();
    assert_eq!(last, "Looks good");
}

#[test]
fn single_comment_publishes_without_review() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    assert!(s.pending_review_id.is_none());
    s.select(1, DiffSide::Right, 13, false, None);
    s.start_comment(None, "", None);
    let id = s.composers.keys().next().unwrap().clone();
    s.update_composer(&id, "nit".into());
    h.act(|s| s.submit_composer(&id, true));
    let s = h.s();
    assert!(s.composers.is_empty());
    assert!(s.pending_review_id.is_none()); // no review started
    let t = s.threads.values().find(|t| t.comments.first().map(|c| c.body.as_str()) == Some("nit")).unwrap().clone();
    assert!(t.comments.iter().all(|c| !c.is_pending));
    let calls = h.demo_calls();
    assert!(calls.contains(&"addSingleComment".into()) && !calls.contains(&"startReview".into()));

    let s = h.s();
    s.start_reply(&t.id);
    let rid = s.composers.keys().next().unwrap().clone();
    s.update_composer(&rid, "done".into());
    h.act(|s| s.submit_composer(&rid, true));
    let bodies: Vec<String> = h.s().threads[&t.id].comments.iter().map(|c| c.body.clone()).collect();
    assert_eq!(bodies, ["nit", "done"]);
    assert!(h.s().pending_review_id.is_none());
}

#[test]
fn detects_push_and_refresh_updates_checkout() {
    let mut h = Harness::new();
    h.load();
    h.ensure_worktree().unwrap();
    h.act(|s| s.check_for_updates(true));
    assert!(!h.s().is_outdated());
    let c40 = "c".repeat(40);
    h.demo.simulate_push(&c40, None);
    h.act(|s| s.check_for_updates(true));
    assert!(h.s().is_outdated());
    let before = h.worktrees.count("ensureWorktree");
    h.act(|s| s.refresh());
    assert!(!h.s().is_outdated());
    assert_eq!(h.s().detail.as_ref().unwrap().head_oid, c40);
    // Refresh goes through ensureWorktree, which fast-forwards a clean checkout.
    assert_eq!(h.worktrees.count("ensureWorktree"), before + 1);
}

#[test]
fn merge_reloads_as_merged() {
    let mut h = Harness::new();
    h.load();
    assert_eq!(h.s().detail.as_ref().unwrap().state, "OPEN");
    let r = h.await_done(|s, done| s.merge_pull_request(MergeMethod::Squash, false, done));
    assert_eq!(r, None);
    assert_eq!(h.s().detail.as_ref().unwrap().state, "MERGED");
}

#[test]
fn close_and_reopen() {
    let mut h = Harness::new();
    h.load();
    assert_eq!(h.await_done(|s, done| s.set_closed(true, done)), None);
    assert_eq!(h.s().detail.as_ref().unwrap().state, "CLOSED");
    assert_eq!(h.await_done(|s, done| s.set_closed(false, done)), None);
    assert_eq!(h.s().detail.as_ref().unwrap().state, "OPEN");
}

#[test]
fn can_merge_follows_merge_state() {
    let mut st = PRMergeStatus::new("MERGEABLE", "CLEAN");
    assert!(st.can_merge());
    assert!(!st.can_bypass());
    st.merge_state_status = "BLOCKED".into();
    assert!(!st.can_merge() && !st.can_bypass());
    st.viewer_can_merge_as_admin = Some(true);
    assert!(!st.can_merge() && st.can_bypass()); // only with the bypass checkbox
    st.mergeable = "CONFLICTING".into();
    assert!(!st.can_merge() && !st.can_bypass());
}

// MARK: - Comments

fn only_composer(h: &mut Harness) -> String {
    let s = h.s();
    assert_eq!(s.composers.len(), 1);
    s.composers.keys().next().unwrap().clone()
}

#[test]
fn pending_review_created_once_for_two_comments() {
    let mut h = Harness::new();
    h.load();
    let a113 = h.anchor(1, 13);
    let a214 = h.anchor(2, 14);
    let p1 = h.path(1);
    let s = h.s();
    let before = s.threads.len();
    s.select(1, DiffSide::Right, 13, false, None);
    s.start_comment(None, "", None);
    assert!(s.selection.is_none());
    let c1 = s.composers.values().next().unwrap().clone();
    assert_eq!(c1.anchor(), Some(&a113));
    assert_eq!(c1.anchor_lines.len(), 1);
    assert!(s.layout.as_ref().unwrap().blocks.iter().any(|b| b.id == c1.id && b.kind == InlineBlockKind::Composer));
    s.update_composer(&c1.id, "first".into());
    h.act(|s| s.submit_composer(&c1.id, false));
    assert!(h.s().composers.is_empty());
    assert!(h.s().pending_review_id.is_some());
    h.s().start_comment(Some(a214), "second", None);
    let id = only_composer(&mut h);
    h.act(|s| s.submit_composer(&id, false));
    assert_eq!(h.demo_count("startReview"), 1);
    assert_eq!(h.demo_count("addThread"), 2);
    let s = h.s();
    assert_eq!(s.threads.len(), before + 2);
    assert_eq!(s.pending_comment_count(), 2);
    assert_eq!(s.open_thread_count_by_path.get(&p1), Some(&1));
}

#[test]
fn empty_composer_not_submitted_and_reuse_same_anchor() {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(1, 13);
    let s = h.s();
    s.start_comment(Some(a.clone()), "", None);
    s.start_comment(Some(a), "filled", None);
    assert_eq!(s.composers.len(), 1);
    assert_eq!(s.composers.values().next().unwrap().text, "filled");
    let id = only_composer(&mut h);
    h.s().update_composer(&id, "   ".into());
    h.act(|s| s.submit_composer(&id, false));
    assert!(!h.demo_calls().contains(&"addThread".into()));
    let s = h.s();
    s.cancel_composer(&id);
    assert!(s.composers.is_empty());
    assert!(!s.layout.as_ref().unwrap().blocks.iter().any(|b| b.id == id));
}

#[test]
fn start_comment_without_selection_does_nothing() {
    let mut h = Harness::new();
    h.load();
    h.s().start_comment(None, "", None);
    assert!(h.s().composers.is_empty());
}

#[test]
fn reply_edit_delete() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    s.start_reply("T0");
    s.start_reply("T0");
    assert_eq!(s.composers.len(), 1);
    let reply = s.composer_for_reply("T0").unwrap().id.clone();
    s.update_composer(&reply, "thanks".into());
    h.act(|s| s.submit_composer(&reply, false));
    let s = h.s();
    assert_eq!(s.threads["T0"].comments.len(), 3);
    assert!(s.threads["T0"].comments.last().unwrap().is_pending);
    let last = s.threads["T0"].comments.last().unwrap().clone();
    s.start_edit("T0", &last);
    let edit = s.composer_for_edit(&last.id).unwrap().clone();
    assert_eq!(edit.text, "thanks");
    s.update_composer(&edit.id, "thanks!!".into());
    h.act(|s| s.submit_composer(&edit.id, false));
    assert_eq!(h.s().threads["T0"].comments.last().unwrap().body, "thanks!!");
    assert!(h.demo_calls().contains(&"updateComment".into()));
    h.act(|s| s.delete_comment("T0", &last.id));
    assert_eq!(h.s().threads["T0"].comments.len(), 2);
    let ids: Vec<String> = h.s().threads["T0"].comments.iter().map(|c| c.id.clone()).collect();
    for c in ids {
        h.act(|s| s.delete_comment("T0", &c));
    }
    assert!(!h.s().threads.contains_key("T0"));
}

#[test]
fn resolve_and_revert_on_failure() {
    let mut h = Harness::with(Opts { flaky: true, ..Default::default() });
    h.load();
    let p0 = h.path(0);
    assert!(h.s().threads["T0"].is_resolved); // synthetic T0 starts resolved
    h.act(|s| s.set_resolved("T0", false));
    assert!(!h.s().threads["T0"].is_resolved);
    assert_eq!(h.s().open_thread_count_by_path.get(&p0), Some(&1));
    h.flaky().fail_resolve.store(true, std::sync::atomic::Ordering::SeqCst);
    h.act(|s| s.set_resolved("T0", true));
    assert!(!h.s().threads["T0"].is_resolved);
    assert!(h.s().banner.is_some());
}

#[test]
fn submit_and_discard_review() {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(1, 13);
    h.s().start_comment(Some(a), "c", None);
    let id = only_composer(&mut h);
    h.act(|s| s.submit_composer(&id, false));
    h.s().review_body = "LGTM".into();
    let ok = h.await_done(|s, done| s.submit_review(ReviewEvent::Approve, done));
    assert!(ok);
    assert!(h.demo_calls().contains(&"submitReview:APPROVE".into()));
    let s = h.s();
    assert!(s.pending_review_id.is_none() && s.review_body.is_empty());
    assert_eq!(s.pending_comment_count(), 0);
    assert_eq!(s.banner.as_deref(), Some("Approved ✓"));
    assert_eq!(s.submit_state, TaskState::Idle);

    let a = h.anchor(2, 13);
    h.s().start_comment(Some(a), "d", None);
    let id = only_composer(&mut h);
    h.act(|s| s.submit_composer(&id, false));
    let before = h.s().threads.len();
    h.act(|s| s.discard_pending_review());
    assert!(h.demo_calls().contains(&"deleteReview".into()));
    assert!(h.s().pending_review_id.is_none());
    assert_eq!(h.s().threads.len(), before - 1);
}

#[test]
fn submitted_review_shows_in_summary_history() {
    let mut h = Harness::new();
    h.load();
    let a = h.anchor(1, 13);
    h.s().start_comment(Some(a), "c", None);
    let id = only_composer(&mut h);
    h.act(|s| s.submit_composer(&id, false));
    h.s().review_body = "Please fix the naming".into();
    assert!(h.await_done(|s, done| s.submit_review(ReviewEvent::RequestChanges, done)));
    let last = h.s().detail.as_ref().unwrap().conversation.as_ref().unwrap().reviews.last().unwrap().clone();
    assert_eq!(last.state, "CHANGES_REQUESTED");
    assert_eq!(last.body, "Please fix the naming");
}

#[test]
fn discard_without_pending_is_noop() {
    let mut h = Harness::new();
    h.load();
    h.act(|s| s.discard_pending_review());
    assert!(!h.demo_calls().contains(&"deleteReview".into()));
}

// MARK: - Navigation

#[test]
fn navigate_files_hunks_threads() {
    let mut h = Harness::with(Opts { threads_every: 2, ..Default::default() });
    h.load();
    let (starts, rows) = {
        let l = h.s().layout.as_ref().unwrap();
        (l.file_start_row.clone(), l.rows.clone())
    };
    struct L {
        file_start_row: Vec<usize>,
        rows: Vec<review_buddy_core::diff::layout::LayoutRow>,
    }
    let l = L { file_start_row: starts, rows };
    let nav = |h: &mut Harness, a: ReviewAction| {
        h.s().navigate(a);
        h.take_scroll()
    };
    h.s().top_visible_row = 0;
    assert_eq!(nav(&mut h, ReviewAction::NextFile), Some(l.file_start_row[1]));
    assert_eq!(nav(&mut h, ReviewAction::NextFile), Some(l.file_start_row[2]));
    h.s().top_visible_row = l.file_start_row[2] + 3;
    assert_eq!(nav(&mut h, ReviewAction::PreviousFile), Some(l.file_start_row[2]));
    assert_eq!(nav(&mut h, ReviewAction::PreviousFile), Some(l.file_start_row[1]));
    h.s().top_visible_row = 0;
    let first_hunk = l.rows.iter().position(|r| r.kind == RowKind::HunkHeader).unwrap();
    assert_eq!(nav(&mut h, ReviewAction::NextHunk), Some(first_hunk));
    let next = nav(&mut h, ReviewAction::NextHunk).unwrap();
    assert!(l.rows[next].kind == RowKind::HunkHeader && next > first_hunk);
    assert_eq!(nav(&mut h, ReviewAction::PreviousHunk), Some(first_hunk));
    h.s().top_visible_row = 0;
    let first_block = l.rows.iter().position(|r| r.kind == RowKind::Block).unwrap();
    assert_eq!(nav(&mut h, ReviewAction::NextThread), Some(first_block.saturating_sub(3)));
    h.s().top_visible_row = l.rows.len() - 1;
    assert_eq!(nav(&mut h, ReviewAction::NextFile), None); // past the end: no scroll
    assert!(nav(&mut h, ReviewAction::PreviousThread).is_some());
}

#[test]
fn scroll_helpers() {
    let mut h = Harness::new();
    h.load();
    let p4 = h.path(4);
    let s = h.s();
    s.scroll_to_file(3);
    assert_eq!(s.pending_scroll, Some(s.layout.as_ref().unwrap().file_start_row[3]));
    s.scroll_to_block("T0");
    let block_row = s.layout.as_ref().unwrap().rows.iter().position(|r| r.kind == RowKind::Block).unwrap();
    assert_eq!(s.pending_scroll, Some(block_row.saturating_sub(4)));
    s.toggle_collapsed(4);
    s.scroll_to_anchor(&CodeAnchor::new(p4, DiffSide::Right, 13, 13));
    assert!(!s.options.collapsed.contains(&4));
    let row = s.layout.as_ref().unwrap().row(4, DiffSide::Right, 13).unwrap();
    assert_eq!(s.pending_scroll, Some(row - 4));
}

/// Swift also checked an `onCurrentFileChange` callback, which the Rust store doesn't have
/// (the immediate-mode UI reads `current_file` every frame); negative rows can't be expressed.
#[test]
fn report_top_row_updates_current_file() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    let start2 = s.layout.as_ref().unwrap().file_start_row[2];
    s.report_top_row(start2 + 1);
    s.report_top_row(start2 + 2);
    s.report_top_row(10_000_000); // out of range: ignored
    assert_eq!(s.current_file, 2);
}

// MARK: - Expand

#[test]
fn expand_above_and_below_uses_file_contents() {
    let mut h = Harness::new();
    h.load();
    let p0 = h.path(0);
    let s = h.s();
    assert!(!s.can_expand_above(0, 0) || s.files()[0].hunks[0].new_start > 1);
    assert!(s.can_expand_above(0, 1));
    assert!(s.can_expand_below(0));
    assert!(!s.can_expand_above(99, 0));
    h.act(|s| s.expand_above(0, 1, 20));
    let s = h.s();
    let expanded: Vec<_> = s.layout.as_ref().unwrap().rows.iter().filter(|r| r.kind == RowKind::Expanded).copied().collect();
    assert_eq!(expanded.len(), 20);
    assert!(h.demo_calls().contains(&"fileContents".into()));
    // Lines from fetched content are available to lines_for.
    let n = expanded[0].a as i64;
    let s = h.s();
    let got = s.lines_for(&CodeAnchor::new(p0, DiffSide::Right, n, n));
    assert!(got.first().map(|l| l.contains(&format!("line {n}"))).unwrap_or(false), "{got:?}");
    // Demo content has 400 lines; expanding below is capped by file length.
    h.act(|s| s.expand_below(0, 20));
    assert_eq!(h.s().options.expanded_below.get(&0), Some(&20));
    assert!(h.s().can_expand_below(0));
}

#[test]
fn expand_uses_worktree_when_ready() {
    let mut h = Harness::new();
    h.load();
    h.ensure_worktree().unwrap();
    h.act(|s| s.expand_above(1, 1, 20));
    assert!(h.worktrees.calls().iter().any(|c| c.starts_with("fileContents:")));
}

// MARK: - Persistence

#[test]
fn discuss_thread_round_trip() {
    let root = temp_dir("rb-rt");
    let mut h = Harness::with(Opts { root: Some(root.clone()), ..Default::default() });
    h.load();
    let a = h.anchor(0, 13);
    let p0 = h.path(0);
    let id = h.act(|s| s.start_discuss(Some(a), None)).unwrap();
    h.services.store.flush();
    let mut h2 = Harness::with(Opts { root: Some(root), ..Default::default() });
    h2.load();
    assert!(h2.s().discuss_threads.contains_key(&id));
    assert_eq!(h2.discuss_paths(), [p0]);
    assert!(h2.s().layout.as_ref().unwrap().blocks.iter().any(|b| b.id == id && b.kind == InlineBlockKind::Discuss));
}

#[test]
fn interrupted_fix_requests_become_failed_on_reload() {
    let root = temp_dir("rb-rt");
    {
        let store = LocalStore::new(root.clone());
        let mut f = FixRequest::new("x", CodeAnchor::new("a", DiffSide::Right, 1, 1), "");
        f.status = FixStatus::Running;
        let state = PRLocalState { fix_requests: vec![f], ..Default::default() };
        store.save_with_debounce(&pr_ref(), state, Duration::ZERO);
        store.flush();
    }
    let mut h = Harness::with(Opts { root: Some(root), ..Default::default() });
    h.load();
    let f = h.s().fix_requests.first().cloned().unwrap();
    assert_eq!(f.status, FixStatus::Failed);
    assert_eq!(f.error.as_deref(), Some("Interrupted"));
}

// MARK: - Git

#[test]
fn push_success_and_failure() {
    let mut h = Harness::new();
    h.load();
    assert!(!h.await_done(|s, done| s.push(done))); // no worktree yet
    h.ensure_worktree().unwrap();
    h.worktrees.set_commits(vec![commit("c1", "fix")]);
    h.act(|s| s.refresh_git_state(false));
    let s = h.s();
    assert_eq!(s.local_commits.len(), 1);
    assert!(s.can_push() && s.push_disabled_reason().is_none());
    assert!(h.await_done(|s, done| s.push(done)));
    let s = h.s();
    assert_eq!(s.push_state, TaskState::Idle);
    assert!(s.local_commits.is_empty());
    assert!(s.banner.as_deref().unwrap_or("").contains("Pushed 1 commit"));
    *h.worktrees.push_error.lock().unwrap() = Some("rejected".into());
    assert!(!h.await_done(|s, done| s.push(done)));
    match &h.s().push_state {
        TaskState::Failed(e) => assert!(e.contains("rejected")),
        other => panic!("expected failure, got {other:?}"),
    }
}

#[test]
fn push_disabled_reason_for_fork() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    let mut d = s.detail.clone().unwrap();
    d.head_repo_name_with_owner = Some("someone/fork".into());
    d.maintainer_can_modify = false;
    d.viewer_login = "me".into();
    s.detail = Some(d.clone());
    assert!(!s.can_push());
    assert!(s.push_disabled_reason().unwrap().contains("fork"));
    d.head_repo_name_with_owner = Some(pr_ref().name_with_owner());
    d.viewer_can_push = false;
    s.detail = Some(d);
    assert!(s.push_disabled_reason().unwrap().contains("push access"));
}

#[test]
fn ensure_worktree_failure_and_missing_git() {
    let mut h = Harness::new();
    h.load();
    *h.worktrees.ensure_error.lock().unwrap() = Some("clone failed".into());
    assert_eq!(h.ensure_worktree(), Err("clone failed".into()));
    assert_eq!(h.s().worktree_state(), WorktreeState::Failed("clone failed".into()));
    let mut h2 = Harness::with(Opts { worktrees: false, ..Default::default() });
    h2.load();
    assert!(h2.ensure_worktree().is_err());
}

#[test]
fn ensure_worktree_is_shared_across_concurrent_callers() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    s.ensure_worktree_then(|_, r| assert!(r.is_ok()));
    s.ensure_worktree_then(|_, r| assert!(r.is_ok()));
    h.drain();
    assert_eq!(h.worktrees.count("ensureWorktree"), 1);
    h.act(|s| s.rebase_onto_remote());
    assert!(h.worktrees.calls().contains(&"rebase".into()));
}
