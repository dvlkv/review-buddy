//! Blocking GitHub GraphQL/REST client. Every call does network I/O: run it on a worker
//! thread, never on the UI thread. `GitHubClient` is `Send + Sync`; share it via `Arc`.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::queries::{self as q, GraphQLErrorDTO};
use crate::git::process::{CommandRunning, ProcessRunner, ToolLocator};
use crate::models::{
    CodeAnchor, ConversationComment, MergeMethod, PRDetail, PRFileMeta, PRRef, PRSummary, ReviewComment, ReviewEvent,
    ReviewThread,
};
use crate::support::keychain::Keychain;

// MARK: - Transport

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `POST`, `PUT`, ...
    pub method: String,
    /// Absolute URL, already percent-encoded.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl HttpRequest {
    /// The first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends one HTTP request. Any HTTP status is an `Ok` response; `Err` means the request
/// never completed (DNS, TLS, connection, timeout).
pub trait HttpTransport: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, String>;
}

/// The real transport, over `ureq`. It keeps no HTTP cache, so a diff right after a push is fresh.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        UreqTransport { agent: ureq::AgentBuilder::new().build() }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for UreqTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let mut r = self.agent.request(&request.method, &request.url).timeout(request.timeout);
        for (k, v) in &request.headers {
            r = r.set(k, v);
        }
        let result = match &request.body {
            Some(b) => r.send_bytes(b),
            None => r.call(),
        };
        let resp = match result {
            Ok(resp) => resp,
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(ureq::Error::Transport(t)) => return Err(t.to_string()),
        };
        let status = resp.status();
        let mut body = Vec::new();
        resp.into_reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
        Ok(HttpResponse { status, body })
    }
}

// MARK: - Tokens

pub trait TokenProvider: Send + Sync {
    fn token(&self) -> Result<String, GitHubError>;
    /// Forget a cached token (after GitHub answered 401).
    fn invalidate(&self) {}
}

pub struct StaticTokenProvider(pub String);

impl StaticTokenProvider {
    pub fn new(value: impl Into<String>) -> Self {
        StaticTokenProvider(value.into())
    }
}

impl TokenProvider for StaticTokenProvider {
    fn token(&self) -> Result<String, GitHubError> {
        Ok(self.0.clone())
    }
}

type Lookup = Box<dyn Fn() -> Option<String> + Send + Sync>;

/// Reads the token from `gh auth token`, falling back to a Keychain PAT (account `github-pat`).
pub struct GHTokenProvider {
    runner: Arc<dyn CommandRunning>,
    find_gh: Lookup,
    read_pat: Lookup,
    cached: Mutex<Option<String>>,
}

impl GHTokenProvider {
    pub fn new(runner: Arc<dyn CommandRunning>) -> Self {
        Self::with_sources(runner, Box::new(|| ToolLocator::find("gh")), Box::new(|| Keychain::read("github-pat")))
    }

    /// `find_gh` locates the `gh` binary; `read_pat` reads the stored PAT. For tests.
    pub fn with_sources(runner: Arc<dyn CommandRunning>, find_gh: Lookup, read_pat: Lookup) -> Self {
        GHTokenProvider { runner, find_gh, read_pat, cached: Mutex::new(None) }
    }
}

impl Default for GHTokenProvider {
    fn default() -> Self {
        Self::new(Arc::new(ProcessRunner))
    }
}

impl TokenProvider for GHTokenProvider {
    fn token(&self) -> Result<String, GitHubError> {
        if let Some(c) = self.cached.lock().unwrap().clone() {
            return Ok(c);
        }
        if let Some(gh) = (self.find_gh)() {
            if let Ok(r) = self.runner.run(&gh, &["auth".to_string(), "token".to_string()], None, None, None) {
                if r.status == 0 {
                    let t = r.stdout_string().trim().to_string();
                    if !t.is_empty() {
                        *self.cached.lock().unwrap() = Some(t.clone());
                        return Ok(t);
                    }
                }
            }
        }
        if let Some(pat) = (self.read_pat)().filter(|p| !p.is_empty()) {
            *self.cached.lock().unwrap() = Some(pat.clone());
            return Ok(pat);
        }
        Err(GitHubError::NotAuthenticated)
    }

    fn invalidate(&self) {
        *self.cached.lock().unwrap() = None;
    }
}

// MARK: - Errors

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHubError {
    NotAuthenticated,
    InvalidResponse,
    Http(u16, String),
    GraphQL(Vec<String>),
    DiffTooLarge,
    NotFound,
    /// The request never completed (network, TLS, timeout).
    Transport(String),
    /// GitHub answered, but not in the shape we expected.
    Decoding(String),
}

impl std::fmt::Display for GitHubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHubError::NotAuthenticated => {
                write!(f, "Not signed in to GitHub. Run `gh auth login` or add a token in Settings.")
            }
            GitHubError::InvalidResponse => write!(f, "GitHub returned an invalid response."),
            GitHubError::Http(code, msg) => write!(f, "GitHub error {code}: {msg}"),
            GitHubError::GraphQL(errs) => write!(f, "{}", errs.join("\n")),
            GitHubError::DiffTooLarge => {
                write!(f, "This diff is too large for the GitHub API; using the local checkout instead.")
            }
            GitHubError::NotFound => write!(f, "Not found."),
            GitHubError::Transport(msg) => write!(f, "Couldn't reach GitHub: {msg}"),
            GitHubError::Decoding(msg) => write!(f, "Couldn't read GitHub's response: {msg}"),
        }
    }
}

impl std::error::Error for GitHubError {}

impl From<GitHubError> for String {
    fn from(e: GitHubError) -> String {
        e.to_string()
    }
}

// MARK: - Inbox

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxSection {
    ReviewRequested,
    Mine,
}

impl InboxSection {
    pub const ALL: [InboxSection; 2] = [InboxSection::ReviewRequested, InboxSection::Mine];

    pub fn query(self) -> &'static str {
        match self {
            InboxSection::ReviewRequested => "is:pr is:open review-requested:@me archived:false sort:updated-desc",
            InboxSection::Mine => "is:pr is:open author:@me archived:false sort:updated-desc",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            InboxSection::ReviewRequested => "Review requested",
            InboxSection::Mine => "My pull requests",
        }
    }
}

// MARK: - API

/// How a merge request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeResult {
    Merged,
    /// Added to a merge queue; it merges later.
    Enqueued,
    /// Accepted and still running in the background when we stopped waiting.
    Pending,
}

pub type GitHubResult<T> = Result<T, GitHubError>;

