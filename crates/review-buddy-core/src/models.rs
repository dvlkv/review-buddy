//! Shared data types: diffs, pull requests, review threads, AI results.
//! JSON field names match the Swift app's Codable output so existing
//! `~/Library/Application Support/ReviewBuddy` files keep loading.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

pub type Date = DateTime<Utc>;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

// MARK: - Diff

/// Which side of a diff a line or anchor belongs to. Matches GitHub's `DiffSide`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DiffSide {
    /// Old version (GitHub `LEFT`).
    #[serde(rename = "LEFT")]
    Left,
    /// New version (GitHub `RIGHT`).
    #[serde(rename = "RIGHT")]
    #[default]
    Right,
}

impl DiffSide {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffSide::Left => "LEFT",
            DiffSide::Right => "RIGHT",
        }
    }
    pub fn parse(s: &str) -> Option<DiffSide> {
        match s {
            "LEFT" => Some(DiffSide::Left),
            "RIGHT" => Some(DiffSide::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
    /// `\ No newline at end of file`
    NoNewline,
}

/// A single line of a parsed diff. Text is a byte range into the owning
/// `DiffSet::buffer` so a 200k-line diff costs no per-line allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line number in the old file, 0 when the line has none.
    pub old_line: i32,
    /// 1-based line number in the new file, 0 when the line has none.
    pub new_line: i32,
    /// Byte offset of the text (after the `+`/`-`/` ` marker).
    pub text_start: i32,
    pub text_length: i32,
}

impl DiffLine {
    pub fn line_number(&self, side: DiffSide) -> i32 {
        match side {
            DiffSide::Left => self.old_line,
            DiffSide::Right => self.new_line,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: i32,
    pub old_count: i32,
    pub new_start: i32,
    pub new_count: i32,
    /// Text after the closing `@@` (usually the enclosing function signature).
    pub heading: String,
    /// Range into `FileDiff::lines`.
    pub line_range: Range<usize>,
}

impl Hunk {
    /// First new-file line the hunk covers. A hunk with no new lines (`+4,0`) sits *after*
    /// line `new_start`, so the lines before it run through `new_start` itself.
    pub fn new_first(&self) -> i32 {
        if self.new_count == 0 { self.new_start + 1 } else { self.new_start }
    }
    /// First new-file line after the hunk.
    pub fn new_end(&self) -> i32 {
        self.new_first() + self.new_count
    }
    pub fn old_first(&self) -> i32 {
        if self.old_count == 0 { self.old_start + 1 } else { self.old_start }
    }
    pub fn old_end(&self) -> i32 {
        self.old_first() + self.old_count
    }
    pub fn header_text(&self) -> String {
        let mut s = format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count
        );
        if !self.heading.is_empty() {
            s.push(' ');
            s.push_str(&self.heading);
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    pub status: FileStatus,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
    pub lines: Vec<DiffLine>,
    pub additions: usize,
    pub deletions: usize,
}

impl FileDiff {
    pub fn new(old_path: impl Into<String>, new_path: impl Into<String>, status: FileStatus) -> Self {
        FileDiff {
            old_path: old_path.into(),
            new_path: new_path.into(),
            status,
            is_binary: false,
            hunks: vec![],
            lines: vec![],
            additions: 0,
            deletions: 0,
        }
    }
    /// The path GitHub uses to address the file (the new path, or the old one for deletions).
    pub fn path(&self) -> &str {
        if self.status == FileStatus::Deleted { &self.old_path } else { &self.new_path }
    }
    pub fn changed_line_count(&self) -> usize {
        self.additions + self.deletions
    }
}

/// A fully parsed multi-file diff that owns the raw bytes all lines point into.
#[derive(Debug)]
pub struct DiffSet {
    pub buffer: Vec<u8>,
    pub files: Vec<FileDiff>,
    index_by_path: HashMap<String, usize>,
    /// Per-file "generated" flag, computed once (layout rebuilds are frequent).
    pub is_generated: Vec<bool>,
}

impl DiffSet {
    pub fn new(buffer: Vec<u8>, files: Vec<FileDiff>) -> Self {
        let mut index = HashMap::with_capacity(files.len() * 2);
        for (i, f) in files.iter().enumerate() {
            index.insert(f.new_path.clone(), i);
            index.entry(f.old_path.clone()).or_insert(i);
        }
        let is_generated = files
            .iter()
            .map(|f| crate::diff::layout::GeneratedFileDetector::is_generated(f.path()))
            .collect();
        DiffSet { buffer, files, index_by_path: index, is_generated }
    }

    pub fn empty() -> Self {
        DiffSet::new(vec![], vec![])
    }

    pub fn file_index(&self, path: &str) -> Option<usize> {
        self.index_by_path.get(path).copied()
    }

    pub fn text_of(&self, line: &DiffLine) -> &str {
        let start = line.text_start as usize;
        let end = start + line.text_length as usize;
        let bytes = &self.buffer[start..end.min(self.buffer.len())];
        match std::str::from_utf8(bytes) {
            Ok(s) => s,
            // The parser only produces ranges on line boundaries; invalid UTF-8 inside
            // a line is shown up to the first bad byte.
            Err(e) => std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
        }
    }

    pub fn text(&self, file: usize, line: usize) -> &str {
        self.text_of(&self.files[file].lines[line])
    }

    pub fn total_lines(&self) -> usize {
        self.files.iter().map(|f| f.lines.len()).sum()
    }
}

// MARK: - Pull requests

/// Identifies a pull request: `owner/repo#number`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct PRRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl fmt::Display for PRRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

impl PRRef {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>, number: u64) -> Self {
        PRRef { owner: owner.into(), repo: repo.into(), number }
    }
    pub fn name_with_owner(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
    pub fn web_url(&self) -> String {
        format!("https://github.com/{}/{}/pull/{}", self.owner, self.repo, self.number)
    }
    /// Stable filesystem-safe key.
    pub fn storage_key(&self) -> String {
        format!("{}__{}__{}", self.owner, self.repo, self.number)
    }

    /// Accepts `https://github.com/o/r/pull/12[/files...]`, `o/r#12`, `github.com/o/r/pull/12`.
    pub fn parse(raw: &str) -> Option<PRRef> {
        let s = raw.trim();
        if let Some(hash) = s.find('#') {
            if !s.contains("/pull/") {
                let repo_part: Vec<&str> = s[..hash].split('/').filter(|p| !p.is_empty()).collect();
                let n: u64 = s[hash + 1..].parse().ok()?;
                if repo_part.len() != 2 {
                    return None;
                }
                return Some(PRRef::new(repo_part[0], repo_part[1], n));
            }
        }
        let mut path = s;
        for prefix in ["https://", "http://"] {
            if let Some(p) = path.strip_prefix(prefix) {
                path = p;
            }
        }
        if let Some(p) = path.strip_prefix("www.") {
            path = p;
        }
        let rest = path.strip_prefix("github.com/")?;
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() < 4 || !(parts[2] == "pull" || parts[2] == "pulls") {
            return None;
        }
        let digits: String = parts[3].chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().ok()?;
        Some(PRRef::new(parts[0], parts[1], n))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CheckState {
    #[serde(rename = "SUCCESS")]
    Success,
    #[serde(rename = "FAILURE")]
    Failure,
    #[serde(rename = "PENDING")]
    Pending,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "EXPECTED")]
    Expected,
    #[serde(rename = "none", other)]
    #[default]
    None,
}

impl CheckState {
    pub fn parse(s: &str) -> CheckState {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ReviewDecision {
    #[serde(rename = "APPROVED")]
    Approved,
    #[serde(rename = "CHANGES_REQUESTED")]
    ChangesRequested,
    #[serde(rename = "REVIEW_REQUIRED")]
    ReviewRequired,
    #[serde(rename = "none", other)]
    #[default]
    None,
}

impl ReviewDecision {
    pub fn parse(s: &str) -> ReviewDecision {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or_default()
    }
}

/// A row in the inbox sidebar.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PRSummary {
    #[serde(rename = "ref")]
    pub pr_ref: PRRef,
    pub title: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    pub updated_at: Date,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    #[serde(default)]
    pub checks: CheckState,
    #[serde(default)]
    pub review_decision: ReviewDecision,
    #[serde(default)]
    pub comment_count: i64,
}

impl PRSummary {
    pub fn id(&self) -> String {
        self.pr_ref.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ViewedState {
    #[serde(rename = "VIEWED")]
    Viewed,
    #[serde(rename = "UNVIEWED")]
    #[default]
    Unviewed,
    #[serde(rename = "DISMISSED")]
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationComment {
    pub id: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    pub body: String,
    pub created_at: Date,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationReview {
    pub id: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    /// APPROVED, CHANGES_REQUESTED, COMMENTED or DISMISSED.
    pub state: String,
    pub body: String,
    pub submitted_at: Date,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCommit {
    pub oid: String,
    pub headline: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    pub date: Date,
    #[serde(default)]
    pub checks: CheckState,
}

/// The PR's conversation (as on GitHub's Conversation and Commits tabs). Inline review
/// threads live in `PRDetail::threads`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PRConversation {
    #[serde(default)]
    pub comments: Vec<ConversationComment>,
    #[serde(default)]
    pub reviews: Vec<ConversationReview>,
    #[serde(default)]
    pub commits: Vec<ConversationCommit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MergeCheck {
    pub name: String,
    /// The app or service that reported it (e.g. "GitHub Actions").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// SUCCESS, FAILURE, ERROR, CANCELLED, TIMED_OUT, ACTION_REQUIRED, NEUTRAL, SKIPPED, STALE or PENDING.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl MergeCheck {
    pub fn id(&self) -> String {
        format!("{}/{}", self.app.clone().unwrap_or_default(), self.name)
    }
    pub fn is_success(&self) -> bool {
        matches!(self.state.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED")
    }
    pub fn is_pending(&self) -> bool {
        self.state == "PENDING"
    }
    pub fn is_failure(&self) -> bool {
        !self.is_success() && !self.is_pending()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeMethod {
    #[serde(rename = "MERGE")]
    Merge,
    #[serde(rename = "SQUASH")]
    Squash,
    #[serde(rename = "REBASE")]
    Rebase,
}

impl MergeMethod {
    pub const ALL: [MergeMethod; 3] = [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase];
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "MERGE",
            MergeMethod::Squash => "SQUASH",
            MergeMethod::Rebase => "REBASE",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Create a merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }
    pub fn button_title(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Merge pull request",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }
}

/// What GitHub's merge box shows: can it merge, what blocks it, and the checks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PRMergeStatus {
    /// MERGEABLE, CONFLICTING or UNKNOWN.
    pub mergeable: String,
    /// CLEAN, BLOCKED, BEHIND, DIRTY, UNSTABLE, DRAFT, HAS_HOOKS or UNKNOWN.
    pub merge_state_status: String,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED, or None when reviews aren't required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    #[serde(default)]
    pub checks: Vec<MergeCheck>,
    /// Merge methods the repository allows; None in older caches (treated as all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_merge_methods: Option<Vec<MergeMethod>>,
    /// The method GitHub preselects for this viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_merge_method: Option<MergeMethod>,
    /// The viewer may merge past branch protection ("bypass rules"). GitHub's own
    /// `viewerCanMergeAsAdmin` misses ruleset/admin bypasses, so repo admins count too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_can_merge_as_admin: Option<bool>,
}

impl PRMergeStatus {
    pub fn new(mergeable: &str, merge_state_status: &str) -> Self {
        PRMergeStatus {
            mergeable: mergeable.into(),
            merge_state_status: merge_state_status.into(),
            review_decision: None,
            checks: vec![],
            allowed_merge_methods: None,
            default_merge_method: None,
            viewer_can_merge_as_admin: None,
        }
    }
    /// Whether GitHub would let the viewer press Merge now.
    pub fn can_merge(&self) -> bool {
        if self.mergeable == "CONFLICTING" {
            return false;
        }
        matches!(self.merge_state_status.as_str(), "CLEAN" | "HAS_HOOKS" | "UNSTABLE" | "BEHIND")
    }
    /// Blocked by requirements, but the viewer can merge anyway by ticking "bypass rules".
    pub fn can_bypass(&self) -> bool {
        self.mergeable != "CONFLICTING"
            && self.merge_state_status == "BLOCKED"
            && self.viewer_can_merge_as_admin == Some(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PRFileMeta {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    pub viewed: ViewedState,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_id: Option<i64>,
    pub body: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    pub created_at: Date,
    #[serde(default)]
    pub is_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub viewer_did_author: bool,
}

impl ReviewComment {
    pub fn new(id: impl Into<String>, body: impl Into<String>, author: impl Into<String>, created_at: Date) -> Self {
        ReviewComment {
            id: id.into(),
            database_id: None,
            body: body.into(),
            author: author.into(),
            author_avatar_url: None,
            created_at,
            is_pending: false,
            url: None,
            viewer_did_author: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    /// Current line on the PR head (None when outdated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_line: Option<i64>,
    #[serde(default)]
    pub side: DiffSide,
    #[serde(default)]
    pub is_resolved: bool,
    #[serde(default)]
    pub is_outdated: bool,
    pub comments: Vec<ReviewComment>,
}

impl ReviewThread {
    pub fn new(id: impl Into<String>, path: impl Into<String>, line: Option<i64>, comments: Vec<ReviewComment>) -> Self {
        ReviewThread {
            id: id.into(),
            path: path.into(),
            line,
            start_line: None,
            original_line: None,
            side: DiffSide::Right,
            is_resolved: false,
            is_outdated: false,
            comments,
        }
    }
    /// The line the thread is drawn under.
    pub fn anchor_line(&self) -> i64 {
        self.line.or(self.original_line).unwrap_or(0)
    }
    pub fn is_pending(&self) -> bool {
        !self.comments.is_empty() && self.comments.iter().all(|c| c.is_pending)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PRDetail {
    #[serde(rename = "ref")]
    pub pr_ref: PRRef,
    pub node_id: String,
    pub title: String,
    pub body: String,
    pub author: String,
    #[serde(rename = "authorAvatarURL", default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_url: Option<String>,
    pub state: String,
    #[serde(default)]
    pub is_draft: bool,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub base_oid: String,
    pub head_oid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_repo_name_with_owner: Option<String>,
    #[serde(rename = "headRepoSSHURL", default, skip_serializing_if = "Option::is_none")]
    pub head_repo_ssh_url: Option<String>,
    #[serde(rename = "headRepoURL", default, skip_serializing_if = "Option::is_none")]
    pub head_repo_url: Option<String>,
    #[serde(default)]
    pub maintainer_can_modify: bool,
    #[serde(default)]
    pub viewer_login: String,
    #[serde(default)]
    pub viewer_can_push: bool,
    #[serde(default)]
    pub files: Vec<PRFileMeta>,
    #[serde(default)]
    pub threads: Vec<ReviewThread>,
    /// The viewer's pending (draft) review, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_review_id: Option<String>,
    #[serde(default)]
    pub changed_files: i64,
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    /// PR comments, submitted reviews and commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<PRConversation>,
    /// Mergeability, review decision and the head commit's checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PRMergeStatus>,
}

impl PRDetail {
    /// A detail with only the required fields set; the rest default.
    pub fn new(pr_ref: PRRef, node_id: &str, title: &str, base_ref: &str, head_ref: &str, base_oid: &str, head_oid: &str) -> Self {
        PRDetail {
            pr_ref,
            node_id: node_id.into(),
            title: title.into(),
            body: String::new(),
            author: String::new(),
            author_avatar_url: None,
            state: "OPEN".into(),
            is_draft: false,
            base_ref_name: base_ref.into(),
            head_ref_name: head_ref.into(),
            base_oid: base_oid.into(),
            head_oid: head_oid.into(),
            head_repo_name_with_owner: None,
            head_repo_ssh_url: None,
            head_repo_url: None,
            maintainer_can_modify: false,
            viewer_login: String::new(),
            viewer_can_push: false,
            files: vec![],
            threads: vec![],
            pending_review_id: None,
            changed_files: 0,
            additions: 0,
            deletions: 0,
            conversation: None,
            status: None,
        }
    }

    pub fn is_from_fork(&self) -> bool {
        match &self.head_repo_name_with_owner {
            None => true,
            Some(head) => head.to_lowercase() != self.pr_ref.name_with_owner().to_lowercase(),
        }
    }

    /// Whether the app may push fixes to the PR head branch.
    pub fn can_push_fixes(&self) -> bool {
        if self.is_from_fork() {
            return self.maintainer_can_modify || self.viewer_login == self.author;
        }
        self.viewer_can_push || self.viewer_login == self.author
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewEvent {
    #[serde(rename = "COMMENT")]
    Comment,
    #[serde(rename = "APPROVE")]
    Approve,
    #[serde(rename = "REQUEST_CHANGES")]
    RequestChanges,
}

impl ReviewEvent {
    pub const ALL: [ReviewEvent; 3] = [ReviewEvent::Comment, ReviewEvent::Approve, ReviewEvent::RequestChanges];
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "Comment",
            ReviewEvent::Approve => "Approve",
            ReviewEvent::RequestChanges => "Request changes",
        }
    }
}

/// A line range on one side of a file's diff.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAnchor {
    pub path: String,
    pub side: DiffSide,
    pub start_line: i64,
    pub end_line: i64,
}

impl<'de> Deserialize<'de> for CodeAnchor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            path: String,
            side: DiffSide,
            start_line: i64,
            end_line: i64,
        }
        let r = Raw::deserialize(d)?;
        Ok(CodeAnchor::new(r.path, r.side, r.start_line, r.end_line))
    }
}

impl CodeAnchor {
    pub fn new(path: impl Into<String>, side: DiffSide, start_line: i64, end_line: i64) -> Self {
        CodeAnchor {
            path: path.into(),
            side,
            start_line: start_line.min(end_line),
            end_line: start_line.max(end_line),
        }
    }
    pub fn is_multiline(&self) -> bool {
        self.start_line != self.end_line
    }
    pub fn label(&self) -> String {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        if self.is_multiline() {
            format!("{} L{}-{}", file, self.start_line, self.end_line)
        } else {
            format!("{} L{}", file, self.start_line)
        }
    }
}

// MARK: - AI

/// FNV-1a over UTF-16 code units (kept identical to the Swift app so persisted hashes match).
pub fn fnv1a_utf16(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for c in s.encode_utf16() {
        h = (h ^ c as u64).wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiscussStatus {
    #[default]
    Active,
    Outdated,
}

/// A private AI conversation anchored to lines of a PR (persisted per PR).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussThread {
    pub id: String,
    #[serde(rename = "ref")]
    pub pr_ref: PRRef,
    pub anchor: CodeAnchor,
    /// Head commit the anchor's line numbers refer to.
    #[serde(rename = "commitSHA")]
    pub commit_sha: String,
    /// Hash of the anchored lines' text, used to detect edits on remap.
    pub anchor_text_hash: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<crate::claude::transcript::DiscussMessage>,
    pub created_at: Date,
    #[serde(default)]
    pub status: DiscussStatus,
    /// Commits produced by "Fix it" from this thread.
    #[serde(default)]
    pub fix_commits: Vec<String>,
}

impl DiscussThread {
    pub fn new(pr_ref: PRRef, anchor: CodeAnchor, commit_sha: impl Into<String>, anchor_text_hash: u64) -> Self {
        DiscussThread {
            id: new_id(),
            pr_ref,
            anchor,
            commit_sha: commit_sha.into(),
            anchor_text_hash,
            session_id: None,
            messages: vec![],
            created_at: Utc::now(),
            status: DiscussStatus::Active,
            fix_commits: vec![],
        }
    }

    pub fn title(&self) -> String {
        use crate::claude::transcript::Role;
        self.messages
            .iter()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.text.split('\n').find(|l| !l.is_empty()).map(str::to_string))
            .unwrap_or_else(|| self.anchor.label())
    }

    pub fn hash_lines(lines: &[impl AsRef<str>]) -> u64 {
        let joined: Vec<&str> = lines.iter().map(|l| l.as_ref()).collect();
        fnv1a_utf16(&joined.join("\n"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FixStatus {
    Queued,
    Running,
    Committed,
    Failed,
}

/// A queued request for the Discuss agent to change code.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixRequest {
    pub id: String,
    pub thread_id: String,
    pub anchor: CodeAnchor,
    pub instructions: String,
    pub status: FixStatus,
    #[serde(default)]
    pub commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: Date,
}

impl FixRequest {
    pub fn new(thread_id: impl Into<String>, anchor: CodeAnchor, instructions: impl Into<String>) -> Self {
        FixRequest {
            id: new_id(),
            thread_id: thread_id.into(),
            anchor,
            instructions: instructions.into(),
            status: FixStatus::Queued,
            commits: vec![],
            error: None,
            created_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReadingItem {
    pub path: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Risk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AISummary {
    pub summary: String,
    #[serde(default)]
    pub reading_order: Vec<ReadingItem>,
    #[serde(default)]
    pub risks: Vec<Risk>,
    #[serde(rename = "headSHA", default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Date>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
    Nit,
}

impl Severity {
    pub const ALL: [Severity; 4] = [Severity::High, Severity::Medium, Severity::Low, Severity::Nit];
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Nit => "nit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FindingState {
    #[default]
    Open,
    Accepted,
    Dismissed,
    Discussed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AIFinding {
    pub id: String,
    pub path: String,
    pub side: DiffSide,
    pub start_line: i64,
    pub end_line: i64,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub state: FindingState,
}

/// Decodes both persisted findings and raw model output (no id/state).
impl<'de> Deserialize<'de> for AIFinding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(d)?;
        let o = v.as_object().ok_or_else(|| D::Error::custom("finding is not an object"))?;
        let get_str = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let get_int = |k: &str| o.get(k).and_then(|v| v.as_i64());
        let path = get_str("path").ok_or_else(|| D::Error::missing_field("path"))?;
        let title = get_str("title").ok_or_else(|| D::Error::missing_field("title"))?;
        let s = get_int("startLine").ok_or_else(|| D::Error::missing_field("startLine"))?;
        let e = get_int("endLine").unwrap_or(s);
        let side = get_str("side").and_then(|s| DiffSide::parse(&s)).unwrap_or(DiffSide::Right);
        let severity = o
            .get("severity")
            .and_then(|v| serde_json::from_value::<Severity>(v.clone()).ok())
            .unwrap_or(Severity::Low);
        let state = o
            .get("state")
            .and_then(|v| serde_json::from_value::<FindingState>(v.clone()).ok())
            .unwrap_or_default();
        Ok(AIFinding {
            id: get_str("id").unwrap_or_else(new_id),
            path,
            side,
            start_line: s.min(e),
            end_line: s.max(e),
            severity,
            title,
            body: get_str("body").unwrap_or_default(),
            suggestion: get_str("suggestion").filter(|s| !s.is_empty()),
            state,
        })
    }
}

impl AIFinding {
    pub fn new(path: &str, side: DiffSide, start: i64, end: i64, severity: Severity, title: &str, body: &str) -> Self {
        AIFinding {
            id: new_id(),
            path: path.into(),
            side,
            start_line: start.min(end),
            end_line: start.max(end),
            severity,
            title: title.into(),
            body: body.into(),
            suggestion: None,
            state: FindingState::Open,
        }
    }
    pub fn anchor(&self) -> CodeAnchor {
        CodeAnchor::new(self.path.clone(), self.side, self.start_line, self.end_line)
    }
    /// The review comment body produced by "Accept".
    pub fn comment_body(&self) -> String {
        let mut s = format!("**{}**\n\n{}", self.title, self.body);
        if let Some(sug) = &self.suggestion {
            s.push_str(&format!("\n\n```suggestion\n{}\n```", sug));
        }
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreReviewResult {
    pub findings: Vec<AIFinding>,
    #[serde(rename = "headSHA", default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Date>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_ref_parsing() {
        assert_eq!(PRRef::parse("https://github.com/o/r/pull/12/files"), Some(PRRef::new("o", "r", 12)));
        assert_eq!(PRRef::parse("o/r#7"), Some(PRRef::new("o", "r", 7)));
        assert_eq!(PRRef::parse("github.com/o/r/pull/3"), Some(PRRef::new("o", "r", 3)));
        assert_eq!(PRRef::parse("https://www.github.com/o/r/pulls/5"), Some(PRRef::new("o", "r", 5)));
        assert_eq!(PRRef::parse("https://gitlab.com/o/r/pull/3"), None);
        assert_eq!(PRRef::parse("nonsense"), None);
    }

    #[test]
    fn finding_decodes_raw_model_output() {
        let f: AIFinding = serde_json::from_str(
            r#"{"path":"a.rs","startLine":9,"endLine":4,"severity":"bogus","title":"T","suggestion":""}"#,
        )
        .unwrap();
        assert_eq!((f.start_line, f.end_line), (4, 9));
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.side, DiffSide::Right);
        assert_eq!(f.suggestion, None);
        assert_eq!(f.state, FindingState::Open);
    }

    #[test]
    fn check_state_unknown_is_none() {
        assert_eq!(CheckState::parse("SUCCESS"), CheckState::Success);
        assert_eq!(CheckState::parse("WHATEVER"), CheckState::None);
    }
}
