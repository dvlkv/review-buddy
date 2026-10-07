# Review Buddy — Spec

A native macOS app for reviewing GitHub pull requests. It looks and behaves like GitHub's "Files changed" review UI, with one addition: when you select lines you can either **Comment** (a normal GitHub review comment) or **Discuss** (a private conversation with a Claude agent that can see the code and fix it).

## 1. Decisions

| Area | Decision |
|---|---|
| Platform | Native Swift, SwiftUI app shell, **macOS 26+** |
| Diff rendering | Fully native: a custom virtualized AppKit canvas (bitmap tiles + prefix-sum row index) hosted in SwiftUI, styled like GitHub. Inline threads/composers are SwiftUI views placed only near the viewport. See PERF.md |
| GitHub API | GraphQL for reviews, threads, viewed state and search; REST `.diff` for the PR diff, falling back to `git diff` in the local checkout when GitHub refuses (too large) |
| Auth | Token from `gh auth token`; a PAT stored in Keychain as fallback |
| AI engine | `claude` CLI started as a subprocess, with streaming JSON in and out (the same engine as the Agent SDK) |
| AI permissions | Full access (`bypassPermissions`), always run inside a worktree for the PR |
| Code on disk | Per repo: your mapped local clone if there is one, otherwise a managed clone. Always a separate worktree per PR |
| Comment mode | Pending review, like GitHub: comments build a draft review that you submit as Approve, Request changes or Comment |
| Fixes | The Discuss agent edits files and **commits to the PR branch** in its worktree. **Pushing needs explicit confirmation from the user** |
| Persistence | JSON files under Application Support (one per PR + settings), written atomically and debounced. Discuss conversations are anchored to code and marked outdated when it changes |
| Syntax highlighting | Built-in single-pass lexer (≈20 languages), not tree-sitter: no dependencies, ~0.1 ms per file |
| Distribution | Not sandboxed (it needs to start `git`, `gh` and `claude`), signed with a Developer ID |

## 2. Main screens

```
┌────────────┬──────────────┬─────────────────────────────────┬───────────────┐
│ Inbox      │ File tree    │ Diff (split / unified)          │ Discuss panel │
│            │              │                                 │ (collapsible) │
│ ▸ Review   │ ☑ src/a.ts   │  12  12   context               │ ◆ a.ts L40-44 │
│   requested│ ☐ src/b.ts   │  13     - old line              │ ───────────── │
│ ▸ Mine     │ ☐ README.md  │      13 + new line   ◆ AI       │ you: why ...  │
│ ▸ Pinned   │              │  ┌ thread: @bob "nit: ..." ┐    │ claude: ...   │
│   repos    │  3/12 viewed │  └ Reply · Resolve         ┘    │ [→ Comment]   │
│            │              │                                 │ [Fix it]      │
└────────────┴──────────────┴─────────────────────────────────┴───────────────┘
 Toolbar: PR title · branch · Summary ▾ · AI pre-review · Review (3 pending) ▾
```

### 2.1 Inbox (sidebar)
- **Review requested:** `is:pr is:open review-requested:@me archived:false`
- **Mine:** `is:pr is:open author:@me archived:false`
- **Pinned repos:** a list of `owner/repo` set by the user; shows the open PRs in each
- **Open by URL:** Cmd+L, paste `https://github.com/o/r/pull/N`
- Each row shows the title, repo, author, CI status, review status, and the time of the last update. The list refreshes when the app gets focus and every 5 minutes.

### 2.2 PR view
- **Header:** title, `head → base`, author, PR description (collapsible, rendered Markdown), and the **AI summary card** (see §6.1).
- **File tree:** grouped into folders, with +/- counts, a "Viewed" checkbox that syncs to GitHub (`markFileAsViewed` / `unmarkFileAsViewed`), and a filter box.
- **Diff:** see §3.
- **Review button:** shows how many comments are pending and opens the submit popover (body, plus Approve / Request changes / Comment).

## 3. Diff view

### 3.1 Rendering
- One virtualized canvas per PR. The flattened row model (`DiffLayout`, 16 bytes/row) holds file headers, hunk headers, diff lines (or aligned pairs in split mode), expanded context, placeholders and **inline blocks** (review threads, comment composers, Discuss markers, AI findings). Rows are drawn into 512 pt bitmap tiles; off-screen tiles render in idle time.
- Syntax highlighting uses a built-in table-driven lexer that tracks state per side within each hunk (block comments and multi-line strings carry across lines). Small files are highlighted synchronously when first drawn (~0.1 ms); files over 4,000 lines are highlighted on a background queue.
- Changed words inside paired deletion/addition lines get GitHub's darker emphasis (prefix/suffix diff snapped to word boundaries).
- **Split / unified** toggle, remembered per user. In split mode, deletions and additions are lined up within each change block, like GitHub does.
- Large files (>2,000 changed lines) and generated or binary files are collapsed by default with "Load diff".
- "Expand context" (↑/↓ arrows on hunk headers) loads lines from the file contents.
- Colors and spacing follow GitHub's Primer tokens and adapt to light and dark mode.

