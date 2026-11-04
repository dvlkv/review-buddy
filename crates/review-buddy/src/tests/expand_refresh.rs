//! Port of `Tests/ReviewBuddyUITests/ExpandAndRefreshTests.swift`.
//!
//! Swift held loads/content fetches in flight with sleeps; here the late result's jobs are
//! taken off the queue (`take_jobs`) and applied after the newer work (`apply`).

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use review_buddy_core::diff::layout::{DiffLayout, DiffLayoutOptions, DiffViewMode, RowKind};
use review_buddy_core::support::synthetic::{Shape, SyntheticDiff};

use super::support::*;
use crate::pr_store::{diff_matches, split_lines, LoadState, PRStore};

const NEW_SHAPE: Shape = Shape::new(TEST_SHAPE.files, 3, 7);

fn expanded_lines(s: &PRStore, file: usize) -> Vec<i64> {
    s.layout.as_ref().unwrap().rows.iter().filter(|r| r.kind == RowKind::Expanded && r.file == file as i32).map(|r| r.a as i64).collect()
}

fn header_row(s: &PRStore, file: usize, hunk: usize) -> usize {
    s.layout
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|r| r.kind == RowKind::HunkHeader && r.file == file as i32 && r.a == hunk as i32)
        .unwrap()
}

/// Expands above `hunk` until nothing is hidden, letting each step land.
fn expand_fully(h: &mut Harness, file: usize, hunk: usize) -> i64 {
    let mut clicks = 0;
    while h.s().can_expand_above(file, hunk) && clicks < 100 {
        h.act(|s| s.expand_above(file, hunk, 20));
        clicks += 1;
    }
    clicks
}

fn new_diff() -> Vec<u8> {
    SyntheticDiff::make(NEW_SHAPE).into_bytes()
}

// MARK: - Expand context

#[test]
fn repeated_expand_above_fills_the_gap_and_stops() {
    let mut h = Harness::new();
    h.load();
    let f = h.s().files()[0].clone();
    let gap = (f.hunks[1].new_first() - f.hunks[0].new_end()) as i64;
    assert!(gap > 20, "needs at least two clicks");
    let header = header_row(h.s(), 0, 1);
    let clicks = expand_fully(&mut h, 0, 1);
    assert_eq!(clicks, (gap + 19) / 20);
    let s = h.s();
    assert!(!s.can_expand_above(0, 1));
    // Every hidden line shown exactly once, in order, right after the header.
    assert_eq!(expanded_lines(s, 0), (f.hunks[0].new_end() as i64..f.hunks[1].new_first() as i64).collect::<Vec<_>>());
    assert_eq!(header_row(s, 0, 1), header);
    assert_eq!(s.layout.as_ref().unwrap().rows[header].b, 0);
    // Never counts past the gap, so later clicks are no-ops.
    assert_eq!(s.options.expanded_above.get(&DiffLayoutOptions::key(0, 1)), Some(&gap));
    let v = s.layout_version;
    h.act(|s| s.expand_above(0, 1, 20));
    assert_eq!(h.s().layout_version, v);
}

#[test]
fn expand_above_first_hunk_reaches_line_one() {
    let mut h = Harness::new();
    h.load();
    expand_fully(&mut h, 2, 0);
    let s = h.s();
    assert_eq!(expanded_lines(s, 2), (1..s.files()[2].hunks[0].new_first() as i64).collect::<Vec<_>>());
    assert!(!s.can_expand_above(2, 0));
}