/// The GitHub surface the app needs. A trait so stores can be tested with a fake.
/// All methods block; call them from worker threads.
pub trait GitHubAPI: Send + Sync {
    fn search(&self, query: &str) -> GitHubResult<Vec<PRSummary>>;
    fn pull_request(&self, pr: &PRRef) -> GitHubResult<PRDetail>;
    /// The PR's unified diff (`application/vnd.github.diff`); `DiffTooLarge` when GitHub refuses.
    fn diff(&self, pr: &PRRef) -> GitHubResult<Vec<u8>>;
    /// The PR head commit on GitHub right now.
    fn head_oid(&self, pr: &PRRef) -> GitHubResult<String>;
    /// Raw file contents at `git_ref` (for expanding diff context).
    fn file_contents(&self, owner: &str, repo: &str, path: &str, git_ref: &str) -> GitHubResult<Vec<u8>>;
    /// Starts a pending review; returns its node id.
    fn start_review(&self, pull_request_id: &str) -> GitHubResult<String>;
    fn add_thread(&self, pull_request_id: &str, review_id: &str, anchor: &CodeAnchor, body: &str)
        -> GitHubResult<ReviewThread>;
    fn reply(&self, thread_id: &str, review_id: Option<&str>, body: &str) -> GitHubResult<ReviewComment>;
    /// A line comment published right away, outside any review (GitHub's "Add single comment").
    fn add_single_comment(&self, pr: &PRRef, commit_id: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<()>;
    /// A thread reply published right away; `comment_database_id` is the thread's first comment.
    fn reply_single(&self, pr: &PRRef, thread_id: &str, comment_database_id: i64, body: &str) -> GitHubResult<()>;
    /// A PR (issue) comment, outside any review.
    fn add_comment(&self, subject_id: &str, body: &str) -> GitHubResult<ConversationComment>;
    /// Merges the PR; fails if the head moved past `expected_head_oid` since it was loaded.
    /// `pr` and `bypass_rules` are used by the asynchronous REST merge, which GitHub
    /// requires for stacked PRs.
    fn merge_pull_request(
        &self,
        pr: &PRRef,
        id: &str,
        method: MergeMethod,
        expected_head_oid: &str,
        bypass_rules: bool,
    ) -> GitHubResult<MergeResult>;
    /// Closes (`closed: true`) or reopens the PR without merging.
    fn set_pull_request_closed(&self, id: &str, closed: bool) -> GitHubResult<()>;
    fn update_comment(&self, id: &str, body: &str) -> GitHubResult<ReviewComment>;
    fn delete_comment(&self, id: &str) -> GitHubResult<()>;
    fn submit_review(&self, review_id: &str, event: ReviewEvent, body: &str) -> GitHubResult<()>;
    fn delete_review(&self, review_id: &str) -> GitHubResult<()>;
    /// Resolves or unresolves a thread; returns the new resolved state.
    fn set_resolved(&self, thread_id: &str, resolved: bool) -> GitHubResult<bool>;
    fn set_viewed(&self, pull_request_id: &str, path: &str, viewed: bool) -> GitHubResult<()>;
}

pub struct GitHubClient {
    transport: Arc<dyn HttpTransport>,
    tokens: Arc<dyn TokenProvider>,
    api_base: String,
    /// Between polls of an asynchronous merge (tests set it to zero).
    pub async_merge_poll_interval: Duration,
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Percent-encodes everything outside RFC 3986's unreserved set, keeping the bytes in `keep`.
fn percent_encode(s: &str, keep: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A path segment-safe encoding that keeps `/` (and the sub-delimiters Foundation keeps).
fn encode_path(p: &str) -> String {
    percent_encode(p, b"/!$&'()*+,;=:@")
}

fn lossy_prefix(data: &[u8], n: usize) -> String {
    String::from_utf8_lossy(&data[..data.len().min(n)]).into_owned()
}

fn decode<T: DeserializeOwned>(data: &[u8]) -> GitHubResult<T> {
    serde_json::from_slice(data).map_err(|e| GitHubError::Decoding(e.to_string()))
}

#[derive(Deserialize)]
struct AsyncMergeDetails {
    message: Option<String>,
    uuid: Option<String>,
}

#[derive(Deserialize)]
struct AsyncMerge {
    status: Option<String>,
    details: Option<AsyncMergeDetails>,
    /// 409: the request already pending for this PR.
    uuid: Option<String>,
}

impl GitHubClient {
    /// The production client: `ureq`, `gh auth token` / Keychain, api.github.com.
    pub fn new() -> Self {
        Self::with(
            Arc::new(UreqTransport::new()),
            Arc::new(GHTokenProvider::default()),
            "https://api.github.com",
        )
    }

    pub fn with(transport: Arc<dyn HttpTransport>, tokens: Arc<dyn TokenProvider>, api_base: &str) -> Self {
        GitHubClient {
            transport,
            tokens,
            api_base: api_base.trim_end_matches('/').to_string(),
            async_merge_poll_interval: Duration::from_secs(1),
        }
    }

    // MARK: Transport helpers

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.api_base, encode_path(path.trim_start_matches('/')))
    }

    fn request(&self, method: &str, url: String, accept: &str) -> GitHubResult<HttpRequest> {
        let token = self.tokens.token()?;
        Ok(HttpRequest {
            method: method.into(),
            url,
            headers: vec![
                ("Authorization".into(), format!("Bearer {token}")),
                ("Accept".into(), accept.into()),
                ("User-Agent".into(), "ReviewBuddy".into()),
                ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
            ],
            body: None,
            timeout: Duration::from_secs(60),
        })
    }

    fn send(&self, r: HttpRequest) -> GitHubResult<HttpResponse> {
        self.transport.send(r).map_err(GitHubError::Transport)
    }

    /// Runs a GraphQL document. `None` variables are left out of the request.
    ///
    /// Partial errors in a query are tolerated when the data still decodes; in a mutation
    /// any error fails the call with GitHub's messages; a `NOT_FOUND` error is `NotFound`.
    pub fn graphql<T: DeserializeOwned>(&self, query: &str, variables: &[(&str, Option<Value>)]) -> GitHubResult<T> {
        let mut r = self.request("POST", format!("{}/graphql", self.api_base), "application/json")?;
        let vars: Map<String, Value> =
            variables.iter().filter_map(|(k, v)| v.clone().map(|v| (k.to_string(), v))).collect();
        r.body = Some(serde_json::to_vec(&json!({ "query": query, "variables": vars })).expect("JSON body"));
        r.headers.push(("Content-Type".into(), "application/json".into()));
        let resp = self.send(r)?;
        if !(200..300).contains(&resp.status) {
            if resp.status == 401 {
                self.tokens.invalidate();
                return Err(GitHubError::NotAuthenticated);
            }
            return Err(GitHubError::Http(resp.status, lossy_prefix(&resp.body, 500)));
        }
        let root: Value = decode(&resp.body)?;
        // Check errors before decoding `data`: a refused mutation comes back as
        // {"data": {"mutationName": null}, "errors": [...]}, which must be a failure
        // with GitHub's message (e.g. "Can not approve your own pull request").
        let errors: Vec<GraphQLErrorDTO> =
            root.get("errors").and_then(|e| serde_json::from_value(e.clone()).ok()).unwrap_or_default();
        let messages = || errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>();
        let is_mutation = query.trim().starts_with("mutation");
        if !errors.is_empty() {
            if errors.iter().any(|e| e.kind.as_deref() == Some("NOT_FOUND")) {
                return Err(GitHubError::NotFound);
            }
            if is_mutation {
                return Err(GitHubError::GraphQL(messages()));
            }
        }
        match root.get("data") {
            None | Some(Value::Null) => Err(GitHubError::GraphQL(if errors.is_empty() {
                vec!["Empty response".into()]
            } else {
                messages()
            })),
            Some(data) => T::deserialize(data).map_err(|e| {
                // Queries tolerate partial errors, but not when the data we need is missing.
                if errors.is_empty() { GitHubError::Decoding(e.to_string()) } else { GitHubError::GraphQL(messages()) }
            }),
        }
    }

    /// Sends a REST request; statuses in `accepting` (besides 2xx) return their body instead of failing.
    fn rest(&self, method: &str, path: &str, body: Option<Value>, accepting: &[u16]) -> GitHubResult<Vec<u8>> {
        let mut r = self.request(method, self.url(path), "application/vnd.github+json")?;
        if let Some(body) = body {
            r.body = Some(serde_json::to_vec(&body).expect("JSON body"));
            r.headers.push(("Content-Type".into(), "application/json".into()));
        }
        let resp = self.send(r)?;
        if !((200..300).contains(&resp.status) || accepting.contains(&resp.status)) {
            if resp.status == 401 {
                self.tokens.invalidate();
                return Err(GitHubError::NotAuthenticated);
            }
            if resp.status == 404 {
                return Err(GitHubError::NotFound);
            }
            #[derive(Deserialize)]
            struct Detail {
                message: Option<String>,
            }
            #[derive(Deserialize)]
            struct Msg {
                message: String,
                errors: Option<Vec<Detail>>,
            }
            if let Ok(m) = serde_json::from_slice::<Msg>(&resp.body) {
                let mut msgs = vec![m.message];
                msgs.extend(m.errors.unwrap_or_default().into_iter().filter_map(|d| d.message));
                return Err(GitHubError::GraphQL(msgs));
            }
            return Err(GitHubError::Http(resp.status, lossy_prefix(&resp.body, 500)));
        }
        Ok(resp.body)
    }

    /// POSTs JSON to a REST endpoint; GraphQL has no "publish one comment now" mutation.
    fn rest_post(&self, path: &str, body: Value) -> GitHubResult<()> {
        self.rest("POST", path, Some(body), &[]).map(|_| ())
    }

    /// "This pull request is part of a stack and must be merged using the asynchronous merge REST API."
    pub fn needs_async_merge(message: &str) -> bool {
        message.to_lowercase().contains("asynchronous merge") || message.contains("merge-a-pull-request-asynchronously")
    }