### 3.2 Selecting lines
- Click a line's gutter to select it; drag or Shift-click to select a range. A selection stays on one side (old or new), matching GitHub's rules for multi-line comments.
- A small floating bar appears next to the selection with two buttons:
  - **Comment** (`c`): opens an inline composer and adds a comment to the pending review.
  - **Discuss** (`d`): creates a Discuss thread anchored to the selection, puts a ◆ marker inline, and opens the side panel.

### 3.3 Review threads
- Shows every existing thread (from all reviewers) inline at its position. Outdated threads are collapsed and labeled "Outdated".
- You can reply, resolve and unresolve (`resolveReviewThread` / `unresolveReviewThread`).
- Comment bodies are rendered as GitHub-flavored Markdown.
- **Suggested changes:** the composer has an "Insert suggestion" button that pre-fills a ```` ```suggestion ```` block with the selected lines. Suggestion blocks in comments are shown as a small before/after diff.

## 4. Comments (pending review)

- The first **Comment** creates a pending review (`addPullRequestReview` with no `event`). Later comments are added with `addPullRequestReviewThread` (with path, line, side, startLine and startSide) or added as replies.
- When a PR is opened, any pending review you already have on GitHub is loaded, so drafts started on github.com appear here too.
- Pending comments get a "Pending" badge and can be edited or deleted until the review is submitted.
- Submit runs `submitPullRequestReview` with the event Approve, Request changes or Comment, plus an optional body. Cmd+Enter submits.

## 5. Discuss (the AI agent)

### 5.1 Workspace on disk
For each repo, the user chooses in Settings → Repositories:
- **Local clone:** a path to an existing clone. Worktrees are created under `<clone>/.git/review-buddy-worktrees/`.
- **Managed (default):** a bare clone at `~/Library/Application Support/ReviewBuddy/repos/<owner>/<repo>.git`.

Each PR gets one worktree:
```
git fetch <url> +refs/pull/<N>/head:refs/review-buddy/head-<N> +refs/heads/<base>:refs/review-buddy/base-<N>
git worktree add -B review-buddy/pr-<N> <worktreePath> refs/review-buddy/head-<N>
```
- The local branch `review-buddy/pr-<N>` is where fixes get committed.
- If the PR's head commit on GitHub moves on and there are no local commits that haven't been pushed, the worktree is fast-forwarded. If there are unpushed local commits, the app shows a banner ("Remote updated · N local commits") and offers to rebase.
- When the PR is merged or closed, the worktree is removed. Pinned and in-progress PRs are kept.

### 5.2 Running the agent
One `ClaudeSession` per Discuss thread, using a `claude` subprocess:
```
claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages \
       --permission-mode bypassPermissions \
       --append-system-prompt "<review-buddy system prompt>" \
       [--resume <session_id>]
