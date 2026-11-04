//! In-memory GitHub used by `--synthetic` launch mode and by store tests.
//! Mutations behave like the real API.

use std::sync::Mutex;

use chrono::{Duration, Utc};
use review_buddy_core::diff::parser::UnifiedDiffParser;
use review_buddy_core::github::client::{GitHubAPI, GitHubResult, MergeResult};
use review_buddy_core::support::synthetic::{Shape, SyntheticDiff};
use review_buddy_core::*;

struct Inner {
    diff: Vec<u8>,
    detail: PRDetail,
    next_id: i64,
    calls: Vec<String>,
}

pub struct DemoGitHub {
    inner: Mutex<Inner>,
}

impl DemoGitHub {
    pub fn new(diff: Vec<u8>, detail: PRDetail) -> Self {
        DemoGitHub { inner: Mutex::new(Inner { diff, detail, next_id: 1, calls: vec![] }) }
    }

    /// A synthetic PR of the given shape with a sprinkling of review threads.
    pub fn synthetic(shape: Shape, threads_every: usize) -> Self {
        let text = SyntheticDiff::make(shape);
        let diff = UnifiedDiffParser::parse(&text);
        let pr_ref = PRRef::new("demo", "synthetic", 1);
        let mut threads = vec![];
        let mut files = vec![];
        for (i, f) in diff.files.iter().enumerate() {
            files.push(PRFileMeta {
                path: f.path().to_string(),
                additions: f.additions as i64,
                deletions: f.deletions as i64,
                viewed: ViewedState::Unviewed,
            });
            if threads_every == 0 || i % threads_every != 0 {
                continue;
            }
            let Some(line) = f.lines.iter().find(|l| l.kind == LineKind::Addition) else { continue };
            let mut t = ReviewThread::new(
                format!("T{i}"),
                f.path(),
                Some(line.new_line as i64),
                vec![
                    ReviewComment::new(
                        format!("C{i}a"),
                        "Should this handle the empty case? `filtered.first` can be nil here.",
                        "octocat",
                        Utc::now() - Duration::hours(2),
                    ),
                    ReviewComment::new(
                        format!("C{i}b"),
                        "Good catch — the guard above covers it, but I'll add a test.",
                        "monalisa",
                        Utc::now() - Duration::hours(1),
                    ),
                ],
            );
            t.is_resolved = i % (threads_every * 3) == 0;
            threads.push(t);
        }
        let mut d = PRDetail::new(
            pr_ref.clone(),
            "PR_demo",
            &format!("Refactor processing pipeline across {} modules", shape.files),
            "main",
            "feature/pipeline",
            &"a".repeat(40),
            &"b".repeat(40),
        );
        d.body = "This PR reworks the item processing pipeline.\n\n- Moves scoring into `Result`\n- Adds async fetching\n- Updates call sites".into();
        d.author = "monalisa".into();
        d.head_repo_name_with_owner = Some(pr_ref.name_with_owner());
        d.maintainer_can_modify = true;
        d.viewer_login = "you".into();
        d.viewer_can_push = true;
        d.files = files;
        d.threads = threads;
        d.changed_files = diff.files.len() as i64;
        d.additions = diff.files.iter().map(|f| f.additions as i64).sum();
        d.deletions = diff.files.iter().map(|f| f.deletions as i64).sum();
        d.conversation = Some(demo_conversation());
        let mut status = PRMergeStatus::new("MERGEABLE", "BLOCKED");
        status.review_decision = Some("REVIEW_REQUIRED".into());
        status.checks = vec![
            MergeCheck { name: "build".into(), app: Some("GitHub Actions".into()), state: "SUCCESS".into(), url: None },
            MergeCheck { name: "test".into(), app: Some("GitHub Actions".into()), state: "FAILURE".into(), url: None },
            MergeCheck { name: "lint".into(), app: Some("GitHub Actions".into()), state: "PENDING".into(), url: None },
        ];
        status.allowed_merge_methods = Some(vec![MergeMethod::Merge, MergeMethod::Squash]);
        status.default_merge_method = Some(MergeMethod::Squash);
        d.status = Some(status);
        DemoGitHub::new(text.into_bytes(), d)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.clone()
    }

    fn record(&self, s: &str) {
        self.inner.lock().unwrap().calls.push(s.to_string());
    }

    fn new_id(&self, p: &str) -> String {
        let mut g = self.inner.lock().unwrap();
        g.next_id += 1;
        format!("{p}{}", g.next_id)
    }