    /// `PUT …/merge-async`, then polls the request until it's no longer pending (about a minute at most).
    fn merge_async(
        &self,
        pr: &PRRef,
        method: MergeMethod,
        expected_head_oid: &str,
        bypass_rules: bool,
    ) -> GitHubResult<MergeResult> {
        let base = format!("repos/{}/{}/pulls/{}/merge-async", pr.owner, pr.repo, pr.number);
        let body = json!({
            "sha": expected_head_oid,
            "merge_method": method.as_str().to_lowercase(),
            "bypass_rules": bypass_rules,
        });
        let mut r: AsyncMerge = decode(&self.rest("PUT", &base, Some(body), &[409])?)?;
        for _ in 0..60 {
            match r.status.as_deref() {
                Some("merged") => return Ok(MergeResult::Merged),
                Some("enqueued") => return Ok(MergeResult::Enqueued),
                Some("failed") => {
                    return Err(GitHubError::GraphQL(vec![r
                        .details
                        .and_then(|d| d.message)
                        .unwrap_or_else(|| "GitHub couldn't merge this pull request.".into())]))
                }
                _ => {}
            }
            let Some(uuid) = r.details.as_ref().and_then(|d| d.uuid.clone()).or(r.uuid.clone()) else {
                return Ok(MergeResult::Pending);
            };
            if !self.async_merge_poll_interval.is_zero() {
                std::thread::sleep(self.async_merge_poll_interval);
            }
            r = decode(&self.rest("GET", &format!("{base}/{uuid}"), None, &[])?)?;
        }
        Ok(MergeResult::Pending)
    }

    fn pr_vars(pr: &PRRef) -> Vec<(&'static str, Option<Value>)> {
        vec![
            ("owner", Some(json!(pr.owner))),
            ("name", Some(json!(pr.repo))),
            ("number", Some(json!(pr.number))),
        ]
    }
}

fn s(v: &str) -> Option<Value> {
    Some(Value::String(v.to_string()))
}

impl GitHubAPI for GitHubClient {
    fn search(&self, query: &str) -> GitHubResult<Vec<PRSummary>> {
        let res: q::SearchData = self.graphql(q::SEARCH, &[("q", s(query))])?;
        Ok(res.search.nodes.iter().filter_map(q::SearchNode::to_summary).collect())
    }

    fn pull_request(&self, pr_ref: &PRRef) -> GitHubResult<PRDetail> {
        let vars = Self::pr_vars(pr_ref);
        let first: q::PRData = self.graphql(q::PULL_REQUEST, &vars)?;
        let repo = &first.repository;
        let pr = repo.pull_request.as_ref().ok_or(GitHubError::NotFound)?;
        let mut files: Vec<PRFileMeta> = Vec::new();
        let push_files = |files: &mut Vec<PRFileMeta>, conn: &Option<q::Connection<q::FileDTO>>| {
            files.extend(conn.iter().flat_map(|c| c.nodes.iter()).map(|f| PRFileMeta {
                path: f.path.clone(),
                additions: f.additions,
                deletions: f.deletions,
                viewed: q::viewed_state(&f.viewer_viewed_state),
            }));
        };
        push_files(&mut files, &pr.files);
        let mut threads: Vec<ReviewThread> = pr.review_threads.nodes.iter().map(q::ThreadDTO::to_thread).collect();
        // Large PRs: keep paging files and threads independently.
        let mut files_page = pr.files.as_ref().map(|f| f.page_info.clone());
        while let Some(p) = files_page.filter(|p| p.has_next_page) {
            // Without a cursor the next request would return the same page forever.
            let Some(after) = p.end_cursor else { break };
            let mut v = vars.clone();
            v.push(("after", s(&after)));
            let next: q::PRFilesData = self.graphql(q::FILES_PAGE, &v)?;
            push_files(&mut files, &next.repository.pull_request.files);
            files_page = next.repository.pull_request.files.as_ref().map(|f| f.page_info.clone());
        }
        let mut threads_page = pr.review_threads.page_info.clone();
        while threads_page.has_next_page {
            let Some(after) = threads_page.end_cursor else { break };
            let mut v = vars.clone();
            v.push(("after", s(&after)));
            let next: q::PRThreadsData = self.graphql(q::THREADS_PAGE, &v)?;
            let conn = next.repository.pull_request.review_threads;
            threads.extend(conn.nodes.iter().map(q::ThreadDTO::to_thread));
            threads_page = conn.page_info;
        }
        let can_push = matches!(repo.viewer_permission.as_deref(), Some("ADMIN" | "MAINTAIN" | "WRITE"));
        Ok(PRDetail {
            pr_ref: pr_ref.clone(),
            node_id: pr.id.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
            author: pr.author.as_ref().map(|a| a.login.clone()).unwrap_or_else(|| "ghost".into()),
            author_avatar_url: pr.author.as_ref().and_then(|a| a.avatar_url.clone()),
            state: pr.state.clone(),
            is_draft: pr.is_draft,
            base_ref_name: pr.base_ref_name.clone(),
            head_ref_name: pr.head_ref_name.clone(),
            base_oid: pr.base_ref_oid.clone(),
            head_oid: pr.head_ref_oid.clone(),
            head_repo_name_with_owner: pr.head_repository.as_ref().map(|h| h.name_with_owner.clone()),
            head_repo_ssh_url: pr.head_repository.as_ref().and_then(|h| h.ssh_url.clone()),
            head_repo_url: pr.head_repository.as_ref().and_then(|h| h.url.clone()),
            maintainer_can_modify: pr.maintainer_can_modify,
            viewer_login: first.viewer.login.clone(),
            viewer_can_push: can_push,
            files,
            threads,
            pending_review_id: pr.reviews.nodes.iter().find(|r| r.viewer_did_author).map(|r| r.id.clone()),
            changed_files: pr.changed_files,
            additions: pr.additions,
            deletions: pr.deletions,
            conversation: pr.conversation(),
            status: pr.status(repo),
        })
    }

    fn diff(&self, pr: &PRRef) -> GitHubResult<Vec<u8>> {
        let url = self.url(&format!("repos/{}/{}/pulls/{}", pr.owner, pr.repo, pr.number));
        let resp = self.send(self.request("GET", url, "application/vnd.github.diff")?)?;
        match resp.status {
            200 => Ok(resp.body),
            406 | 422 => Err(GitHubError::DiffTooLarge),
            404 => Err(GitHubError::NotFound),
            code => Err(GitHubError::Http(code, lossy_prefix(&resp.body, 300))),
        }
    }

    fn head_oid(&self, pr: &PRRef) -> GitHubResult<String> {
        let r: q::HeadOidData = self.graphql(q::HEAD_OID, &Self::pr_vars(pr))?;
        r.repository.pull_request.map(|p| p.head_ref_oid).ok_or(GitHubError::NotFound)
    }

    fn file_contents(&self, owner: &str, repo: &str, path: &str, git_ref: &str) -> GitHubResult<Vec<u8>> {
        let url = format!(
            "{}?ref={}",
            self.url(&format!("repos/{owner}/{repo}/contents/{path}")),
            percent_encode(git_ref, b"/!$'()*+,;:@?")
        );
        let resp = self.send(self.request("GET", url, "application/vnd.github.raw+json")?)?;
        match resp.status {
            200 => Ok(resp.body),
            404 => Err(GitHubError::NotFound),
            code => Err(GitHubError::Http(code, String::new())),
        }
    }

    fn start_review(&self, pull_request_id: &str) -> GitHubResult<String> {
        let r: q::AddReviewData = self.graphql(q::ADD_REVIEW, &[("pr", s(pull_request_id))])?;
        Ok(r.add_pull_request_review.pull_request_review.id)
    }

    fn add_thread(
        &self,
        pull_request_id: &str,
        review_id: &str,
        anchor: &CodeAnchor,
        body: &str,
    ) -> GitHubResult<ReviewThread> {
        let multi = anchor.is_multiline();
        let side = anchor.side.as_str();
        let vars = [
            ("pr", s(pull_request_id)),
            ("review", s(review_id)),
            ("path", s(&anchor.path)),
            ("body", s(body)),
            ("line", Some(json!(anchor.end_line))),
            ("side", s(side)),
            ("startLine", multi.then(|| json!(anchor.start_line))),
            ("startSide", if multi { s(side) } else { None }),
        ];
        let r: q::AddThreadData = self.graphql(q::ADD_THREAD, &vars)?;
        r.add_pull_request_review_thread
            .thread
            .map(|t| t.to_thread())
            .ok_or_else(|| GitHubError::GraphQL(vec!["Could not create thread".into()]))
    }

    fn reply(&self, thread_id: &str, review_id: Option<&str>, body: &str) -> GitHubResult<ReviewComment> {
        let r: q::ReplyData =
            self.graphql(q::REPLY, &[("thread", s(thread_id)), ("review", review_id.and_then(s)), ("body", s(body))])?;
        Ok(r.add_pull_request_review_thread_reply.comment.to_comment())
    }

