import Foundation

/// Every keyboard-reachable action. The diff view maps keys → actions through
/// `KeyBindings`, and the command palette lists the same actions.
public enum ReviewAction: String, CaseIterable, Sendable, Identifiable {
    case nextFile, previousFile, nextHunk, previousHunk, nextThread, previousThread
    case toggleViewed, comment, discuss, reply, toggleSplit, openByURL, commandPalette
    case submit, toggleDiscussPanel, cancel, summarize, preReview, refresh, pushFixes
    case expandAllThreads, collapseFile

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .nextFile: "Next file"
        case .previousFile: "Previous file"
        case .nextHunk: "Next hunk"
        case .previousHunk: "Previous hunk"
        case .nextThread: "Next thread"
        case .previousThread: "Previous thread"
        case .toggleViewed: "Toggle viewed"
        case .comment: "Comment on selection"
        case .discuss: "Discuss selection with Claude"
        case .reply: "Reply to thread"
        case .toggleSplit: "Toggle split / unified"
        case .openByURL: "Open pull request by URL…"
        case .commandPalette: "Command palette"
        case .submit: "Submit"
        case .toggleDiscussPanel: "Toggle Discuss panel"
        case .cancel: "Cancel"
        case .summarize: "Summarize pull request"
        case .preReview: "Run AI pre-review"
        case .refresh: "Refresh"
        case .pushFixes: "Push fix commits…"
        case .expandAllThreads: "Expand all threads"
        case .collapseFile: "Collapse / expand file"
        }
    }

    /// Display string for menus and the palette.
    public var shortcut: String? {
        switch self {
        case .nextFile: "J"
        case .previousFile: "K"
        case .nextHunk: "N"
        case .previousHunk: "P"
        case .nextThread: "]"
        case .previousThread: "["
        case .toggleViewed: "V"
        case .comment: "C"
        case .discuss: "D"
        case .reply: "R"
        case .toggleSplit: "S"
        case .openByURL: "⌘L"
        case .commandPalette: "⌘K"
        case .submit: "⌘↩"
        case .toggleDiscussPanel: "⌘\\"
        case .cancel: "Esc"
        case .refresh: "⌘R"
        case .collapseFile: "X"
        default: nil
        }
    }
}

public struct KeyStroke: Hashable, Sendable {
    public var key: String
    public var command: Bool
    public var shift: Bool
    public var option: Bool
    public var control: Bool
    public init(_ key: String, command: Bool = false, shift: Bool = false, option: Bool = false, control: Bool = false) {
        self.key = key.lowercased(); self.command = command; self.shift = shift; self.option = option; self.control = control
    }
}

public enum KeyBindings {
    static let table: [KeyStroke: ReviewAction] = [
        KeyStroke("j"): .nextFile, KeyStroke("k"): .previousFile,
        KeyStroke("n"): .nextHunk, KeyStroke("p"): .previousHunk,
        KeyStroke("]"): .nextThread, KeyStroke("["): .previousThread,
        KeyStroke("v"): .toggleViewed, KeyStroke("c"): .comment, KeyStroke("d"): .discuss,
        KeyStroke("r"): .reply, KeyStroke("s"): .toggleSplit, KeyStroke("x"): .collapseFile,
        KeyStroke("l", command: true): .openByURL, KeyStroke("k", command: true): .commandPalette,
        KeyStroke("\r", command: true): .submit, KeyStroke("\\", command: true): .toggleDiscussPanel,
        KeyStroke("\u{1b}"): .cancel, KeyStroke("r", command: true): .refresh,
    ]

    /// Single-key shortcuts only fire when no text field has focus.
    public static func action(for stroke: KeyStroke, textInputFocused: Bool) -> ReviewAction? {
        guard let a = table[stroke] else { return nil }
        let isPlain = !stroke.command && !stroke.control && !stroke.option
        if textInputFocused && isPlain && a != .cancel { return nil }
        return a
    }

    /// Fuzzy subsequence score for the command palette (higher is better, nil = no match).
    public static func fuzzyScore(_ query: String, _ candidate: String) -> Int? {
        let q = Array(query.lowercased()), c = Array(candidate.lowercased())
        if q.isEmpty { return 0 }
        var score = 0, qi = 0, last = -2, ci = 0
        while ci < c.count && qi < q.count {
            if c[ci] == q[qi] {
                score += (ci == last + 1) ? 5 : 1
                if ci == 0 || c[ci - 1] == " " || c[ci - 1] == "/" || c[ci - 1] == "." { score += 4 }
                last = ci
                qi += 1
            }
            ci += 1
        }
        guard qi == q.count else { return nil }
        return score * 100 - c.count
    }
}
