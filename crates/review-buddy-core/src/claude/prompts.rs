//! All text sent to Claude lives here so it is reviewable and testable.

use crate::models::{CodeAnchor, DiffSide, PRDetail, ReviewThread};

fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn system_prompt(pr: &PRDetail) -> String {
    let n = pr.pr_ref.number;
    format!(
        "You are helping a code reviewer inside Review Buddy, a desktop app for reviewing GitHub pull requests.\n\
         The current working directory is a git worktree checked out at the head of {r} \
         (\"{title}\"), branch `{head}` → `{base}`.\n\
         The base of the PR is `refs/review-buddy/base-{n}` and the PR head as published on GitHub is \
         `refs/review-buddy/head-{n}`; run `git diff {range}` to see the PR diff. \
         HEAD may additionally contain local fix commits made in this session.\n\
         Be concise and concrete: reference files and line numbers, quote the code you mean, and say \
         clearly when something is a real bug versus a style preference.\n\
         Do not push to any remote. Only commit when explicitly asked to fix something.",
        r = pr.pr_ref,
        title = pr.title,
        head = pr.head_ref_name,
        base = pr.base_ref_name,
        range = diff_range(pr),
    )
}

/// Wraps the reviewer's first question with the code context they selected.
#[allow(clippy::too_many_arguments)]
pub fn discuss_context(
    pr: &PRDetail,
    anchor: &CodeAnchor,
    selected_lines: &[String],
    surrounding: Option<&str>,
    threads_on_file: &[ReviewThread],
    other_threads: &[ReviewThread],
    question: &str,
) -> String {
    let mut s = String::from("<review_context>\n");
    s += &format!("Pull request: {} — {}\n", pr.pr_ref, pr.title);
    s += &format!(
        "Base: {} ({})  Head: {} ({})\n",
        pr.base_ref_name,
        prefix(&pr.base_oid, 10),
        pr.head_ref_name,
        prefix(&pr.head_oid, 10)
    );
    let body = pr.body.trim();
    if !body.is_empty() {
        s += &format!("\nPR description:\n{}\n", prefix(body, 4000));
    }
    let side_name = if anchor.side == DiffSide::Left { "old (base) version" } else { "new (head) version" };
    s += &format!(
        "\nSelected: {} lines {}-{} of the {}:\n```\n",
        anchor.path, anchor.start_line, anchor.end_line, side_name
    );
    s += &selected_lines.join("\n");
    s += "\n```\n";
    if let Some(sur) = surrounding.filter(|s| !s.is_empty()) {
        s += &format!("\nSurrounding code (±20 lines):\n```\n{sur}\n```\n");
    }
    if !threads_on_file.is_empty() {
        s += "\nExisting review threads on this file:\n";
        for t in threads_on_file {
            s += &format!(
                "- L{}{}{}: ",
                t.anchor_line(),
                if t.is_resolved { " [resolved]" } else { "" },
                if t.is_outdated { " [outdated]" } else { "" }
            );
            let comments: Vec<String> = t
                .comments
                .iter()
                .map(|c| format!("@{}: {}", c.author, prefix(&c.body.replace('\n', " "), 400)))
                .collect();
            s += &comments.join(" / ");
            s += "\n";
        }
    }
    if !other_threads.is_empty() {
        s += "\nThreads on other files:\n";
        for t in other_threads.iter().take(30) {
            let first = t
                .comments
                .first()
                .map(|c| format!("@{}: {}", c.author, prefix(&c.body.replace('\n', " "), 120)))
                .unwrap_or_default();
            s += &format!("- {}:{} {}\n", t.path, t.anchor_line(), first);
        }
    }
    s += "</review_context>\n\n";
    s += question;
    s
}

pub const FIX_INSTRUCTION: &str = "Implement the fix we discussed above in this worktree. Keep the change minimal and focused. \
Run the relevant checks or tests if the project has them and they are quick. \
Then commit to the current branch with a descriptive message. Do not push.";

pub fn draft_comment(anchor: &CodeAnchor) -> String {
    let (p, a, b) = (&anchor.path, anchor.start_line, anchor.end_line);
    format!(
        "Draft a GitHub pull request review comment for {p} lines {a}-{b}, \
         based on our conversation so far. Write it to the PR author: direct, specific, and polite. \
         If the conversation settled on a concrete code change for exactly those lines, include it as a GitHub \
         suggestion block (```suggestion ... ```) containing the full replacement for lines {a}-{b}. \
         Output only the comment body in Markdown, with no preamble."
    )
}

/// The PR as published (excludes local fix commits).
pub fn diff_range(pr: &PRDetail) -> String {
    let n = pr.pr_ref.number;
    format!("refs/review-buddy/base-{n}...refs/review-buddy/head-{n}")
}

pub fn summary(pr: &PRDetail) -> String {
    format!(
        "Summarize pull request {r} \"{title}\" for a reviewer. Read the diff with \
         `git diff {range}` (and any files you need for context).\n\
         PR description:\n\
         {body}\n\
         \n\
         Return: a 3–6 sentence summary of what changes and why; a suggested reading order of the changed \
         files (most important first, each with a short reason); and risky areas worth careful review.",
        r = pr.pr_ref,
        title = pr.title,
        range = diff_range(pr),
        body = prefix(&pr.body, 4000),
    )
}

pub const SUMMARY_SCHEMA: &str = r#"{"type":"object","properties":{
  "summary":{"type":"string"},
  "readingOrder":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"reason":{"type":"string"}},"required":["path","reason"]}},
  "risks":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"description":{"type":"string"}},"required":["description"]}}
},"required":["summary","readingOrder","risks"]}"#;

pub fn pre_review(pr: &PRDetail) -> String {
    format!(
        "Review pull request {r} \"{title}\" as a careful senior engineer. Read the full diff with \
         `git diff {range}` and open files as needed (use `git show refs/review-buddy/head-{n}:<path>` \
         for file contents as published).\n\
         PR description:\n\
         {body}\n\
         \n\
         Report real problems: bugs, incorrect edge cases, security issues, races, performance traps, \
         missing error handling, and confusing code. Skip pure formatting. Each finding must point at lines \
         that appear in the diff: use side RIGHT with new-file line numbers for added/context lines, or LEFT \
         with old-file line numbers for removed lines. If you have a concrete replacement for exactly those \
         lines, put it in `suggestion` (the full replacement text, no fences). Return at most 25 findings.",
        r = pr.pr_ref,
        title = pr.title,
        range = diff_range(pr),
        n = pr.pr_ref.number,
        body = prefix(&pr.body, 4000),
    )
}

pub const PRE_REVIEW_SCHEMA: &str = r#"{"type":"object","properties":{"findings":{"type":"array","items":{"type":"object","properties":{
  "path":{"type":"string"},
  "side":{"type":"string","enum":["LEFT","RIGHT"]},
  "startLine":{"type":"integer"},
  "endLine":{"type":"integer"},
  "severity":{"type":"string","enum":["high","medium","low","nit"]},
  "title":{"type":"string"},
  "body":{"type":"string"},
  "suggestion":{"type":"string"}
},"required":["path","side","startLine","endLine","severity","title","body"]}}},"required":["findings"]}"#;