    /// Simulates a push to the PR. `diff` replaces the PR's diff, as a push that changes code would.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn simulate_push(&self, head_oid: &str, diff: Option<Vec<u8>>) {
        let mut g = self.inner.lock().unwrap();
        g.detail.head_oid = head_oid.to_string();
        if let Some(d) = diff {
            g.diff = d;
        }
    }

    #[allow(dead_code)]
    pub fn set_detail(&self, f: impl FnOnce(&mut PRDetail)) {
        f(&mut self.inner.lock().unwrap().detail);
    }
}

fn demo_conversation() -> PRConversation {
    let base = Utc::now() - Duration::days(5);
    let at = |h: i64| base + Duration::hours(h);
    let comment = |id: &str, author: &str, body: &str, h: i64| ConversationComment {
        id: id.into(),
        author: author.into(),
        author_avatar_url: None,
        body: body.into(),
        created_at: at(h),
        url: None,
    };
    let review = |id: &str, author: &str, state: &str, body: &str, h: i64| ConversationReview {
        id: id.into(),
        author: author.into(),
        author_avatar_url: None,
        state: state.into(),
        body: body.into(),
        submitted_at: at(h),
        url: None,
    };
    let commit = |oid: &str, headline: &str, h: i64, checks: CheckState| ConversationCommit {
        oid: oid.into(),
        headline: headline.into(),
        author: "monalisa".into(),
        author_avatar_url: None,
        date: at(h),
        checks,
    };
    PRConversation {
        comments: vec![
            comment("IC1", "octocat", "Could we split the scoring change into its own PR?", 30),
            comment("IC2", "monalisa", "Kept it here — the call sites change together. Added tests instead.", 52),
        ],
        reviews: vec![
            review("RV1", "octocat", "CHANGES_REQUESTED", "A couple of edge cases need handling.", 31),
            review("RV2", "hubot", "APPROVED", "", 70),
        ],
        commits: vec![
            commit("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b", "Move scoring into Result", 0, CheckState::Success),
            commit("2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c", "Add async fetching", 5, CheckState::Success),
            commit("3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d", "Handle empty input; add tests", 50, CheckState::Failure),
        ],
    }
}

fn you_comment(id: String, body: &str, pending: bool) -> ReviewComment {
    let mut c = ReviewComment::new(id, body, "you", Utc::now());
    c.is_pending = pending;
    c.viewer_did_author = true;
    c
}

impl GitHubAPI for DemoGitHub {
    fn search(&self, _query: &str) -> GitHubResult<Vec<PRSummary>> {
        self.record("search");
        let d = self.inner.lock().unwrap().detail.clone();
        Ok(vec![PRSummary {
            pr_ref: d.pr_ref.clone(),
            title: d.title.clone(),
            author: d.author.clone(),
            author_avatar_url: None,
            updated_at: Utc::now(),
            is_draft: false,
            additions: d.additions,
            deletions: d.deletions,
            checks: CheckState::Success,
            review_decision: ReviewDecision::ReviewRequired,
            comment_count: 0,
        }])
    }

    fn pull_request(&self, _pr: &PRRef) -> GitHubResult<PRDetail> {
        self.record("pullRequest");
        Ok(self.inner.lock().unwrap().detail.clone())
    }

    fn diff(&self, _pr: &PRRef) -> GitHubResult<Vec<u8>> {
        self.record("diff");
        Ok(self.inner.lock().unwrap().diff.clone())
    }

    fn head_oid(&self, _pr: &PRRef) -> GitHubResult<String> {
        self.record("headOid");
        Ok(self.inner.lock().unwrap().detail.head_oid.clone())
    }

    fn file_contents(&self, _owner: &str, _repo: &str, path: &str, _git_ref: &str) -> GitHubResult<Vec<u8>> {
        self.record("fileContents");
        let lines: Vec<String> = (1..=400).map(|i| format!("    // {path} line {i}")).collect();
        Ok(lines.join("\n").into_bytes())
    }

    fn start_review(&self, _pull_request_id: &str) -> GitHubResult<String> {
        self.record("startReview");
        let id = self.new_id("R");
        self.inner.lock().unwrap().detail.pending_review_id = Some(id.clone());
        Ok(id)
    }

