import Foundation

/// All text sent to Claude lives here so it is reviewable and testable.
public enum Prompts {
    public static func systemPrompt(pr: PRDetail) -> String {
        """
        You are helping a code reviewer inside Review Buddy, a desktop app for reviewing GitHub pull requests.
        The current working directory is a git worktree checked out at the head of \(pr.ref) \
        ("\(pr.title)"), branch `\(pr.headRefName)` → `\(pr.baseRefName)`.
        The base of the PR is `refs/review-buddy/base-\(pr.ref.number)` and the PR head as published on GitHub is \
        `refs/review-buddy/head-\(pr.ref.number)`; run `git diff \(diffRange(pr))` to see the PR diff. \
        HEAD may additionally contain local fix commits made in this session.
        Be concise and concrete: reference files and line numbers, quote the code you mean, and say \
        clearly when something is a real bug versus a style preference.
        Do not push to any remote. Only commit when explicitly asked to fix something.
        """
    }

    /// Wraps the reviewer's first question with the code context they selected.
    public static func discussContext(pr: PRDetail, anchor: CodeAnchor, selectedLines: [String],
                                      surrounding: String?, threadsOnFile: [ReviewThread],
                                      otherThreads: [ReviewThread], question: String) -> String {
        var s = "<review_context>\n"
        s += "Pull request: \(pr.ref) — \(pr.title)\n"
        s += "Base: \(pr.baseRefName) (\(pr.baseOid.prefix(10)))  Head: \(pr.headRefName) (\(pr.headOid.prefix(10)))\n"
        let body = pr.body.trimmingCharacters(in: .whitespacesAndNewlines)
        if !body.isEmpty { s += "\nPR description:\n\(body.prefix(4000))\n" }
        let sideName = anchor.side == .left ? "old (base) version" : "new (head) version"
        s += "\nSelected: \(anchor.path) lines \(anchor.startLine)-\(anchor.endLine) of the \(sideName):\n```\n"
        s += selectedLines.joined(separator: "\n")
        s += "\n```\n"
        if let surrounding, !surrounding.isEmpty {
            s += "\nSurrounding code (±20 lines):\n```\n\(surrounding)\n```\n"
        }
        if !threadsOnFile.isEmpty {
            s += "\nExisting review threads on this file:\n"
            for t in threadsOnFile {
                s += "- L\(t.anchorLine)\(t.isResolved ? " [resolved]" : "")\(t.isOutdated ? " [outdated]" : ""): "
                s += t.comments.map { "@\($0.author): \($0.body.replacingOccurrences(of: "\n", with: " ").prefix(400))" }
                    .joined(separator: " / ")
                s += "\n"
            }
        }
        if !otherThreads.isEmpty {
            s += "\nThreads on other files:\n"
            for t in otherThreads.prefix(30) {
                let first = t.comments.first.map { "@\($0.author): \($0.body.replacingOccurrences(of: "\n", with: " ").prefix(120))" } ?? ""
                s += "- \(t.path):\(t.anchorLine) \(first)\n"
            }
        }
        s += "</review_context>\n\n"
        s += question
        return s
    }

    public static let fixInstruction = """
    Implement the fix we discussed above in this worktree. Keep the change minimal and focused. \
    Run the relevant checks or tests if the project has them and they are quick. \
    Then commit to the current branch with a descriptive message. Do not push.
    """

    public static func draftComment(anchor: CodeAnchor) -> String {
        """
        Draft a GitHub pull request review comment for \(anchor.path) lines \(anchor.startLine)-\(anchor.endLine), \
        based on our conversation so far. Write it to the PR author: direct, specific, and polite. \
        If the conversation settled on a concrete code change for exactly those lines, include it as a GitHub \
        suggestion block (```suggestion ... ```) containing the full replacement for lines \(anchor.startLine)-\(anchor.endLine). \
        Output only the comment body in Markdown, with no preamble.
        """
    }

    /// The PR as published (excludes local fix commits).
    public static func diffRange(_ pr: PRDetail) -> String {
        "refs/review-buddy/base-\(pr.ref.number)...refs/review-buddy/head-\(pr.ref.number)"
    }

    public static func summary(pr: PRDetail) -> String {
        """
        Summarize pull request \(pr.ref) "\(pr.title)" for a reviewer. Read the diff with \
        `git diff \(diffRange(pr))` (and any files you need for context).
        PR description:
        \(pr.body.prefix(4000))

        Return: a 3–6 sentence summary of what changes and why; a suggested reading order of the changed \
        files (most important first, each with a short reason); and risky areas worth careful review.
        """
    }

    public static let summarySchema = """
    {"type":"object","properties":{
      "summary":{"type":"string"},
      "readingOrder":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"reason":{"type":"string"}},"required":["path","reason"]}},
      "risks":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"description":{"type":"string"}},"required":["description"]}}
    },"required":["summary","readingOrder","risks"]}
    """

    public static func preReview(pr: PRDetail) -> String {
        """
        Review pull request \(pr.ref) "\(pr.title)" as a careful senior engineer. Read the full diff with \
        `git diff \(diffRange(pr))` and open files as needed (use `git show refs/review-buddy/head-\(pr.ref.number):<path>` \
        for file contents as published).
        PR description:
        \(pr.body.prefix(4000))

        Report real problems: bugs, incorrect edge cases, security issues, races, performance traps, \
        missing error handling, and confusing code. Skip pure formatting. Each finding must point at lines \
        that appear in the diff: use side RIGHT with new-file line numbers for added/context lines, or LEFT \
        with old-file line numbers for removed lines. If you have a concrete replacement for exactly those \
        lines, put it in `suggestion` (the full replacement text, no fences). Return at most 25 findings.
        """
    }

    public static let preReviewSchema = """
    {"type":"object","properties":{"findings":{"type":"array","items":{"type":"object","properties":{
      "path":{"type":"string"},
      "side":{"type":"string","enum":["LEFT","RIGHT"]},
      "startLine":{"type":"integer"},
      "endLine":{"type":"integer"},
      "severity":{"type":"string","enum":["high","medium","low","nit"]},
      "title":{"type":"string"},
      "body":{"type":"string"},
      "suggestion":{"type":"string"}
    },"required":["path","side","startLine","endLine","severity","title","body"]}}},"required":["findings"]}
    """
}