#[test]
fn rapid_clicks_fetch_the_file_once() {
    let mut h = Harness::with(Opts {
        flaky: true,
        worktrees: false,
        configure: Some(Box::new(|f| {
            *f.contents.lock().unwrap() =
                Some(Box::new(|path, _| (1..=400).map(|i| format!("{path} {i}")).collect::<Vec<_>>().join("\n").into_bytes()));
        })),
        ..Default::default()
    });
    h.load();
    let s = h.s();
    let gap = (s.files()[0].hunks[1].new_first() - s.files()[0].hunks[0].new_end()) as usize;
    // Both clicks land before the content arrives.
    s.expand_above(0, 1, 20);
    s.expand_above(0, 1, 20);
    h.drain();
    assert_eq!(expanded_lines(h.s(), 0).len(), gap.min(40));
    assert_eq!(h.flaky().content_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn expand_below_stops_at_end_of_file() {
    let mut h = Harness::new();
    h.load();
    let last_end = h.s().files()[0].hunks.last().unwrap().new_end() as i64;
    let total = last_end + 45;
    *h.worktrees.file_contents_text.lock().unwrap() = (1..=total).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n") + "\n";
    h.ensure_worktree().unwrap();
    for _ in 0..10 {
        let v = h.s().layout_version;
        h.act(|s| s.expand_below(0, 20));
        if h.s().layout_version == v {
            break;
        }
    }
    let s = h.s();
    // The trailing newline doesn't add a phantom empty line.
    assert_eq!(expanded_lines(s, 0).last(), Some(&total));
    assert_eq!(expanded_lines(s, 0).first(), Some(&last_end));
    assert!(!s.can_expand_below(0));
}

#[test]
fn checkout_without_the_commit_falls_back_to_github() {
    let mut h = Harness::new();
    *h.worktrees.file_contents_error.lock().unwrap() = Some("fatal: invalid object name".into());
    h.load();
    h.ensure_worktree().unwrap();
    h.act(|s| s.expand_above(0, 1, 20));
    assert!(!expanded_lines(h.s(), 0).is_empty());
    assert!(h.worktrees.calls().iter().any(|c| c.starts_with("fileContents:")));
    assert!(h.demo_calls().contains(&"fileContents".into()));
    assert_eq!(h.s().banner, None);
}

#[test]
fn split_lines_matches_diff_numbering() {
    assert_eq!(split_lines(b"a\nb\n"), ["a", "b"]);
    assert_eq!(split_lines(b"a\nb"), ["a", "b"]);
    assert_eq!(split_lines(b"a\r\nb\r\n"), ["a", "b"]);
    assert_eq!(split_lines(b"a\n\n"), ["a", ""]);
    assert!(split_lines(b"").is_empty());
    assert_eq!(split_lines("é\r\n日本\n".as_bytes()), ["é", "日本"]);
}

// MARK: - Refresh

/// Regression: after a push, a refresh in split view kept the old layout's split pairs
/// (same file count), so rows pointed at the old diff's lines.
#[test]
fn refresh_in_split_mode_lays_out_the_new_diff() {
    let mut h = Harness::new();
    h.load();
    h.s().set_mode(DiffViewMode::Split);
    expand_fully(&mut h, 0, 1);
    let texts = |s: &PRStore| -> Vec<String> {
        let d = s.diff.as_ref().unwrap();
        d.files[0].lines.iter().map(|l| d.text_of(l).to_string()).collect()
    };
    let old_text = texts(h.s());
    let c40 = "c".repeat(40);
    h.demo.simulate_push(&c40, Some(new_diff()));
    h.act(|s| s.refresh());
    let s = h.s();
    assert_eq!(s.detail.as_ref().unwrap().head_oid, c40);
    assert_eq!(s.files()[0].hunks.len(), 3);
    assert_ne!(texts(s), old_text);
    let fresh = DiffLayout::with_blocks(s.diff.clone().unwrap(), s.options.clone(), &s.blocks_by_file(), None);
    assert_eq!(s.layout.as_ref().unwrap().rows, fresh.rows);
    for r in s.layout.as_ref().unwrap().rows.iter().filter(|r| r.kind == RowKind::Pair) {
        let n = s.files()[r.file as usize].lines.len() as i32;
        assert!(r.a < n && r.b < n);
    }
    // Expansions and file text were for the old head.
    assert!(s.options.expanded_above.is_empty() && s.options.expanded_below.is_empty());
    assert!(s.file_content_cache.is_empty());
    assert!(expanded_lines(s, 0).is_empty());
}

/// Regression: pushing fix commits left the diff on the old head until the app was reopened.
#[test]
fn push_reloads_the_diff() {
    let mut h = Harness::new();
    h.load();
    h.ensure_worktree().unwrap();
    h.worktrees.set_commits(vec![commit("c1", "fix")]);
    h.act(|s| s.refresh_git_state(false));
    let new_head = "c".repeat(40);
    h.demo.simulate_push(&new_head, Some(new_diff()));
    assert!(h.await_done(|s, done| s.push(done)));
    assert_eq!(h.s().detail.as_ref().unwrap().head_oid, new_head);
    assert_eq!(h.s().files()[0].hunks.len(), 3);
}

fn start_discuss_on_file0(h: &mut Harness) -> String {
    let a = h.anchor(0, 13);
    h.act(|s| s.start_discuss(Some(a), None)).unwrap()
}

/// A Discuss agent that ran `git push` itself: the turn's end reloads the PR.
#[test]
fn discuss_turn_that_pushed_reloads_the_diff() {
    let mut h = Harness::new();
    h.load();
    let id = start_discuss_on_file0(&mut h);
    h.ctl(&id, |c, s| c.send(s, "push it"));
    let new_head = "c".repeat(40);
    h.worktrees.set_head(&new_head);
    h.demo.simulate_push(&new_head, Some(new_diff()));
    h.sessions.get(0).reply("Pushed.", "sess-1");
    h.drain();
    assert_eq!(h.s().detail.as_ref().unwrap().head_oid, new_head);
    assert_eq!(h.s().files()[0].hunks.len(), 3);
}

/// Someone else pushed: don't reload under the user, just flag it as outdated.
#[test]
fn discuss_turn_does_not_reload_for_others_pushes() {
    let mut h = Harness::new();
    h.load();
    let id = start_discuss_on_file0(&mut h);
    h.ctl(&id, |c, s| c.send(s, "hi"));
    let old_head = h.s().detail.as_ref().unwrap().head_oid.clone();
    h.demo.simulate_push(&"d".repeat(40), None);
    h.sessions.get(0).reply("Hello.", "sess-1");
    h.drain();
    assert!(h.s().is_outdated());
    assert_eq!(h.s().detail.as_ref().unwrap().head_oid, old_head);
}

#[test]
fn content_arriving_after_refresh_is_dropped() {
    let mut h = Harness::with(Opts {
        flaky: true,
        worktrees: false,
        configure: Some(Box::new(|f| {
            *f.contents.lock().unwrap() = Some(Box::new(|_, r| {
                (1..=400).map(|i| format!("OLD {r} {i}")).collect::<Vec<_>>().join("\n").into_bytes()
            }));
        })),
        ..Default::default()
    });
    h.load();
    h.s().expand_above(0, 1, 20);
    let late = h.take_jobs(); // the old head's file content, still "in flight"
    h.demo.simulate_push(&"c".repeat(40), Some(new_diff()));
    h.act(|s| s.refresh());
    h.apply(late);
    let s = h.s();
    assert!(s.file_content_cache.is_empty());
    assert!(s.options.expanded_above.is_empty());
    assert!(expanded_lines(s, 0).is_empty());
}

#[test]
fn large_diff_refresh_fetches_existing_checkout() {
    let mut h = Harness::with(Opts {
        flaky: true,
        configure: Some(Box::new(|f| f.diff_too_large.store(true, Ordering::SeqCst))),
        ..Default::default()
    });
    h.load();
    assert_eq!(h.s().diff_source, "local git");
    let before = h.worktrees.count("fetchPR");
    h.load();
    let calls = h.worktrees.calls();
    assert_eq!(h.worktrees.count("fetchPR"), before + 1);
    // Fetched before diffing, so the diff is of the new head.
    let last = |c: &str| calls.iter().rposition(|x| x == c).unwrap();
    assert!(last("fetchPR") < last("diff"));
}

#[test]
fn stale_diff_from_github_is_retried() {
    let full = SyntheticDiff::make(TEST_SHAPE);
    let last_file = full.rfind("diff --git").unwrap();
    let stale = full[..last_file].as_bytes().to_vec();
    let mut h = Harness::with(Opts {
        flaky: true,
        configure: Some(Box::new(move |f| *f.queued_diffs.lock().unwrap() = vec![stale])),
        ..Default::default()
    });
    h.load();
    assert_eq!(h.s().files().len(), TEST_SHAPE.files);
    assert_eq!(h.demo_count("diff"), 1); // the retry reached the real diff
}

#[test]
fn matching_diff_is_not_refetched() {
    let mut h = Harness::new();
    h.s().stale_diff_retry_delay = Duration::from_secs(10);
    h.load();
    assert_eq!(h.demo_count("diff"), 1);
    let s = h.s();
    assert!(diff_matches(s.diff.as_ref().unwrap(), s.detail.as_ref().unwrap()));
}

#[test]
fn local_diff_is_pinned_to_the_loaded_head() {
    let mut h = Harness::with(Opts {
        flaky: true,
        configure: Some(Box::new(|f| f.diff_too_large.store(true, Ordering::SeqCst))),
        ..Default::default()
    });
    h.load();
    let head = h.s().detail.as_ref().unwrap().head_oid.clone();
    assert!(h.worktrees.calls().contains(&format!("diff:{head}")));
    // Not fetched locally: fall back to the fetched branch tip rather than failing.
    *h.worktrees.unknown_heads.lock().unwrap() = HashSet::from([head]);
    h.load();
    assert_eq!(h.s().load_state, LoadState::Loaded);
    let last = h.worktrees.calls().into_iter().filter(|c| c.starts_with("diff:")).last();
    assert_eq!(last.as_deref(), Some("diff:refs/review-buddy/head-1"));
}

#[test]
fn refresh_returns_to_the_same_file() {
    let mut h = Harness::new();
    h.load();
    let target = h.path(3);
    let s = h.s();
    s.top_visible_row = s.layout.as_ref().unwrap().file_start_row[3] + 4;
    // The new diff drops file 0, renumbering every row.
    let text = SyntheticDiff::make(TEST_SHAPE);
    let second = text[1..].find("diff --git").unwrap() + 1;
    h.demo.simulate_push(&"c".repeat(40), Some(text[second..].as_bytes().to_vec()));
    h.act(|s| s.refresh());
    let s = h.s();
    let f = s.diff.as_ref().unwrap().file_index(&target).unwrap();
    assert_eq!(s.pending_scroll, Some(s.layout.as_ref().unwrap().file_start_row[f] + 4));
}

/// Two refreshes overlap and the older one's diff arrives last: the newer diff stays.
#[test]
fn older_load_finishing_last_does_not_win() {
    let mut h = Harness::with(Opts { flaky: true, ..Default::default() });
    h.load();
    h.s().load();
    let slow = h.take_jobs(); // load #2 got the old diff and is still "in flight"
    let c40 = "c".repeat(40);
    h.demo.simulate_push(&c40, Some(new_diff()));
    h.load();
    h.apply(slow);
    let s = h.s();
    assert_eq!(s.detail.as_ref().unwrap().head_oid, c40);
    assert_eq!(s.files()[0].hunks.len(), 3);
    assert_eq!(s.load_state, LoadState::Loaded);
}
