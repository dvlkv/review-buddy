//! Port of `Tests/ReviewBuddyUITests/SpecGapTests.swift`.

use std::sync::atomic::Ordering;

use review_buddy_core::diff::layout::RowKind;
use review_buddy_core::review::keybindings::{KeyBindings, KeyStroke, ReviewAction};
use review_buddy_core::*;

use super::support::*;
use crate::pr_store::ComposerKind;

#[test]
fn expand_footer_rows_are_shown() {
    let mut h = Harness::new();
    h.load();
    let s = h.s();
    let footers = s.layout.as_ref().unwrap().rows.iter().filter(|r| r.kind == RowKind::ExpandFooter).count();
    assert_eq!(footers, s.files().len());
    assert!(s.can_expand_below(0));
}

#[test]
fn expand_below_adds_context_after_last_hunk() {
    let mut h = Harness::new();
    h.load();
    let count = |h: &mut Harness| h.s().layout.as_ref().unwrap().rows.iter().filter(|r| r.kind == RowKind::Expanded).count();
    let before = count(&mut h);
    h.act(|s| s.expand_below(0, 20));
    assert!(count(&mut h) > before);
    let s = h.s();
    let last = s.files()[0].hunks.last().unwrap().clone();
    let first = s.layout.as_ref().unwrap().rows.iter().find(|r| r.kind == RowKind::Expanded).unwrap();
    assert_eq!(first.a, last.new_start + last.new_count);
}

#[test]
fn reply_key_targets_thread_reached_with_navigation() {
    let mut h = Harness::with(Opts { threads_every: 1, ..Default::default() });
    h.load();
    let s = h.s();
    s.navigate(ReviewAction::NextThread);
    let focused = s.focused_thread_id.clone().expect("a thread was reached");
    s.reply_to_focused_thread();
    assert!(s.composer_for_reply(&focused).is_some());
}

#[test]
fn reply_key_falls_back_to_first_visible_thread() {
    let mut h = Harness::with(Opts { threads_every: 1, ..Default::default() });
    h.load();
    let s = h.s();
    assert!(s.focused_thread_id.is_none());
    s.reply_to_focused_thread();
    assert!(s.composers.values().any(|c| matches!(c.kind, ComposerKind::Reply { .. })));
}

#[test]
fn findings_become_outdated_when_head_moves() {
    let mut h = Harness::new();
    h.load();
    let p0 = h.path(0);
    let s = h.s();
    let l = first_addition(s, 0);
    s.apply_pre_review(vec![AIFinding::new(&p0, DiffSide::Right, l, l, Severity::Low, "t", "b")]);
    assert!(!s.findings_are_outdated());
    let mut d = s.detail.clone().unwrap();
    d.head_oid = "c".repeat(40);
    s.detail = Some(d);
    assert!(s.findings_are_outdated());
}

#[test]
fn findings_head_survives_persistence() {
    let mut h = Harness::new();
    h.load();
    let p0 = h.path(0);
    let s = h.s();
    s.apply_pre_review(vec![AIFinding::new(&p0, DiffSide::Right, 1, 1, Severity::Low, "t", "b")]);
    let original = s.detail.as_ref().unwrap().head_oid.clone();
    let mut d = s.detail.clone().unwrap();
    d.head_oid = "d".repeat(40);
    s.detail = Some(d);
    s.persist();
    h.services.store.flush();
    let reloaded = h.services.store.load(&pr_ref());
    assert_eq!(reloaded.pre_review.and_then(|p| p.head_sha), Some(original));
}

fn set_state(h: &mut Harness, state: &str) {
    let s = h.s();
    let mut d = s.detail.clone().unwrap();
    d.state = state.into();
    s.detail = Some(d);
}

#[test]
fn merged_pr_worktree_is_removed_when_clean() {
    let mut h = Harness::new();
    h.load();
    h.worktrees.has_existing_worktree.store(true, Ordering::SeqCst);
    set_state(&mut h, "MERGED");
    h.act(|s| s.clean_up_worktree_if_closed());
    assert!(h.worktrees.calls().contains(&"remove".into()));
}

#[test]
fn merged_pr_worktree_kept_with_unpushed_commits() {
    let mut h = Harness::new();
    h.load();
    h.worktrees.has_existing_worktree.store(true, Ordering::SeqCst);
    h.worktrees.set_commits(vec![commit("abc1234", "fix")]);
    set_state(&mut h, "CLOSED");
    h.act(|s| s.clean_up_worktree_if_closed());
    assert!(!h.worktrees.calls().contains(&"remove".into()));
}

#[test]
fn closed_pr_worktree_not_removed_while_in_use() {
    let mut h = Harness::new();
    h.load();
    h.worktrees.has_existing_worktree.store(true, Ordering::SeqCst);
    h.ensure_worktree().unwrap(); // e.g. Claude is summarizing in it
    set_state(&mut h, "MERGED");
    h.act(|s| s.clean_up_worktree_if_closed());
    assert!(!h.worktrees.calls().contains(&"remove".into()));
}

#[test]
fn open_pr_worktree_not_touched() {
    let mut h = Harness::new();
    h.load();
    h.worktrees.has_existing_worktree.store(true, Ordering::SeqCst);
    h.act(|s| s.clean_up_worktree_if_closed());
    assert!(!h.worktrees.calls().contains(&"ensureExisting".into()));
}

#[test]
fn reply_is_bound_to_r() {
    assert_eq!(KeyBindings::action(&KeyStroke::new("r"), false), Some(ReviewAction::Reply));
}