    fn add_thread(&self, _pr_id: &str, _review_id: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<ReviewThread> {
        self.record("addThread");
        let mut t = ReviewThread::new(self.new_id("T"), anchor.path.clone(), Some(anchor.end_line), vec![]);
        t.start_line = anchor.is_multiline().then_some(anchor.start_line);
        t.side = anchor.side;
        t.comments.push(you_comment(self.new_id("C"), body, true));
        self.inner.lock().unwrap().detail.threads.push(t.clone());
        Ok(t)
    }

    fn reply(&self, _thread_id: &str, review_id: Option<&str>, body: &str) -> GitHubResult<ReviewComment> {
        self.record("reply");
        Ok(you_comment(self.new_id("C"), body, review_id.is_some()))
    }

    fn add_single_comment(&self, _pr: &PRRef, _commit: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<()> {
        self.record("addSingleComment");
        let mut t = ReviewThread::new(self.new_id("T"), anchor.path.clone(), Some(anchor.end_line), vec![]);
        t.start_line = anchor.is_multiline().then_some(anchor.start_line);
        t.side = anchor.side;
        let mut c = you_comment(self.new_id("C"), body, false);
        c.database_id = Some(self.inner.lock().unwrap().next_id);
        t.comments.push(c);
        self.inner.lock().unwrap().detail.threads.push(t);
        Ok(())
    }

    fn reply_single(&self, _pr: &PRRef, thread_id: &str, _db_id: i64, body: &str) -> GitHubResult<()> {
        self.record("replySingle");
        let c = you_comment(self.new_id("C"), body, false);
        let mut g = self.inner.lock().unwrap();
        if let Some(t) = g.detail.threads.iter_mut().find(|t| t.id == thread_id) {
            t.comments.push(c);
        }
        Ok(())
    }

    fn add_comment(&self, _subject_id: &str, body: &str) -> GitHubResult<ConversationComment> {
        self.record("addComment");
        Ok(ConversationComment {
            id: self.new_id("IC"),
            author: "you".into(),
            author_avatar_url: None,
            body: body.into(),
            created_at: Utc::now(),
            url: None,
        })
    }

    fn merge_pull_request(&self, _pr: &PRRef, _id: &str, method: MergeMethod, _oid: &str, _bypass: bool) -> GitHubResult<MergeResult> {
        self.record(&format!("merge:{}", method.as_str()));
        self.inner.lock().unwrap().detail.state = "MERGED".into();
        Ok(MergeResult::Merged)
    }

    fn set_pull_request_closed(&self, _id: &str, closed: bool) -> GitHubResult<()> {
        self.record(if closed { "close" } else { "reopen" });
        self.inner.lock().unwrap().detail.state = if closed { "CLOSED" } else { "OPEN" }.into();
        Ok(())
    }

    fn update_comment(&self, id: &str, body: &str) -> GitHubResult<ReviewComment> {
        self.record("updateComment");
        Ok(you_comment(id.to_string(), body, true))
    }

    fn delete_comment(&self, _id: &str) -> GitHubResult<()> {
        self.record("deleteComment");
        Ok(())
    }

    fn submit_review(&self, _review_id: &str, event: ReviewEvent, body: &str) -> GitHubResult<()> {
        self.record(&format!("submitReview:{}", event.as_str()));
        let state = match event {
            ReviewEvent::Approve => "APPROVED",
            ReviewEvent::RequestChanges => "CHANGES_REQUESTED",
            ReviewEvent::Comment => "COMMENTED",
        };
        let mut g = self.inner.lock().unwrap();
        g.detail.pending_review_id = None;
        let conv = g.detail.conversation.get_or_insert_with(Default::default);
        let n = conv.reviews.len() + 1;
        conv.reviews.push(ConversationReview {
            id: format!("R{n}"),
            author: "you".into(),
            author_avatar_url: None,
            state: state.into(),
            body: body.into(),
            submitted_at: Utc::now(),
            url: None,
        });
        Ok(())
    }

    fn delete_review(&self, _review_id: &str) -> GitHubResult<()> {
        self.record("deleteReview");
        Ok(())
    }

    fn set_resolved(&self, _thread_id: &str, resolved: bool) -> GitHubResult<bool> {
        self.record(&format!("setResolved:{resolved}"));
        Ok(resolved)
    }

    fn set_viewed(&self, _pr_id: &str, _path: &str, viewed: bool) -> GitHubResult<()> {
        self.record(&format!("setViewed:{viewed}"));
        Ok(())
    }
}
