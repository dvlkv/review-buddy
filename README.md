# Review Buddy

A native macOS app for reviewing GitHub pull requests. It looks and works like GitHub's
"Files changed" view. When you select lines, you can either **Comment** (a normal GitHub
review comment) or **Discuss** (a private conversation with a Claude agent working in a
checkout of the PR, which can also fix the code and commit it to the PR branch).

See [SPEC.md](SPEC.md) for the design and [PERF.md](PERF.md) for the performance work.

## Requirements

- macOS 26, Xcode 26 / Swift 6.2
- [GitHub CLI](https://cli.github.com) signed in (`gh auth login`), or a personal access
  token entered in Settings
- [Claude Code](https://claude.com/claude-code) (`claude`) for Discuss, summaries and the
  AI pre-review

## Build and run

```sh
Scripts/bundle.sh release            # → build/ReviewBuddy.app
open build/ReviewBuddy.app
open build/ReviewBuddy.app --args --open https://github.com/owner/repo/pull/123
```

Without a network or GitHub account:

```sh
build/ReviewBuddy.app/Contents/MacOS/ReviewBuddy --synthetic huge   # 2,000-file in-memory PR
```

## Tests

```sh
swift test                 # Core + UI tests
Scripts/perf-check.sh      # scripted UI performance run against budgets
```

## Keyboard

| Key | Action | Key | Action |
|---|---|---|---|
| `j` / `k` | next / previous file | `c` | comment on selection |
| `n` / `p` | next / previous hunk | `d` | discuss selection with Claude |
| `]` / `[` | next / previous thread | `v` | toggle viewed |
| `s` | split / unified | `x` | collapse file |
| `⌘L` | open PR by URL | `⌘K` | command palette |
| `⌘↩` | submit | `⌘\` | toggle Discuss panel |

## Layout

- `Sources/ReviewBuddyCore`: diff parsing and layout, highlighting, GitHub client, git
  worktrees, the Claude stream-json session, persistence. No AppKit.
- `Sources/ReviewBuddyUI`: stores (`PRStore`, `DiscussController`, `InboxStore`), the diff
  canvas, and SwiftUI views.
- `Sources/ReviewBuddy`: the app entry point.
