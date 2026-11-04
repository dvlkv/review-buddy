//! GitHub GraphQL documents and their response DTOs.
//!
//! The query strings are the external contract with GitHub and are byte-for-byte the
//! strings the Swift app sends (Swift's `"""` literals dedent their lines and splice
//! `commentFields` / `threadFields` in as-is, which is what the literals below spell out).

use serde::Deserialize;

use crate::models::{
    CheckState, ConversationComment, ConversationCommit, ConversationReview, Date, DiffSide, MergeCheck, MergeMethod,
    PRConversation, PRMergeStatus, PRRef, PRSummary, ReviewComment, ReviewDecision, ReviewThread, ViewedState,
};

macro_rules! comment_fields {
    () => {
        "id databaseId body createdAt state url viewerDidAuthor author { login avatarUrl }"
    };
}

macro_rules! thread_fields {
    () => {
        concat!(
            "id path line startLine originalLine diffSide isResolved isOutdated\n",
            "comments(first: 100) { nodes { ",
            comment_fields!(),
            " } }"
        )
    };
}

pub const COMMENT_FIELDS: &str = comment_fields!();
pub const THREAD_FIELDS: &str = thread_fields!();

pub const SEARCH: &str = "query($q: String!) {
  search(query: $q, type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number title isDraft updatedAt additions deletions reviewDecision
        repository { name owner { login } }
        author { login avatarUrl }
        comments { totalCount }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}";

pub const PULL_REQUEST: &str = concat!(
    "query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    viewerPermission mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod
    pullRequest(number: $number) {
      id title body state isDraft baseRefName headRefName baseRefOid headRefOid maintainerCanModify
      changedFiles additions deletions
      author { login avatarUrl }
      headRepository { nameWithOwner sshUrl url }
      files(first: 100) { pageInfo { hasNextPage endCursor } nodes { path additions deletions viewerViewedState } }
      reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { ",
    thread_fields!(),
    " } }
      reviews(states: [PENDING], first: 5) { nodes { id viewerDidAuthor } }
      comments(last: 100) { nodes { id body createdAt url author { login avatarUrl } } }
      allReviews: reviews(last: 100) { nodes { id state body submittedAt url author { login avatarUrl } } }
      commits(first: 250) { nodes { commit { oid messageHeadline committedDate author { name user { login avatarUrl } } statusCheckRollup { state } } } }
      mergeable mergeStateStatus reviewDecision viewerCanMergeAsAdmin
      headCommit: commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { name conclusion status detailsUrl checkSuite { app { name } } }
        ... on StatusContext { context state targetUrl }
      } } } } } }
    }
  }
}"
);

pub const FILES_PAGE: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    files(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { path additions deletions viewerViewedState } }
  } }
}";

pub const THREADS_PAGE: &str = concat!(
    "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { ",
    thread_fields!(),
    " } }
  } }
}"
);

pub const ADD_REVIEW: &str =
    "mutation($pr: ID!) { addPullRequestReview(input: { pullRequestId: $pr }) { pullRequestReview { id } } }";

pub const ADD_THREAD: &str = concat!(
    "mutation($pr: ID!, $review: ID!, $path: String!, $body: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide) {
  addPullRequestReviewThread(input: {
    pullRequestId: $pr, pullRequestReviewId: $review, path: $path, body: $body,
    line: $line, side: $side, startLine: $startLine, startSide: $startSide, subjectType: LINE
  }) { thread { ",
    thread_fields!(),
    " } }
}"
);

pub const REPLY: &str = concat!(
    "mutation($thread: ID!, $review: ID, $body: String!) {
  addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, pullRequestReviewId: $review, body: $body }) {
    comment { ",
    comment_fields!(),
    " }
  }
}"
);

pub const MERGE_PULL_REQUEST: &str = "mutation($id: ID!, $method: PullRequestMergeMethod!, $head: GitObjectID!) {
  mergePullRequest(input: { pullRequestId: $id, mergeMethod: $method, expectedHeadOid: $head }) {
    pullRequest { state }
  }
}";

/// Cheap poll for "was the PR pushed to since we loaded it".
pub const HEAD_OID: &str = "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) { headRefOid } }
}";

pub const CLOSE_PULL_REQUEST: &str =
    "mutation($id: ID!) { closePullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }";

pub const REOPEN_PULL_REQUEST: &str =
    "mutation($id: ID!) { reopenPullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }";

pub const ADD_COMMENT: &str = "mutation($subject: ID!, $body: String!) {
  addComment(input: { subjectId: $subject, body: $body }) {
    commentEdge { node { id body createdAt url author { login avatarUrl } } }
  }
}";