```
- The working directory is the PR worktree. The `session_id` from the first `system/init` event is saved so the conversation can be resumed after a restart.
- Streamed events are shown in the panel: assistant text (streamed Markdown), tool calls (folded rows like "Read src/a.ts", "Bash: npm test" that can be expanded), and errors.
- The panel has a Stop button (sends an interrupt, or SIGINT as a fallback) and a model picker (defaults to the CLI's default model).
- Auth: whatever the local `claude` CLI is logged in with (a subscription or `ANTHROPIC_API_KEY`).

### 5.3 Context given automatically
The first user message is wrapped in a context block:
- Repo, PR number, title, description, base and head commits
- The anchor: file path, side, line range, the selected lines, plus ±20 lines of surrounding context
- The existing review threads on this file (author, body, resolved state); threads from other files appear as a one-line summary each

The full PR diff is **not** included. The agent can run `git diff <base>...HEAD` itself in the worktree.

### 5.4 Actions in the Discuss panel
- **→ Comment:** asks the agent (in a short side request, so the main conversation isn't polluted) to draft a review comment from the conversation. The draft opens in the inline composer at the same anchor; you edit it and add it to the pending review. If the agent proposes a code change for the selected lines, the draft uses a `suggestion` block.
- **Fix it:** sends a message to the same session: "Implement the fix discussed above, run the relevant checks if there are any, and commit to the current branch with a descriptive message. Don't push." Then:
  - The panel shows the commits that were produced (`git log origin-head..HEAD`) with their diffs.
  - A toolbar badge shows "N local commits", and a **Push** button opens a confirmation sheet listing the commits, then runs `git push <headRepoRemote> HEAD:<headRefName>`.
  - Fork PRs: push only works if `maintainerCanModify` is true; otherwise the button is disabled and explains why.
- **Fix requests** are their own model type (thread id, anchor, instructions, status). That way a batched "fix queue" can be added later without reworking things.

### 5.5 Persistence and anchoring
- Stored as JSON under `~/Library/Application Support/ReviewBuddy/prs/<owner>__<repo>__<n>.json`.
- `DiscussThread { id, repo, prNumber, path, side, startLine, endLine, commitSHA, anchorTextHash, sessionId, messages[], createdAt, status }`
- When a PR is reopened, each anchor is moved to the new head commit using the diff between the old commit and the new one. If the anchored lines changed or were deleted, the thread is marked **Outdated** (still readable and resumable, like GitHub's outdated threads).

## 6. Other AI features

### 6.1 PR summary and walkthrough
- A "Summarize" button in the PR header (it can be set to run automatically when a PR is opened) starts a one-off agent run in the worktree.
- The output is shown as a card: a summary of 3–6 sentences, a **suggested reading order** (clicking an item jumps to that file), and risky areas.
- It's cached per PR and head commit.

### 6.2 Whole-PR AI pre-review
- An "AI pre-review" button starts an agent run that reads the full diff and returns structured findings: `[{ path, side, startLine, endLine, severity, title, body, suggestion? }]`. The output is validated against a JSON schema; the exact CLI flag will be confirmed during implementation.
- Findings show up inline as **private suggested comments** (a distinct AI style) with these buttons:
  - **Accept:** opens the inline composer pre-filled with the finding, to add to the pending review
  - **Discuss:** turns the finding into a Discuss thread, with the finding as context
  - **Dismiss**
- Findings are saved per PR and head commit.

## 7. Keyboard

| Key | Action |
|---|---|
| `j` / `k` | Next / previous file |
| `n` / `p` | Next / previous hunk |
| `]` / `[` | Next / previous thread (review, Discuss or AI finding) |
| `v` | Toggle "Viewed" on the current file |
| `c` | Comment on the selected lines |
| `d` | Discuss the selected lines |
| `r` | Reply to the focused thread |
| `s` | Toggle split / unified |
| `⌘L` | Open PR by URL |
| `⌘K` | Command palette (PRs, files, actions) |
| `⌘↩` | Submit the comment, or the review when focus is on the review popover |
| `⌘\` | Toggle the Discuss panel |
| `Esc` | Close the composer or popover |

## 8. Architecture

```
ReviewBuddy (SwiftUI App)
├── App/            Window scenes, commands, settings
├── Features/
│   ├── Inbox/      Sidebar lists, search queries, refresh
│   ├── PR/         PR header, summary card, review submit popover
│   ├── Diff/       Canvas diff (tiles, sticky header, inline SwiftUI blocks), selection, composer, threads
│   ├── Discuss/    Side panel, event stream rendering, actions
│   └── PreReview/  Findings model and inline rendering
├── Services/
│   ├── GitHub/     GraphQL + REST client (URLSession, async/await), auth via gh
│   ├── Git/        Worktree manager (shells out to git), push
│   ├── Claude/     ClaudeSession: Process + stream-json encoder/decoder
│   ├── Diff/       Unified diff parser, split alignment, anchor remapping
│   └── Highlight/  Built-in lexer + language table; Primer colors in the UI theme
└── Persistence/    JSON LocalStore (AppSettings, PRLocalState: Discuss threads, fix requests, findings, summary)
```
- State is managed with `@Observable` models, one `PRStore` per open PR.
- All subprocesses go through one `ProcessRunner` (async, cancellable, captures stderr for errors).
- GraphQL queries are hand-written `.graphql` strings with `Codable` response types; we'll consider codegen later.

## 9. Milestones

1. **M0, scaffold:** Xcode project, app shell with three columns, `gh auth token` auth, Settings window.
2. **M1, inbox + read-only diff:** the three inbox sections plus open by URL; file tree; unified diff with syntax highlighting; navigation shortcuts.
3. **M2, review threads + pending review:** inline threads (read, reply, resolve), the Comment composer, pending review, submit.
4. **M3, GitHub feature parity:** split view, Viewed syncing, suggestion blocks and their preview, expand context, command palette.
5. **M4, worktrees + Discuss:** repo mapping, worktree manager, ClaudeSession, side panel, persistence and anchoring.
6. **M5, Discuss actions:** → Comment, Fix it (commit), local commits badge, Push with confirmation, handling remote updates.
7. **M6, AI extras:** PR summary and walkthrough, whole-PR pre-review.

## 10. Open questions
- **Sharing worktrees:** Discuss threads on the same PR share one worktree. Implemented: "Fix it" runs one at a time per PR; additional requests are queued.
- **Pre-review cost:** should "AI pre-review" run automatically for small PRs, or only when clicked?
- **Notifications:** should the app notify you (macOS notifications) when you get a new review request?