    fn add_single_comment(&self, pr: &PRRef, commit_id: &str, anchor: &CodeAnchor, body: &str) -> GitHubResult<()> {
        let mut j = json!({
            "body": body, "commit_id": commit_id, "path": anchor.path,
            "line": anchor.end_line, "side": anchor.side.as_str(),
        });
        if anchor.is_multiline() {
            j["start_line"] = json!(anchor.start_line);
            j["start_side"] = json!(anchor.side.as_str());
        }
        self.rest_post(&format!("repos/{}/{}/pulls/{}/comments", pr.owner, pr.repo, pr.number), j)
    }

    fn reply_single(&self, pr: &PRRef, _thread_id: &str, comment_database_id: i64, body: &str) -> GitHubResult<()> {
        self.rest_post(
            &format!("repos/{}/{}/pulls/{}/comments/{}/replies", pr.owner, pr.repo, pr.number, comment_database_id),
            json!({ "body": body }),
        )
    }

    fn add_comment(&self, subject_id: &str, body: &str) -> GitHubResult<ConversationComment> {
        let r: q::AddCommentData = self.graphql(q::ADD_COMMENT, &[("subject", s(subject_id)), ("body", s(body))])?;
        Ok(r.add_comment.comment_edge.node.to_comment())
    }

    fn merge_pull_request(
        &self,
        pr: &PRRef,
        id: &str,
        method: MergeMethod,
        expected_head_oid: &str,
        bypass_rules: bool,
    ) -> GitHubResult<MergeResult> {
        let vars = [("id", s(id)), ("method", s(method.as_str())), ("head", s(expected_head_oid))];
        match self.graphql::<IgnoredAny>(q::MERGE_PULL_REQUEST, &vars) {
            Ok(_) => Ok(MergeResult::Merged),
            Err(GitHubError::GraphQL(errs)) if errs.iter().any(|m| Self::needs_async_merge(m)) => {
                self.merge_async(pr, method, expected_head_oid, bypass_rules)
            }
            Err(e) => Err(e),
        }
    }

    fn set_pull_request_closed(&self, id: &str, closed: bool) -> GitHubResult<()> {
        let query = if closed { q::CLOSE_PULL_REQUEST } else { q::REOPEN_PULL_REQUEST };
        self.graphql::<IgnoredAny>(query, &[("id", s(id))]).map(|_| ())
    }

    fn update_comment(&self, id: &str, body: &str) -> GitHubResult<ReviewComment> {
        let r: q::UpdateCommentData = self.graphql(q::UPDATE_COMMENT, &[("id", s(id)), ("body", s(body))])?;
        Ok(r.update_pull_request_review_comment.pull_request_review_comment.to_comment())
    }

    fn delete_comment(&self, id: &str) -> GitHubResult<()> {
        self.graphql::<IgnoredAny>(q::DELETE_COMMENT, &[("id", s(id))]).map(|_| ())
    }

    fn submit_review(&self, review_id: &str, event: ReviewEvent, body: &str) -> GitHubResult<()> {
        let vars = [("review", s(review_id)), ("event", s(event.as_str())), ("body", s(body))];
        self.graphql::<IgnoredAny>(q::SUBMIT_REVIEW, &vars).map(|_| ())
    }

    fn delete_review(&self, review_id: &str) -> GitHubResult<()> {
        self.graphql::<IgnoredAny>(q::DELETE_REVIEW, &[("review", s(review_id))]).map(|_| ())
    }

    fn set_resolved(&self, thread_id: &str, resolved: bool) -> GitHubResult<bool> {
        let query = if resolved { q::RESOLVE } else { q::UNRESOLVE };
        let r: q::ResolveData = self.graphql(query, &[("id", s(thread_id))])?;
        Ok(r.resolve_review_thread.or(r.unresolve_review_thread).map(|p| p.thread.is_resolved).unwrap_or(resolved))
    }

    fn set_viewed(&self, pull_request_id: &str, path: &str, viewed: bool) -> GitHubResult<()> {
        let query = if viewed { q::MARK_VIEWED } else { q::UNMARK_VIEWED };
        self.graphql::<IgnoredAny>(query, &[("pr", s(pull_request_id)), ("path", s(path))]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CheckState, DiffSide, ReviewDecision, ViewedState};
    use crate::git::process::ProcessResult;

    type Handler = Box<dyn Fn(&HttpRequest, &str, &Map<String, Value>) -> (u16, String) + Send + Sync>;

    struct Sent {
        request: HttpRequest,
        query: String,
        variables: Map<String, Value>,
    }

    impl Sent {
        fn var(&self, k: &str) -> Option<&Value> {
            self.variables.get(k)
        }
        fn var_str(&self, k: &str) -> Option<&str> {
            self.var(k).and_then(Value::as_str)
        }
        fn var_int(&self, k: &str) -> Option<i64> {
            self.var(k).and_then(Value::as_i64)
        }
        fn body_json(&self) -> Value {
            serde_json::from_slice(self.request.body.as_ref().unwrap()).unwrap()
        }
        fn path(&self) -> String {
            let rest = self.request.url.strip_prefix("https://api.github.com").unwrap();
            rest.split('?').next().unwrap().to_string()
        }
    }

    /// Records requests and answers with canned responses.
    struct FakeTransport {
        sent: Mutex<Vec<Sent>>,
        handler: Handler,
    }

    impl FakeTransport {
        fn sent(&self) -> std::sync::MutexGuard<'_, Vec<Sent>> {
            self.sent.lock().unwrap()
        }
    }

    impl HttpTransport for FakeTransport {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
            let (mut query, mut variables) = (String::new(), Map::new());
            if let Some(Ok(Value::Object(obj))) = request.body.as_ref().map(|b| serde_json::from_slice::<Value>(b)) {
                query = obj.get("query").and_then(Value::as_str).unwrap_or("").to_string();
                if let Some(Value::Object(v)) = obj.get("variables") {
                    variables = v.clone();
                }
            }
            let (status, body) = (self.handler)(&request, &query, &variables);
            self.sent.lock().unwrap().push(Sent { request, query, variables });
            Ok(HttpResponse { status, body: body.into_bytes() })
        }
    }

    fn make_client(
        handler: impl Fn(&HttpRequest, &str, &Map<String, Value>) -> (u16, String) + Send + Sync + 'static,
    ) -> (GitHubClient, Arc<FakeTransport>) {
        let t = Arc::new(FakeTransport { sent: Mutex::new(vec![]), handler: Box::new(handler) });
        let mut c = GitHubClient::with(t.clone(), Arc::new(StaticTokenProvider::new("tok123")), "https://api.github.com");
        c.async_merge_poll_interval = Duration::ZERO;
        (c, t)
    }

    fn ok(body: &str) -> (u16, String) {
        (200, body.to_string())
    }

