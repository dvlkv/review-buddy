//! The store-logic parts of `Tests/ReviewBuddyUITests/CanvasTests.swift`: highlight
//! cache, file tree builder, inbox and app model.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::persistence::AppSettings;
use review_buddy_core::support::synthetic::{Shape, SyntheticDiff};

use super::support::*;
use crate::file_tree::FileTreeBuilder;
use crate::pr_store::LoadState;

fn p(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// MARK: - Highlight cache (RendererTests)

#[test]
fn highlight_cache_sync_for_small_files() {
    let mut h = Harness::new();
    let d = UnifiedDiffParser::parse(&SyntheticDiff::make(Shape::new(2, 1, 12)));
    let n = d.files[0].lines.len();
    h.s().install(d, None);
    h.drain();
    let t = h.s().tokens(0).expect("small files highlight now");
    assert_eq!(t.len(), n);
    assert!(h.s().tokens(1).is_some());
    // Small files are never queued for background work.
    assert!(h.take_jobs().is_empty());
}

#[test]
fn highlight_cache_async_for_large_files() {
    let mut h = Harness::new();
    let d = UnifiedDiffParser::parse(&SyntheticDiff::make(Shape::new(1, 220, 20)));
    let n = d.files[0].lines.len();
    assert!(n > 4000);
    h.s().install(d, None);
    h.drain();
    assert!(h.s().tokens(0).is_none());
    assert!(h.s().tokens(0).is_none()); // requested once
    h.drain();
    assert_eq!(h.s().tokens(0).map(|t| t.len()), Some(n));
}

// MARK: - File tree

#[test]
fn display_order_matches_tree() {
    // Folders first, then files, case-insensitive — and "Feature10" before "Feature2".
    let paths = p(&["b/x.go", "a.go", "a/z.go", "B.go", "f/Feature2/y.go", "f/Feature10/y.go"]);
    assert_eq!(FileTreeBuilder::display_order(&paths, &[]), [2, 0, 5, 4, 1, 3]);
    assert_eq!(FileTreeBuilder::display_order(&paths, &[3, 1]), [3, 1, 2, 0, 5, 4]);
    let rows = FileTreeBuilder::rows(&paths, "", &HashSet::new(), &[3]);
    assert!(rows[0].is_section && rows[1].file_index == Some(3) && rows[1].rank == Some(1));
    assert_eq!(rows[2].name, "All files");
    assert!(!rows[3..].iter().any(|r| r.file_index == Some(3))); // listed once
}

#[test]
fn compression_and_ordering() {
    let paths = p(&["src/app/main.go", "src/app/main_test.go", "src/app/a/x.go", "README.md", "docs/guide/intro.md"]);
    let rows = FileTreeBuilder::rows(&paths, "", &HashSet::new(), &[]);
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["docs/guide", "intro.md", "src/app", "a", "x.go", "main.go", "main_test.go", "README.md"]);
    assert_eq!(rows.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 0, 1, 2, 1, 1, 0]);
    assert_eq!(rows.iter().find(|r| r.name == "main.go").unwrap().file_index, Some(0));
    assert_eq!(rows.iter().filter(|r| r.is_folder).count(), 3);
}

#[test]
fn filter_and_collapse() {
    let paths = p(&["src/a.go", "src/b.go", "lib/c.go"]);
    let names = |rows: Vec<crate::file_tree::FileTreeRow>| rows.into_iter().map(|r| r.name).collect::<Vec<_>>();
    assert_eq!(names(FileTreeBuilder::rows(&paths, "B.GO", &HashSet::new(), &[])), ["src", "b.go"]);
    let collapsed = HashSet::from(["src/".to_string()]);
    assert_eq!(names(FileTreeBuilder::rows(&paths, "", &collapsed, &[])), ["lib", "c.go", "src"]);
    assert!(FileTreeBuilder::rows(&paths, "zzz", &HashSet::new(), &[]).is_empty());
}

// MARK: - Inbox and app (InboxAndAppTests)

#[test]
fn inbox_sections_and_refresh() {
    let settings = AppSettings { pinned_repos: vec!["o/r".into()], ..Default::default() };
    let mut h = Harness::with(Opts { settings, ..Default::default() });
    let ids: Vec<String> = h.app.inbox.sections.iter().map(|s| s.id.clone()).collect();
    assert_eq!(ids, ["reviewRequested", "mine", "repo:o/r"]);
    assert!(h.app.inbox.sections[2].query.contains("repo:o/r"));
    h.app.refresh_inbox();
    h.drain();
    assert!(h.app.inbox.sections.iter().all(|s| s.items.len() == 1 && !s.is_loading && s.error.is_none()));
    assert!(h.app.inbox.last_refresh.is_some());
    h.services.update_settings(|s| s.pinned_repos.clear());
    h.app.pinned_repos_changed();
    assert_eq!(h.app.inbox.sections.len(), 2);
    assert_eq!(h.app.inbox.sections[0].items.len(), 1); // items kept across rebuilds
    h.drain();
}

#[test]
fn inbox_per_section_errors() {
    let mut h = Harness::with(Opts {
        flaky: true,
        configure: Some(Box::new(|f| f.fail_search.store(true, Ordering::SeqCst))),
        ..Default::default()
    });
    h.app.refresh_inbox();
    h.drain();
    let secs = &h.app.inbox.sections;
    assert!(secs[0].error.is_none() && secs[0].items.len() == 1);
    assert_eq!(secs[1].error.as_deref(), Some("search failed"));
}

/// A harness whose `AppState` opens the PR itself (no pre-installed store).
fn empty_app() -> Harness {
    let mut h = Harness::new();
    h.app.prs.clear();
    h.app.current = None;
    h
}

#[test]
fn app_model_caches_stores() {
    let mut h = empty_app();
    h.app.open(pr_ref());
    h.drain();
    assert_eq!(h.app.current_pr().unwrap().load_state, LoadState::Loaded);
    h.app.current_pr_mut().unwrap().banner = Some("marker".into());
    h.app.open(pr_ref());
    h.drain();
    assert_eq!(h.app.prs.len(), 1);
    assert_eq!(h.app.current_pr().unwrap().banner.as_deref(), Some("marker")); // same store
    assert!(!h.app.open_url("not a url"));
    assert_eq!(h.app.current, Some(pr_ref()));
}

#[test]
fn open_by_url_parses() {
    let mut h = empty_app();
    assert!(h.app.open_url("https://github.com/demo/synthetic/pull/1/files"));
    h.drain();
    assert_eq!(h.app.current, Some(pr_ref()));
}