pub const UPDATE_COMMENT: &str = concat!(
    "mutation($id: ID!, $body: String!) {
  updatePullRequestReviewComment(input: { pullRequestReviewCommentId: $id, body: $body }) {
    pullRequestReviewComment { ",
    comment_fields!(),
    " }
  }
}"
);

pub const DELETE_COMMENT: &str =
    "mutation($id: ID!) { deletePullRequestReviewComment(input: { id: $id }) { clientMutationId } }";

pub const SUBMIT_REVIEW: &str = "mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) {
  submitPullRequestReview(input: { pullRequestReviewId: $review, event: $event, body: $body }) { clientMutationId }
}";

pub const DELETE_REVIEW: &str =
    "mutation($review: ID!) { deletePullRequestReview(input: { pullRequestReviewId: $review }) { clientMutationId } }";

pub const RESOLVE: &str = "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id isResolved } } }";

pub const UNRESOLVE: &str =
    "mutation($id: ID!) { unresolveReviewThread(input: { threadId: $id }) { thread { id isResolved } } }";

pub const MARK_VIEWED: &str =
    "mutation($pr: ID!, $path: String!) { markFileAsViewed(input: { pullRequestId: $pr, path: $path }) { clientMutationId } }";

pub const UNMARK_VIEWED: &str =
    "mutation($pr: ID!, $path: String!) { unmarkFileAsViewed(input: { pullRequestId: $pr, path: $path }) { clientMutationId } }";