    const THREAD_JSON: &str = r#"{"id":"T1","path":"a.swift","line":12,"startLine":10,"originalLine":12,"diffSide":"RIGHT","isResolved":false,"isOutdated":false,
 "comments":{"nodes":[{"id":"C1","databaseId":5,"body":"hi","createdAt":"2026-01-02T03:04:05Z","state":"PENDING","url":"https://github.com/x","viewerDidAuthor":true,"author":{"login":"bob","avatarUrl":"https://a/b.png"}}]}}"#;

    fn pr_json(files_next: bool, threads_next: bool, permission: &str, pending: bool) -> String {
        let reviews = if pending { r#"{"id":"R_other","viewerDidAuthor":false},{"id":"R_mine","viewerDidAuthor":true}"# } else { "" };
        format!(
            r#"{{"data":{{"viewer":{{"login":"me"}},"repository":{{"viewerPermission":"{permission}","pullRequest":{{
"id":"PR_1","title":"Title","body":"Body","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"feat",
"baseRefOid":"aaa","headRefOid":"bbb","maintainerCanModify":true,"changedFiles":3,"additions":10,"deletions":2,
"author":{{"login":"alice","avatarUrl":null}},"headRepository":{{"nameWithOwner":"o/r","sshUrl":"git@github.com:o/r.git","url":"https://github.com/o/r"}},
"files":{{"pageInfo":{{"hasNextPage":{files_next},"endCursor":"F1"}},"nodes":[{{"path":"a.swift","additions":5,"deletions":1,"viewerViewedState":"VIEWED"}}]}},
"reviewThreads":{{"pageInfo":{{"hasNextPage":{threads_next},"endCursor":"T1c"}},"nodes":[{THREAD_JSON}]}},
"reviews":{{"nodes":[{reviews}]}}
}}}}}}}}"#
        )
    }

    /// Inserts `extra` before the final `}}}}` (into the pullRequest object).
    fn splice(base: String, extra: &str) -> String {
        let at = base.rfind("}}}}").unwrap();
        let mut s = base;
        s.insert_str(at, extra);
        s
    }

    fn pr1() -> PRRef {
        PRRef::new("o", "r", 1)
    }

    fn pr7() -> PRRef {
        PRRef::new("o", "r", 7)
    }

    #[test]
    fn sends_auth_headers() {
        let (c, t) = make_client(|_, _, _| ok(r#"{"data":{"search":{"nodes":[]}}}"#));
        c.search("x").unwrap();
        let sent = t.sent();
        let r = &sent[0].request;
        assert_eq!(r.header("Authorization"), Some("Bearer tok123"));
        assert_eq!(r.header("User-Agent"), Some("ReviewBuddy"));
        assert_eq!(r.header("X-GitHub-Api-Version"), Some("2022-11-28"));
        assert_eq!(r.header("Content-Type"), Some("application/json"));
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://api.github.com/graphql");
        assert_eq!(sent[0].query, q::SEARCH);
    }

    #[test]
    fn search_maps_summaries() {
        let (c, t) = make_client(|_, _, _| {
            ok(r#"{"data":{"search":{"nodes":[
              {"number":7,"title":"Fix","isDraft":true,"updatedAt":"2026-05-01T00:00:00Z","additions":3,"deletions":4,"reviewDecision":"APPROVED",
               "repository":{"name":"r","owner":{"login":"o"}},"author":{"login":"al","avatarUrl":"https://x/y.png"},"comments":{"totalCount":9},
               "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]}},
              {}
            ]}}}"#)
        });
        let res = c.search(InboxSection::ReviewRequested.query()).unwrap();
        assert_eq!(t.sent()[0].var_str("q"), Some(InboxSection::ReviewRequested.query()));
        assert_eq!(res.len(), 1);
        let s = &res[0];
        assert_eq!(s.pr_ref, pr7());
        assert!(s.title == "Fix" && s.author == "al" && s.is_draft);
        assert_eq!(s.author_avatar_url.as_deref(), Some("https://x/y.png"));
        assert!(s.checks == CheckState::Failure && s.review_decision == ReviewDecision::Approved && s.comment_count == 9);
        assert!(s.additions == 3 && s.deletions == 4);
        assert_eq!(s.updated_at.to_rfc3339(), "2026-05-01T00:00:00+00:00");
    }

    #[test]
    fn pull_request_paginates() {
        let first = pr_json(true, true, "WRITE", true);
        let (c, t) = make_client(move |_, q, vars| {
            if q.contains("viewer { login }") {
                return ok(&first);
            }
            if q.contains("files(first: 100, after") {
                let after = vars.get("after").and_then(Value::as_str).unwrap_or("");
                let next = after == "F1";
                return ok(&format!(
                    r#"{{"data":{{"repository":{{"pullRequest":{{"files":{{"pageInfo":{{"hasNextPage":{next},"endCursor":"F2"}},
                    "nodes":[{{"path":"f-{after}","additions":1,"deletions":0,"viewerViewedState":"UNVIEWED"}}]}}}}}}}}}}"#
                ));
            }
            if q.contains("reviewThreads(first: 100, after") {
                let t2 = THREAD_JSON.replace("\"T1\"", "\"T2\"");
                return ok(&format!(
                    r#"{{"data":{{"repository":{{"pullRequest":{{"reviewThreads":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[{t2}]}}}}}}}}}}"#
                ));
            }
            (500, "unexpected".into())
        });
        let pr = c.pull_request(&pr1()).unwrap();
        assert_eq!(pr.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.swift", "f-F1", "f-F2"]);
        assert!(pr.files[0].viewed == ViewedState::Viewed && pr.files[1].viewed == ViewedState::Unviewed);
        assert_eq!(pr.threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["T1", "T2"]);
        assert_eq!(pr.pending_review_id.as_deref(), Some("R_mine"));
        assert!(pr.viewer_can_push && pr.viewer_login == "me");
        assert!(pr.node_id == "PR_1" && pr.base_oid == "aaa" && pr.head_oid == "bbb" && pr.author == "alice");
        assert_eq!(pr.head_repo_ssh_url.as_deref(), Some("git@github.com:o/r.git"));
        assert!(pr.maintainer_can_modify && pr.changed_files == 3 && pr.additions == 10 && pr.deletions == 2);
        let th = &pr.threads[0];
        assert!(th.line == Some(12) && th.start_line == Some(10) && th.side == DiffSide::Right);
        let c0 = &th.comments[0];
        assert!(c0.is_pending && c0.viewer_did_author && c0.database_id == Some(5));
        assert_eq!(c0.author_avatar_url.as_deref(), Some("https://a/b.png"));
        assert!(th.is_pending());
        let sent = t.sent();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[1].var_int("number"), Some(1));
        assert_eq!(sent[1].var_str("after"), Some("F1"));
        assert_eq!(sent[0].var("after"), None);
    }

    #[test]
    fn pull_request_read_permission_and_no_pending() {
        let body = pr_json(false, false, "READ", false);
        let (c, t) = make_client(move |_, _, _| ok(&body));
        let pr = c.pull_request(&pr1()).unwrap();
        assert!(!pr.viewer_can_push);
        assert_eq!(pr.pending_review_id, None);
        assert_eq!(t.sent().len(), 1);
    }

    #[test]
    fn pull_request_conversation() {
        let extra = r#"
,"comments":{"nodes":[{"id":"IC1","body":"LGTM?","createdAt":"2026-01-03T00:00:00Z","url":"https://github.com/c","author":{"login":"carol","avatarUrl":null}}]},
"allReviews":{"nodes":[
  {"id":"RV1","state":"APPROVED","body":"","submittedAt":"2026-01-04T00:00:00Z","url":null,"author":{"login":"dave","avatarUrl":null}},
  {"id":"RV2","state":"PENDING","body":"draft","submittedAt":null,"url":null,"author":{"login":"me","avatarUrl":null}}]},
"commits":{"nodes":[
  {"commit":{"oid":"abc123","messageHeadline":"Fix it","committedDate":"2026-01-01T00:00:00Z","author":{"name":"Eve","user":{"login":"eve","avatarUrl":null}},"statusCheckRollup":{"state":"FAILURE"}}},
  {"commit":{"oid":"def456","messageHeadline":"Bot commit","committedDate":"2026-01-02T00:00:00Z","author":{"name":"Some Bot","user":null},"statusCheckRollup":null}}]}
"#;
        let body = splice(pr_json(false, false, "WRITE", true), extra);
        let (c, _) = make_client(move |_, _, _| ok(&body));
        let conv = c.pull_request(&pr1()).unwrap().conversation.unwrap();
        assert_eq!(conv.comments.iter().map(|c| c.author.as_str()).collect::<Vec<_>>(), ["carol"]);
        assert_eq!(conv.comments[0].url.as_deref(), Some("https://github.com/c"));
        // The viewer's pending draft is left out.
        assert_eq!(conv.reviews.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(), ["APPROVED"]);
        // Name when there's no GitHub user.
        assert_eq!(conv.commits.iter().map(|c| c.author.as_str()).collect::<Vec<_>>(), ["eve", "Some Bot"]);
        assert!(conv.commits[0].checks == CheckState::Failure && conv.commits[1].checks == CheckState::None);
        assert_eq!(conv.commits[0].headline, "Fix it");
    }

    #[test]
    fn pull_request_merge_status_and_checks() {
        let extra = r#"
,"mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","reviewDecision":"REVIEW_REQUIRED","viewerCanMergeAsAdmin":false,
"headCommit":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[
  {"__typename":"CheckRun","name":"test","conclusion":"FAILURE","status":"COMPLETED","detailsUrl":"https://ci/1","checkSuite":{"app":{"name":"GitHub Actions"}}},
  {"__typename":"CheckRun","name":"build","conclusion":null,"status":"IN_PROGRESS","detailsUrl":null,"checkSuite":null},
  {"__typename":"StatusContext","context":"ci/legacy","state":"EXPECTED","targetUrl":null}]}}}}]}
