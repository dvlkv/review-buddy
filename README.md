<p align="center">
  <img src="Resources/AppIcon.png" width="128" alt="Review Buddy icon">
</p>

<h1 align="center">Review Buddy</h1>

<p align="center">
  A fast desktop app for reviewing GitHub pull requests, with Claude as your review partner.<br>
  <a href="https://github.com/dvlkv/review-buddy/releases/latest"><b>Download the latest release</b></a>
</p>

<p align="center">
  <img src="docs/screenshots/discuss.png" alt="Review Buddy: diff, AI summary and a Discuss conversation with Claude">
</p>

Review Buddy looks and works like GitHub's "Files changed" tab, but it's a fast native app
(written in Rust, drawn with egui on the GPU), and it has Claude built in. Select some code and either **Comment** (a normal GitHub review
comment) or **Discuss** it privately with a Claude agent that has the PR checked out. The agent
can read the rest of the codebase, answer questions, and fix the code for you.

## Features

### A fast, GitHub-style diff

- Unified and split views, syntax highlighting, and expandable context.
- A file tree, "Viewed" checkboxes that sync with GitHub, and collapsible files.
- Diffs with thousands of files scroll smoothly: the diff is drawn natively, not in a web view
  (see [PERF.md](PERF.md)).
- Large PRs that GitHub won't send as one diff are built from a local checkout instead.

![Split view in dark mode](docs/screenshots/split.png)

### Review like on GitHub

- Comment on a line or a range by clicking or dragging the line numbers, or the **+** button.
- **Add single comment** posts right away. **Start a review** collects comments into a pending
  review that you submit as Comment, Approve or Request changes.
- Reply to threads, resolve them, edit or delete your comments, and insert suggestions.

![Comment box with "Add single comment" and "Start a review"](docs/screenshots/comment.png)

### Select text, then Comment or Discuss

- Select part of a line by dragging, or double-click to select a word, like in a text editor.
- A **Comment / Discuss** bar appears next to the cursor.

![Selecting code shows Comment and Discuss buttons](docs/screenshots/select.png)

### Discuss with Claude

- A private conversation about the selected code, shown in a side panel and marked inline in
  the diff. Nothing is posted to GitHub.
- Claude runs in a git worktree of the PR, so it can search and read the whole repository.
- **Fix it** has Claude change the code and commit to a local branch. You review the commits,
  then push them to the PR branch. Review Buddy never force-pushes.
- **Comment** turns the conclusion into a draft GitHub comment that you can edit before posting.

### AI summary and reading order

- A short summary of the PR, the risky areas, and a suggested reading order.
- The suggested files move to the top of the diff and the file tree under **Review first**.
  Right-click a file to change the order; it's saved per PR.
- **AI pre-review** runs a full first pass and lists findings that you can turn into comments.

![AI summary with reading order and risky areas](docs/screenshots/review.png)

### Summary screen

The **Summary** row above the file tree opens the PR's overview, like GitHub's Conversation tab:

- The conversation timeline (comments, reviews, threads) and the list of commits.
- Merge readiness: review status, checks with links to their logs, and conflicts.
- **Merge**, with the methods the repository allows, an admin "bypass rules" option, and a
  confirmation step.
- **Close / Reopen** the pull request, and a box for leaving a PR comment.

![Summary screen with conversation and merge status](docs/screenshots/summary.png)

### Stays up to date

- The inbox lists PRs waiting for your review and your own PRs, and refreshes automatically.
- When someone pushes to the open PR, a bar offers **Refresh**. Refresh reloads the diff and
  fast-forwards the local checkout, as long as that doesn't discard your local commits or
  uncommitted changes.

## Install

1. Download `ReviewBuddy-<version>.zip` from the
   [latest release](https://github.com/dvlkv/review-buddy/releases/latest) and unzip it.
2. Move **ReviewBuddy.app** to Applications.
3. The app isn't notarized yet. The first time, right-click it and choose **Open**, or run:
   ```sh
   xattr -dr com.apple.quarantine /Applications/ReviewBuddy.app
   ```

### Requirements

- macOS on Apple silicon (the app is plain Rust + egui, so Intel Macs and Linux build too, but
  releases target Apple silicon).
- [GitHub CLI](https://cli.github.com), signed in with `gh auth login`. Alternatively, enter a
  personal access token in Settings.
- [Claude Code](https://claude.com/claude-code) (`claude`) for Discuss, the AI summary and the
  pre-review. The rest of the app works without it.

Claude Code runs locally under your own account. Review Buddy has no server, and talks only to
GitHub and to the `claude` process on your Mac.

## Keyboard

| Key | Action | Key | Action |
|---|---|---|---|
| `j` / `k` | next / previous file | `c` | comment on selection |
| `n` / `p` | next / previous hunk | `d` | discuss selection with Claude |
| `]` / `[` | next / previous thread | `v` | toggle viewed |
| `s` | split / unified | `x` | collapse file |
| `⌘R` | refresh the PR | `⌘L` | open PR by URL |
| `⌘K` | command palette | `⌘\` | toggle Discuss panel |
| `⌃⌘S` | toggle inbox sidebar | `⌘↩` / `⇧⌘↩` | submit to review / single comment |

## Build from source

Requires a Rust toolchain, 1.92 or later (`rustup update`).

```sh
Scripts/bundle.sh release            # → build/ReviewBuddy.app
open build/ReviewBuddy.app
open build/ReviewBuddy.app --args --open https://github.com/owner/repo/pull/123
cargo run --release -- --open owner/repo#123   # or straight from the workspace
```

Try it without a GitHub account or network, using a generated in-memory PR:

```sh
cargo run --release -- --synthetic small   # or large / huge (2,000 files)
```

Run the tests:

```sh
cargo test --workspace         # core + app (store) tests
Scripts/perf-check.sh          # scripted UI performance run against budgets
```

Scripted UI checks render PNGs without screen-recording permission:

```sh
cargo run -- --synthetic small --snap-dir /tmp/rb \
  --ui-script "selectfirst;comment;typecomment:Hello;snap:comment;split;snap:split;quit"
```

The app icon is drawn by `Scripts/make-icon.swift`, which writes `Resources/AppIcon.icns`
(a one-off asset tool; the `.icns` is committed, so building the app doesn't need Swift).

## Project layout

- `crates/review-buddy-core`: diff parsing and layout, syntax highlighting, the GitHub client,
  git worktrees, the Claude stream-json session, Markdown parsing, and persistence. No GUI code.
- `crates/review-buddy`: the app. `pr_store.rs` / `pr_store_ai.rs` (state and actions for one
  PR), `discuss.rs` (a Claude conversation), `app.rs` (inbox and open PRs), and `ui/` (the egui
  views, including the virtualized diff canvas in `ui/diff_canvas.rs`).