// MARK: - Response DTOs
//
// Field optionality mirrors the Swift `Decodable` structs: a required field that's missing
// fails the decode, an optional one becomes None. Fields that are decoded but never read
// are kept so the decode stays as strict as Swift's.

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct GraphQLErrorDTO {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageInfoDTO {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorDTO {
    pub login: String,
    pub avatar_url: Option<String>,
}

fn login(a: &Option<ActorDTO>) -> String {
    a.as_ref().map(|a| a.login.clone()).unwrap_or_else(|| "ghost".into())
}

fn avatar(a: &Option<ActorDTO>) -> Option<String> {
    a.as_ref().and_then(|a| a.avatar_url.clone())
}

#[derive(Deserialize, Debug)]
pub(crate) struct Nodes<T> {
    pub nodes: Vec<T>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Connection<T> {
    pub page_info: PageInfoDTO,
    pub nodes: Vec<T>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct StateDTO {
    pub state: String,
}

// Search

#[derive(Deserialize, Debug)]
pub(crate) struct SearchData {
    pub search: Nodes<SearchNode>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct SearchRepo {
    pub name: String,
    pub owner: ActorDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CountDTO {
    pub total_count: i64,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RollupCommitDTO {
    pub status_check_rollup: Option<StateDTO>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct RollupCommitNode {
    pub commit: RollupCommitDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchNode {
    pub number: Option<u64>,
    pub title: Option<String>,
    pub is_draft: Option<bool>,
    pub updated_at: Option<Date>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub review_decision: Option<String>,
    pub repository: Option<SearchRepo>,
    pub author: Option<ActorDTO>,
    pub comments: Option<CountDTO>,
    pub commits: Option<Nodes<RollupCommitNode>>,
}

/// Swift's `Date.distantPast` (0001-01-01T00:00:00Z).
pub(crate) fn distant_past() -> Date {
    chrono::DateTime::from_timestamp(-62_135_596_800, 0).expect("valid timestamp")
}

pub(crate) fn viewed_state(raw: &str) -> ViewedState {
    match raw {
        "VIEWED" => ViewedState::Viewed,
        "DISMISSED" => ViewedState::Dismissed,
        _ => ViewedState::Unviewed,
    }
}

pub(crate) fn merge_method(raw: &str) -> Option<MergeMethod> {
    MergeMethod::ALL.into_iter().find(|m| m.as_str() == raw)
}

impl SearchNode {
    pub fn to_summary(&self) -> Option<PRSummary> {
        let (number, title, repo) = (self.number?, self.title.clone()?, self.repository.as_ref()?);
        let rollup = self
            .commits
            .as_ref()
            .and_then(|c| c.nodes.first())
            .and_then(|n| n.commit.status_check_rollup.as_ref())
            .map(|r| r.state.as_str())
            .unwrap_or("");
        Some(PRSummary {
            pr_ref: PRRef::new(repo.owner.login.clone(), repo.name.clone(), number),
            title,
            author: login(&self.author),
            author_avatar_url: avatar(&self.author),
            updated_at: self.updated_at.unwrap_or_else(distant_past),
            is_draft: self.is_draft.unwrap_or(false),
            additions: self.additions.unwrap_or(0),
            deletions: self.deletions.unwrap_or(0),
            checks: CheckState::parse(rollup),
            review_decision: ReviewDecision::parse(self.review_decision.as_deref().unwrap_or("")),
            comment_count: self.comments.as_ref().map(|c| c.total_count).unwrap_or(0),
        })
    }
}

// Review comments and threads

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentDTO {
    pub id: String,
    pub database_id: Option<i64>,
    pub body: String,
    pub created_at: Date,
    pub state: Option<String>,
    pub url: Option<String>,
    pub viewer_did_author: Option<bool>,
    pub author: Option<ActorDTO>,
}

impl CommentDTO {
    pub fn to_comment(&self) -> ReviewComment {
        ReviewComment {
            id: self.id.clone(),
            database_id: self.database_id,
            body: self.body.clone(),
            author: login(&self.author),
            author_avatar_url: avatar(&self.author),
            created_at: self.created_at,
            is_pending: self.state.as_deref() == Some("PENDING"),
            url: self.url.clone(),
            viewer_did_author: self.viewer_did_author.unwrap_or(false),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDTO {
    pub id: String,
    pub path: String,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_line: Option<i64>,
    pub diff_side: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub comments: Nodes<CommentDTO>,
}

impl ThreadDTO {
    pub fn to_thread(&self) -> ReviewThread {
        ReviewThread {
            id: self.id.clone(),
            path: self.path.clone(),
            line: self.line,
            start_line: self.start_line,
            original_line: self.original_line,
            side: DiffSide::parse(&self.diff_side).unwrap_or(DiffSide::Right),
            is_resolved: self.is_resolved,
            is_outdated: self.is_outdated,
            comments: self.comments.nodes.iter().map(CommentDTO::to_comment).collect(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileDTO {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    pub viewer_viewed_state: String,
}

// Conversation

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueCommentDTO {
    pub id: String,
    pub body: String,
    pub created_at: Date,
    pub url: Option<String>,
    pub author: Option<ActorDTO>,
}

impl IssueCommentDTO {
    pub fn to_comment(&self) -> ConversationComment {
        ConversationComment {
            id: self.id.clone(),
            author: login(&self.author),
            author_avatar_url: avatar(&self.author),
            body: self.body.clone(),
            created_at: self.created_at,
            url: self.url.clone(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewDTO {
    pub id: String,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<Date>,
    pub url: Option<String>,
    pub author: Option<ActorDTO>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CommitAuthorDTO {
    pub name: Option<String>,
    pub user: Option<ActorDTO>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitDTO {
    pub oid: String,
    pub message_headline: String,
    pub committed_date: Date,
    pub author: Option<CommitAuthorDTO>,
    pub status_check_rollup: Option<StateDTO>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CommitNodeDTO {
    pub commit: CommitDTO,
}

// Merge status

#[derive(Deserialize, Debug)]
pub(crate) struct ContextsRollupDTO {
    pub contexts: Nodes<CheckContextDTO>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeadCommitInner {
    pub status_check_rollup: Option<ContextsRollupDTO>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct HeadCommitDTO {
    pub commit: HeadCommitInner,
}

#[derive(Deserialize, Debug)]
pub(crate) struct NamedDTO {
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub(crate) struct SuiteDTO {
    pub app: Option<NamedDTO>,
}

/// A CheckRun (Actions, apps) or a legacy StatusContext.
#[allow(dead_code)] // `typename` and `status` are decoded (as in Swift) but not read.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckContextDTO {
    #[serde(rename = "__typename")]
    pub typename: String,
    // CheckRun
    pub name: Option<String>,
    pub conclusion: Option<String>,
    pub status: Option<String>,
    pub details_url: Option<String>,
    pub check_suite: Option<SuiteDTO>,
    // StatusContext
    pub context: Option<String>,
    pub state: Option<String>,
    pub target_url: Option<String>,
}

impl CheckContextDTO {
    pub fn check(&self) -> Option<MergeCheck> {
        if let Some(name) = &self.name {
            // A run without a conclusion is still queued or in progress.
            return Some(MergeCheck {
                name: name.clone(),
                app: self.check_suite.as_ref().and_then(|s| s.app.as_ref()).map(|a| a.name.clone()),
                state: self.conclusion.clone().unwrap_or_else(|| "PENDING".into()),
                url: self.details_url.clone(),
            });
        }
        if let (Some(context), Some(state)) = (&self.context, &self.state) {
            return Some(MergeCheck {
                name: context.clone(),
                app: None,
                state: if state == "EXPECTED" { "PENDING".into() } else { state.clone() },
                url: self.target_url.clone(),
            });
        }
        None
    }
}

// Pull request

#[derive(Deserialize, Debug)]
pub(crate) struct ViewerDTO {
    pub login: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PRRepoDTO {
    pub viewer_permission: Option<String>,
    pub merge_commit_allowed: Option<bool>,
    pub squash_merge_allowed: Option<bool>,
    pub rebase_merge_allowed: Option<bool>,
    pub viewer_default_merge_method: Option<String>,
    pub pull_request: Option<PullRequestDTO>,
}

impl PRRepoDTO {
    pub fn allowed_merge_methods(&self) -> Option<Vec<MergeMethod>> {
        if self.merge_commit_allowed.is_none() && self.squash_merge_allowed.is_none() && self.rebase_merge_allowed.is_none()
        {
            return None;
        }
        Some(
            [
                (MergeMethod::Merge, self.merge_commit_allowed),
                (MergeMethod::Squash, self.squash_merge_allowed),
                (MergeMethod::Rebase, self.rebase_merge_allowed),
            ]
            .into_iter()
            .filter(|(_, allowed)| *allowed == Some(true))
            .map(|(m, _)| m)
            .collect(),
        )
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeadRepoDTO {
    pub name_with_owner: String,
    pub ssh_url: Option<String>,
    pub url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingReviewDTO {
    pub id: String,
    pub viewer_did_author: bool,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestDTO {
    pub id: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub is_draft: bool,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub base_ref_oid: String,
    pub head_ref_oid: String,
    pub maintainer_can_modify: bool,
    pub changed_files: i64,
    pub additions: i64,
    pub deletions: i64,
    pub author: Option<ActorDTO>,
    pub head_repository: Option<HeadRepoDTO>,
    pub files: Option<Connection<FileDTO>>,
    pub review_threads: Connection<ThreadDTO>,
    pub reviews: Nodes<PendingReviewDTO>,
    // Optional: hand-written fixtures and older responses omit them.
    pub comments: Option<Nodes<IssueCommentDTO>>,
    pub all_reviews: Option<Nodes<ReviewDTO>>,
    pub commits: Option<Nodes<CommitNodeDTO>>,
    pub mergeable: Option<String>,
    pub merge_state_status: Option<String>,
    pub review_decision: Option<String>,
    pub viewer_can_merge_as_admin: Option<bool>,
    pub head_commit: Option<Nodes<HeadCommitDTO>>,
}

impl PullRequestDTO {
    pub fn status(&self, repo: &PRRepoDTO) -> Option<PRMergeStatus> {
        let (mergeable, merge_state_status) = (self.mergeable.as_ref()?, self.merge_state_status.as_ref()?);
        let checks = self
            .head_commit
            .as_ref()
            .and_then(|h| h.nodes.first())
            .and_then(|n| n.commit.status_check_rollup.as_ref())
            .map(|r| r.contexts.nodes.iter().filter_map(CheckContextDTO::check).collect())
            .unwrap_or_default();
        Some(PRMergeStatus {
            mergeable: mergeable.clone(),
            merge_state_status: merge_state_status.clone(),
            review_decision: self.review_decision.clone(),
            checks,
            allowed_merge_methods: repo.allowed_merge_methods(),
            default_merge_method: repo.viewer_default_merge_method.as_deref().and_then(merge_method),
            viewer_can_merge_as_admin: self
                .viewer_can_merge_as_admin
                .map(|b| b || repo.viewer_permission.as_deref() == Some("ADMIN")),
        })
    }

    /// Built from the newest 100 comments and reviews (`last: 100`): every single comment
    /// is a review too, so busy PRs pass 100 quickly.
    pub fn conversation(&self) -> Option<PRConversation> {
        if self.comments.is_none() && self.all_reviews.is_none() && self.commits.is_none() {
            return None;
        }
        Some(PRConversation {
            comments: self
                .comments
                .as_ref()
                .map(|c| c.nodes.iter().map(IssueCommentDTO::to_comment).collect())
                .unwrap_or_default(),
            // PENDING is the viewer's own unsubmitted draft (see `pending_review_id`).
            reviews: self
                .all_reviews
                .as_ref()
                .map(|r| {
                    r.nodes
                        .iter()
                        .filter(|r| r.state != "PENDING")
                        .filter_map(|r| {
                            Some(ConversationReview {
                                id: r.id.clone(),
                                author: login(&r.author),
                                author_avatar_url: avatar(&r.author),
                                state: r.state.clone(),
                                body: r.body.clone(),
                                submitted_at: r.submitted_at?,
                                url: r.url.clone(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            commits: self
                .commits
                .as_ref()
                .map(|c| {
                    c.nodes
                        .iter()
                        .map(|n| {
                            let c = &n.commit;
                            let user = c.author.as_ref().and_then(|a| a.user.as_ref());
                            ConversationCommit {
                                oid: c.oid.clone(),
                                headline: c.message_headline.clone(),
                                author: user
                                    .map(|u| u.login.clone())
                                    .or_else(|| c.author.as_ref().and_then(|a| a.name.clone()))
                                    .unwrap_or_else(|| "unknown".into()),
                                author_avatar_url: user.and_then(|u| u.avatar_url.clone()),
                                date: c.committed_date,
                                checks: CheckState::parse(
                                    c.status_check_rollup.as_ref().map(|r| r.state.as_str()).unwrap_or(""),
                                ),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct PRData {
    pub viewer: ViewerDTO,
    pub repository: PRRepoDTO,
}

#[derive(Deserialize, Debug)]
pub(crate) struct FilesPR {
    pub files: Option<Connection<FileDTO>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilesRepo {
    pub pull_request: FilesPR,
}

#[derive(Deserialize, Debug)]
pub(crate) struct PRFilesData {
    pub repository: FilesRepo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadsPR {
    pub review_threads: Connection<ThreadDTO>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadsRepo {
    pub pull_request: ThreadsPR,
}

#[derive(Deserialize, Debug)]
pub(crate) struct PRThreadsData {
    pub repository: ThreadsRepo,
}

// Head oid

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeadOidPR {
    pub head_ref_oid: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeadOidRepo {
    pub pull_request: Option<HeadOidPR>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct HeadOidData {
    pub repository: HeadOidRepo,
}

// Mutations

#[derive(Deserialize, Debug)]
pub(crate) struct CommentEdgeDTO {
    pub node: IssueCommentDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddCommentPayload {
    pub comment_edge: CommentEdgeDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddCommentData {
    pub add_comment: AddCommentPayload,
}

#[derive(Deserialize, Debug)]
pub(crate) struct IdDTO {
    pub id: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddReviewPayload {
    pub pull_request_review: IdDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddReviewData {
    pub add_pull_request_review: AddReviewPayload,
}

#[derive(Deserialize, Debug)]
pub(crate) struct AddThreadPayload {
    pub thread: Option<ThreadDTO>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddThreadData {
    pub add_pull_request_review_thread: AddThreadPayload,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ReplyPayload {
    pub comment: CommentDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplyData {
    pub add_pull_request_review_thread_reply: ReplyPayload,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateCommentPayload {
    pub pull_request_review_comment: CommentDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateCommentData {
    pub update_pull_request_review_comment: UpdateCommentPayload,
}

#[allow(dead_code)] // `id` is decoded (as in Swift) but not read.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedThreadDTO {
    pub id: String,
    pub is_resolved: bool,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ResolvePayload {
    pub thread: ResolvedThreadDTO,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveData {
    pub resolve_review_thread: Option<ResolvePayload>,
    pub unresolve_review_thread: Option<ResolvePayload>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_are_spliced_like_swift() {
        assert_eq!(
            THREAD_FIELDS,
            "id path line startLine originalLine diffSide isResolved isOutdated\ncomments(first: 100) { nodes { id databaseId body createdAt state url viewerDidAuthor author { login avatarUrl } } }"
        );
        assert!(PULL_REQUEST.starts_with("query($owner: String!, $name: String!, $number: Int!) {\n  viewer { login }\n"));
        assert!(PULL_REQUEST.contains(
            "      reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { id path line startLine originalLine diffSide isResolved isOutdated\ncomments(first: 100) { nodes { id databaseId"
        ));
        assert!(PULL_REQUEST.ends_with("    }\n  }\n}"));
        assert!(REPLY.contains("    comment { id databaseId body createdAt state url viewerDidAuthor author { login avatarUrl } }\n"));
        for q in [ADD_REVIEW, ADD_THREAD, REPLY, MERGE_PULL_REQUEST, RESOLVE, UNRESOLVE, MARK_VIEWED, UNMARK_VIEWED] {
            assert!(q.starts_with("mutation"));
        }
    }
}