"#;
        let body = splice(pr_json(false, false, "WRITE", true), extra);
        let (c, _) = make_client(move |_, _, _| ok(&body));
        let st = c.pull_request(&pr1()).unwrap().status.unwrap();
        assert!(st.merge_state_status == "BLOCKED" && st.review_decision.as_deref() == Some("REVIEW_REQUIRED"));
        assert_eq!(st.checks.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["test", "build", "ci/legacy"]);
        assert_eq!(st.viewer_can_merge_as_admin, Some(false)); // fixture viewer has WRITE, not ADMIN
        assert!(st.checks[0].is_failure() && st.checks[0].app.as_deref() == Some("GitHub Actions"));
        assert_eq!(st.checks[0].url.as_deref(), Some("https://ci/1"));
        assert!(st.checks[1].is_pending() && st.checks[2].is_pending()); // no conclusion yet; EXPECTED status
        assert_eq!(st.allowed_merge_methods, None); // fixture has no *MergeAllowed flags
        assert_eq!(st.default_merge_method, None);
    }

    #[test]
    fn merge_methods_and_admin_bypass() {
        let base = pr_json(false, false, "ADMIN", true).replacen(
            r#""viewerPermission":"ADMIN","#,
            r#""viewerPermission":"ADMIN","mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":true,"viewerDefaultMergeMethod":"SQUASH","#,
            1,
        );
        let body = splice(base, r#","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","viewerCanMergeAsAdmin":false"#);
        let (c, _) = make_client(move |_, _, _| ok(&body));
        let st = c.pull_request(&pr1()).unwrap().status.unwrap();
        assert_eq!(st.allowed_merge_methods, Some(vec![MergeMethod::Squash, MergeMethod::Rebase]));
        assert_eq!(st.default_merge_method, Some(MergeMethod::Squash));
        // GitHub said false, but repo admins can bypass anyway.
        assert_eq!(st.viewer_can_merge_as_admin, Some(true));
        assert!(st.can_bypass());
        assert!(st.checks.is_empty() && st.review_decision.is_none());
    }

    #[test]
    fn add_comment_mutation() {
        let (c, t) = make_client(|_, _, _| {
            ok(r#"{"data":{"addComment":{"commentEdge":{"node":{"id":"IC9","body":"hi","createdAt":"2026-01-05T00:00:00Z","url":null,"author":{"login":"me","avatarUrl":null}}}}}}"#)
        });
        let comment = c.add_comment("PR_1", "hi").unwrap();
        assert!(comment.id == "IC9" && comment.author == "me" && comment.body == "hi");
        assert_eq!(t.sent()[0].var_str("subject"), Some("PR_1"));
        assert_eq!(t.sent()[0].var_str("body"), Some("hi"));
    }

    #[test]
    fn merge_pull_request_mutation() {
        let (c, t) = make_client(|_, _, _| ok(r#"{"data":{"mergePullRequest":{"pullRequest":{"state":"MERGED"}}}}"#));
        assert_eq!(c.merge_pull_request(&pr7(), "PR_1", MergeMethod::Squash, "abc", false), Ok(MergeResult::Merged));
        {
            let sent = t.sent();
            assert!(sent[0].query.contains("mergePullRequest"));
            assert_eq!(sent[0].var_str("method"), Some("SQUASH"));
            assert_eq!(sent[0].var_str("head"), Some("abc"));
            assert_eq!(sent[0].var_str("id"), Some("PR_1"));
        }
        let (c2, _) = make_client(|_, _, _| {
            ok(r#"{"data":{"mergePullRequest":null},"errors":[{"type":"UNPROCESSABLE","message":"Head branch was modified. Review and try the merge again."}]}"#)
        });
        assert_eq!(
            c2.merge_pull_request(&pr7(), "PR_1", MergeMethod::Merge, "old", false),
            Err(GitHubError::GraphQL(vec!["Head branch was modified. Review and try the merge again.".into()]))
        );
    }

    const STACK_ERROR: &str = r#"{"data":{"mergePullRequest":null},"errors":[{"type":"UNPROCESSABLE","message":"This pull request is part of a stack and must be merged using the asynchronous merge REST API. For more information, see https://docs.github.com/rest/pulls/pulls#merge-a-pull-request-asynchronously"}]}"#;

    #[test]
    fn stacked_pr_falls_back_to_async_merge_and_polls() {
        let (c, t) = make_client(|r, _, _| match (r.method.as_str(), r.url.as_str()) {
            ("PUT", _) => (202, r#"{"status":"pending","details":{"message":"Merging","uuid":"U1"}}"#.into()),
            ("GET", "https://api.github.com/repos/o/r/pulls/7/merge-async/U1") => {
                ok(r#"{"status":"merged","details":{"message":"Merged","sha":"m1"}}"#)
            }
            _ => ok(STACK_ERROR),
        });
        assert_eq!(c.merge_pull_request(&pr7(), "PR_1", MergeMethod::Squash, "abc", true), Ok(MergeResult::Merged));
        let sent = t.sent();
        assert_eq!(sent[1].path(), "/repos/o/r/pulls/7/merge-async");
        assert_eq!(sent[1].request.method, "PUT");
        assert_eq!(sent[1].request.header("Accept"), Some("application/vnd.github+json"));
        let j = sent[1].body_json();
        assert!(j["sha"] == "abc" && j["merge_method"] == "squash" && j["bypass_rules"] == true);
        assert_eq!(sent.len(), 3);
    }

    #[test]
    fn async_merge_reports_queue_and_failure() {
        let (c, _) = make_client(|r, _, _| {
            if r.method == "PUT" { ok(r#"{"status":"enqueued","details":{"message":"Queued"}}"#) } else { ok(STACK_ERROR) }
        });
        assert_eq!(c.merge_pull_request(&pr7(), "PR_1", MergeMethod::Merge, "abc", false), Ok(MergeResult::Enqueued));
        let (c2, _) = make_client(|r, _, _| match r.method.as_str() {
            "PUT" => (409, r#"{"uuid":"U9","status":"pending","details":{}}"#.into()),
            "GET" => ok(r#"{"status":"failed","details":{"message":"Head branch was modified"}}"#),
            _ => ok(STACK_ERROR),
        });
        assert_eq!(
            c2.merge_pull_request(&pr7(), "PR_1", MergeMethod::Merge, "abc", false),
            Err(GitHubError::GraphQL(vec!["Head branch was modified".into()]))
        );
        // Accepted but no request id to poll: still pending.
        let (c3, _) = make_client(|r, _, _| if r.method == "PUT" { (202, "{}".into()) } else { ok(STACK_ERROR) });
        assert_eq!(c3.merge_pull_request(&pr7(), "PR_1", MergeMethod::Rebase, "abc", false), Ok(MergeResult::Pending));
    }

    #[test]
    fn single_comment_and_reply_use_rest_right_away() {
        let (c, t) = make_client(|_, _, _| (201, r#"{"id":9}"#.into()));
        let anchor = CodeAnchor::new("a.swift", DiffSide::Right, 3, 5);
        c.add_single_comment(&pr7(), "abc", &anchor, "hi").unwrap();
        c.reply_single(&pr7(), "T", 42, "yo").unwrap();
        {
            let sent = t.sent();
            assert_eq!(sent[0].path(), "/repos/o/r/pulls/7/comments");
            assert_eq!(sent[0].request.method, "POST");
            let j = sent[0].body_json();
            assert!(j["commit_id"] == "abc" && j["line"] == 5 && j["start_line"] == 3);
            assert!(j["side"] == "RIGHT" && j["start_side"] == "RIGHT" && j["path"] == "a.swift" && j["body"] == "hi");
            assert_eq!(sent[1].path(), "/repos/o/r/pulls/7/comments/42/replies");
            assert_eq!(sent[1].body_json(), json!({ "body": "yo" }));
        }
        // Single-line: no start_line/start_side.
        c.add_single_comment(&pr7(), "abc", &CodeAnchor::new("a.swift", DiffSide::Left, 4, 4), "hi").unwrap();
        let j = t.sent()[2].body_json();
        assert!(j.get("start_line").is_none() && j.get("start_side").is_none() && j["side"] == "LEFT");

        let (c2, _) = make_client(|_, _, _| {
            (422, r#"{"message":"Validation Failed","errors":[{"message":"line must be part of the diff"}]}"#.into())
        });
        assert_eq!(
            c2.add_single_comment(&pr7(), "abc", &anchor, "hi"),
            Err(GitHubError::GraphQL(vec!["Validation Failed".into(), "line must be part of the diff".into()]))
        );
        let (c3, _) = make_client(|_, _, _| (500, "boom".into()));
        assert_eq!(c3.reply_single(&pr7(), "T", 1, "x"), Err(GitHubError::Http(500, "boom".into())));
        let (c4, _) = make_client(|_, _, _| (404, "{}".into()));
        assert_eq!(c4.reply_single(&pr7(), "T", 1, "x"), Err(GitHubError::NotFound));
    }

    #[test]
    fn close_and_reopen_mutations() {
        let (c, t) = make_client(|_, _, _| ok(r#"{"data":{"closePullRequest":{"pullRequest":{"state":"CLOSED"}}}}"#));
        c.set_pull_request_closed("PR_1", true).unwrap();
        c.set_pull_request_closed("PR_1", false).unwrap();
        let sent = t.sent();
        assert!(sent[0].query.contains("closePullRequest") && sent[1].query.contains("reopenPullRequest"));
        assert_eq!(sent[0].var_str("id"), Some("PR_1"));
    }

    #[test]
    fn pull_request_without_conversation_fields() {
        let body = pr_json(false, false, "WRITE", true);
        let (c, _) = make_client(move |_, _, _| ok(&body));
        let pr = c.pull_request(&pr1()).unwrap();
        assert_eq!(pr.conversation, None);
        assert_eq!(pr.status, None);
    }

    #[test]
    fn pull_request_null_is_not_found() {
        let (c, _) = make_client(|_, _, _| {
            ok(r#"{"data":{"viewer":{"login":"me"},"repository":{"viewerPermission":"READ","pullRequest":null}}}"#)
        });
        assert_eq!(c.pull_request(&pr1()), Err(GitHubError::NotFound));
    }

    #[test]
    fn graphql_errors() {
        let (c, _) = make_client(|_, _, _| ok(r#"{"data":null,"errors":[{"message":"bad thing"},{"message":"worse"}]}"#));
        assert_eq!(c.search("x"), Err(GitHubError::GraphQL(vec!["bad thing".into(), "worse".into()])));
        let (c2, _) = make_client(|_, _, _| ok(r#"{"data":null,"errors":[{"message":"nope","type":"NOT_FOUND"}]}"#));
        assert_eq!(c2.search("x"), Err(GitHubError::NotFound));
        let (c3, _) = make_client(|_, _, _| ok(r#"{"data":null}"#));
        assert_eq!(c3.search("x"), Err(GitHubError::GraphQL(vec!["Empty response".into()])));
        let (c4, _) = make_client(|_, _, _| ok(r#"{"data":{"search":{}}}"#));
        assert!(matches!(c4.search("x"), Err(GitHubError::Decoding(_))));
    }

    /// Real GitHub response for approving your own PR: data present but null, plus errors.
    #[test]
    fn refused_mutation_with_null_data_returns_github_message() {
        let (c, _) = make_client(|_, _, _| {
            ok(r#"{"data":{"submitPullRequestReview":null},"errors":[{"type":"UNPROCESSABLE","path":["submitPullRequestReview"],"message":"Could not approve for pull request review. Can not approve your own pull request"}]}"#)
        });
        assert_eq!(
            c.submit_review("R", ReviewEvent::Approve, ""),
            Err(GitHubError::GraphQL(vec![
                "Could not approve for pull request review. Can not approve your own pull request".into()
            ]))
        );
    }

    #[test]
    fn refused_resolve_fails_instead_of_decoding_error() {
        let (c, _) = make_client(|_, _, _| {
            ok(r#"{"data":{"resolveReviewThread":null},"errors":[{"type":"FORBIDDEN","message":"Resource not accessible"}]}"#)
        });
        assert_eq!(c.set_resolved("T", true), Err(GitHubError::GraphQL(vec!["Resource not accessible".into()])));
    }

    #[test]
    fn query_keeps_partial_data_despite_errors() {
        // A search where one result is inaccessible still returns the others.
        let (c, _) = make_client(|_, _, _| {
            ok(r#"{"data":{"search":{"nodes":[{"number":1,"title":"t","repository":{"name":"r","owner":{"login":"o"}},"updatedAt":"2026-01-01T00:00:00Z"},{}]}},"errors":[{"type":"FORBIDDEN","message":"no access to one repo"}]}"#)
        });
        let r = c.search("x").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].author, "ghost");
    }

    #[test]
    fn http_errors() {
        let (c, _) = make_client(|_, _, _| (401, "unauthorized".into()));
        assert_eq!(c.search("x"), Err(GitHubError::NotAuthenticated));
        let (c2, _) = make_client(|_, _, _| (502, "gateway".into()));
        assert_eq!(c2.search("x"), Err(GitHubError::Http(502, "gateway".into())));
    }

    struct FailingTransport;
    impl HttpTransport for FailingTransport {
        fn send(&self, _: HttpRequest) -> Result<HttpResponse, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn transport_failure_is_reported() {
        let c = GitHubClient::with(Arc::new(FailingTransport), Arc::new(StaticTokenProvider::new("t")), "https://x");
        assert_eq!(c.search("x"), Err(GitHubError::Transport("offline".into())));
    }

    #[test]
    fn diff_statuses() {
        let (c, t) = make_client(|_, _, _| ok("diff --git a/x b/x\n"));
        let pr = PRRef::new("o", "r", 3);
        let d = c.diff(&pr).unwrap();
        assert!(String::from_utf8(d).unwrap().starts_with("diff --git"));
        {
            let sent = t.sent();
            assert_eq!(sent[0].path(), "/repos/o/r/pulls/3");
            assert_eq!(sent[0].request.method, "GET");
            assert_eq!(sent[0].request.header("Accept"), Some("application/vnd.github.diff"));
            assert_eq!(sent[0].request.body, None);
        }
        for (code, err) in [
            (406, GitHubError::DiffTooLarge),
            (422, GitHubError::DiffTooLarge),
            (404, GitHubError::NotFound),
            (500, GitHubError::Http(500, "x".into())),
        ] {
            let (c2, _) = make_client(move |_, _, _| (code, "x".into()));
            assert_eq!(c2.diff(&pr), Err(err));
        }
    }

    #[test]
    fn file_contents() {
        let (c, t) = make_client(|_, _, _| ok("content"));
        let d = c.file_contents("o", "r", "src/a b.swift", "abc").unwrap();
        assert_eq!(d, b"content");
        {
            let sent = t.sent();
            assert_eq!(sent[0].request.url, "https://api.github.com/repos/o/r/contents/src/a%20b.swift?ref=abc");
            assert_eq!(sent[0].request.header("Accept"), Some("application/vnd.github.raw+json"));
        }
        let (c2, _) = make_client(|_, _, _| (404, String::new()));
        assert_eq!(c2.file_contents("o", "r", "x", "y"), Err(GitHubError::NotFound));
        let (c3, _) = make_client(|_, _, _| (403, "nope".into()));
        assert_eq!(c3.file_contents("o", "r", "x", "y"), Err(GitHubError::Http(403, String::new())));
    }

    #[test]
    fn head_oid() {
        let (c, t) = make_client(|_, _, _| ok(r#"{"data":{"repository":{"pullRequest":{"headRefOid":"cafe"}}}}"#));
        assert_eq!(c.head_oid(&pr7()), Ok("cafe".into()));
        assert_eq!(t.sent()[0].var_int("number"), Some(7));
        let (c2, _) = make_client(|_, _, _| ok(r#"{"data":{"repository":{"pullRequest":null}}}"#));
        assert_eq!(c2.head_oid(&pr7()), Err(GitHubError::NotFound));
    }

    #[test]
    fn start_review() {
        let (c, t) = make_client(|_, _, _| ok(r#"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R1"}}}}"#));
        assert_eq!(c.start_review("PR_1"), Ok("R1".into()));
        assert!(t.sent()[0].query.contains("addPullRequestReview("));
        assert_eq!(t.sent()[0].var_str("pr"), Some("PR_1"));
    }

    #[test]
    fn add_thread_single_and_multi_line() {
        let resp = format!(r#"{{"data":{{"addPullRequestReviewThread":{{"thread":{THREAD_JSON}}}}}}}"#);
        let (c, t) = make_client(move |_, _, _| ok(&resp));
        let single = c.add_thread("PR", "R", &CodeAnchor::new("a.swift", DiffSide::Left, 4, 4), "b").unwrap();
        assert_eq!(single.id, "T1");
        {
            let sent = t.sent();
            let v = &sent[0];
            assert!(v.query.contains("addPullRequestReviewThread"));
            assert!(v.var_str("path") == Some("a.swift") && v.var_int("line") == Some(4) && v.var_str("side") == Some("LEFT"));
            assert!(v.var("startLine").is_none() && v.var("startSide").is_none());
            assert!(v.var_str("body") == Some("b") && v.var_str("review") == Some("R") && v.var_str("pr") == Some("PR"));
        }
        c.add_thread("PR", "R", &CodeAnchor::new("a.swift", DiffSide::Right, 9, 3), "m").unwrap();
        let sent = t.sent();
        let v = &sent[1];
        assert!(v.var_int("line") == Some(9) && v.var_int("startLine") == Some(3) && v.var_str("startSide") == Some("RIGHT"));
    }

    #[test]
    fn add_thread_null_thread_fails() {
        let (c, _) = make_client(|_, _, _| ok(r#"{"data":{"addPullRequestReviewThread":{"thread":null}}}"#));
        assert_eq!(
            c.add_thread("P", "R", &CodeAnchor::new("a", DiffSide::Right, 1, 1), "x").map(|t| t.id),
            Err(GitHubError::GraphQL(vec!["Could not create thread".into()]))
        );
    }

    const COMMENT_JSON: &str = r#"{"id":"C9","databaseId":9,"body":"yo","createdAt":"2026-01-02T03:04:05Z","state":"SUBMITTED","url":null,"viewerDidAuthor":true,"author":{"login":"me","avatarUrl":null}}"#;

    #[test]
    fn reply_and_update() {
        let (c, t) = make_client(|_, q, _| {
            if q.contains("addPullRequestReviewThreadReply") {
                return ok(&format!(r#"{{"data":{{"addPullRequestReviewThreadReply":{{"comment":{COMMENT_JSON}}}}}}}"#));
            }
            ok(&format!(r#"{{"data":{{"updatePullRequestReviewComment":{{"pullRequestReviewComment":{COMMENT_JSON}}}}}}}"#))
        });
        let r = c.reply("T1", None, "yo").unwrap();
        assert!(r.id == "C9" && !r.is_pending && r.author == "me" && r.database_id == Some(9));
        assert_eq!(t.sent()[0].var_str("thread"), Some("T1"));
        assert!(t.sent()[0].var("review").is_none());
        c.reply("T1", Some("R1"), "yo").unwrap();
        assert_eq!(t.sent()[1].var_str("review"), Some("R1"));
        let u = c.update_comment("C9", "new").unwrap();
        assert_eq!(u.body, "yo");
        let sent = t.sent();
        assert!(sent[2].query.contains("updatePullRequestReviewComment"));
        assert!(sent[2].var_str("id") == Some("C9") && sent[2].var_str("body") == Some("new"));
    }

    #[test]
    fn simple_mutations() {
        let (c, t) = make_client(|_, q, _| {
            if q.contains("resolveReviewThread(") && !q.contains("unresolve") {
                return ok(r#"{"data":{"resolveReviewThread":{"thread":{"id":"T","isResolved":true}}}}"#);
            }
            if q.contains("unresolveReviewThread") {
                return ok(r#"{"data":{"unresolveReviewThread":{"thread":{"id":"T","isResolved":false}}}}"#);
            }
            ok(r#"{"data":{"x":{"clientMutationId":null}}}"#)
        });
        c.delete_comment("C1").unwrap();
        c.submit_review("R", ReviewEvent::RequestChanges, "pls").unwrap();
        c.delete_review("R").unwrap();
        assert_eq!(c.set_resolved("T", true), Ok(true));
        assert_eq!(c.set_resolved("T", false), Ok(false));
        c.set_viewed("PR", "a.swift", true).unwrap();
        c.set_viewed("PR", "a.swift", false).unwrap();
        let sent = t.sent();
        assert!(sent[0].query.contains("deletePullRequestReviewComment") && sent[0].var_str("id") == Some("C1"));
        assert!(sent[1].query.contains("submitPullRequestReview"));
        assert!(sent[1].var_str("event") == Some("REQUEST_CHANGES") && sent[1].var_str("body") == Some("pls"));
        assert!(sent[2].query.contains("deletePullRequestReview(") && sent[2].var_str("review") == Some("R"));
        assert!(sent[3].query.contains("resolveReviewThread") && sent[3].var_str("id") == Some("T"));
        assert!(sent[4].query.contains("unresolveReviewThread"));
        assert!(sent[5].query.contains("markFileAsViewed") && !sent[5].query.contains("unmark"));
        assert!(sent[5].var_str("path") == Some("a.swift") && sent[5].var_str("pr") == Some("PR"));
        assert!(sent[6].query.contains("unmarkFileAsViewed"));
    }

    #[test]
    fn inbox_sections() {
        assert_eq!(InboxSection::Mine.query(), "is:pr is:open author:@me archived:false sort:updated-desc");
        assert_eq!(InboxSection::ReviewRequested.title(), "Review requested");
        assert_eq!(serde_json::to_string(&InboxSection::ReviewRequested).unwrap(), "\"reviewRequested\"");
    }

    #[test]
    fn needs_async_merge_matches_github_messages() {
        assert!(GitHubClient::needs_async_merge("Must use the Asynchronous Merge API"));
        assert!(GitHubClient::needs_async_merge("see …#merge-a-pull-request-asynchronously"));
        assert!(!GitHubClient::needs_async_merge("Head branch was modified"));
    }

    // MARK: Token provider

    struct ScriptedRunner {
        result: Result<ProcessResult, String>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl CommandRunning for ScriptedRunner {
        fn run(
            &self,
            executable: &str,
            args: &[String],
            _: Option<&std::path::Path>,
            _: Option<&std::collections::HashMap<String, String>>,
            _: Option<&[u8]>,
        ) -> Result<ProcessResult, String> {
            self.calls.lock().unwrap().push((executable.into(), args.to_vec()));
            self.result.clone()
        }
    }

    fn runner(status: i32, stdout: &str) -> Arc<ScriptedRunner> {
        Arc::new(ScriptedRunner {
            result: Ok(ProcessResult { status, stdout: stdout.as_bytes().to_vec(), stderr: vec![] }),
            calls: Mutex::new(vec![]),
        })
    }

    #[test]
    fn gh_token_comes_first_and_is_cached() {
        let r = runner(0, "gho_abc\n");
        let p = GHTokenProvider::with_sources(
            r.clone(),
            Box::new(|| Some("/x/gh".into())),
            Box::new(|| Some("pat".into())),
        );
        assert_eq!(p.token(), Ok("gho_abc".into()));
        assert_eq!(p.token(), Ok("gho_abc".into()));
        assert_eq!(r.calls.lock().unwrap().clone(), vec![("/x/gh".to_string(), vec!["auth".to_string(), "token".to_string()])]);
        p.invalidate();
        p.token().unwrap();
        assert_eq!(r.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn keychain_pat_is_the_fallback() {
        // gh fails.
        let p = GHTokenProvider::with_sources(runner(1, ""), Box::new(|| Some("/x/gh".into())), Box::new(|| Some("pat".into())));
        assert_eq!(p.token(), Ok("pat".into()));
        // gh prints nothing.
        let p = GHTokenProvider::with_sources(runner(0, "  \n"), Box::new(|| Some("/x/gh".into())), Box::new(|| Some("pat".into())));
        assert_eq!(p.token(), Ok("pat".into()));
        // No gh at all.
        let r = runner(0, "unused");
        let p = GHTokenProvider::with_sources(r.clone(), Box::new(|| None), Box::new(|| Some("pat".into())));
        assert_eq!(p.token(), Ok("pat".into()));
        assert!(r.calls.lock().unwrap().is_empty());
        // Nothing anywhere.
        let p = GHTokenProvider::with_sources(runner(1, ""), Box::new(|| None), Box::new(|| Some(String::new())));
        assert_eq!(p.token(), Err(GitHubError::NotAuthenticated));
    }

    struct CountingTokens(Mutex<usize>);
    impl TokenProvider for CountingTokens {
        fn token(&self) -> Result<String, GitHubError> {
            Ok("t".into())
        }
        fn invalidate(&self) {
            *self.0.lock().unwrap() += 1;
        }
    }

    #[test]
    fn unauthorized_invalidates_the_token() {
        let tokens = Arc::new(CountingTokens(Mutex::new(0)));
        let t = Arc::new(FakeTransport { sent: Mutex::new(vec![]), handler: Box::new(|_, _, _| (401, String::new())) });
        let c = GitHubClient::with(t, tokens.clone(), "https://api.github.com/");
        assert_eq!(c.search("x"), Err(GitHubError::NotAuthenticated));
        assert_eq!(c.reply_single(&pr7(), "T", 1, "x"), Err(GitHubError::NotAuthenticated));
        assert_eq!(*tokens.0.lock().unwrap(), 2);
    }

    #[test]
    fn client_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GitHubClient>();
        assert_send_sync::<GHTokenProvider>();
    }
}
