import Foundation

/// Block-level GitHub-flavored Markdown for comment bodies. Inline formatting is
/// left to `AttributedString(markdown:)` in the UI; this handles structure,
/// including ```suggestion blocks which GitHub renders as a mini diff.
public enum MarkdownBlock: Equatable, Sendable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case code(language: String?, text: String)
    case suggestion(String)
    case quote(String)
    case list(ordered: Bool, items: [String])
    case taskList([(done: Bool, text: String)])
    case rule

    public static func == (a: MarkdownBlock, b: MarkdownBlock) -> Bool {
        switch (a, b) {
        case (.paragraph(let x), .paragraph(let y)): x == y
        case (.heading(let l1, let t1), .heading(let l2, let t2)): l1 == l2 && t1 == t2
        case (.code(let l1, let t1), .code(let l2, let t2)): l1 == l2 && t1 == t2
        case (.suggestion(let x), .suggestion(let y)): x == y
        case (.quote(let x), .quote(let y)): x == y
        case (.list(let o1, let i1), .list(let o2, let i2)): o1 == o2 && i1 == i2
        case (.taskList(let x), .taskList(let y)): x.map(\.done) == y.map(\.done) && x.map(\.text) == y.map(\.text)
        case (.rule, .rule): true
        default: false
        }
    }
}

public enum MarkdownParser {
    public static func parse(_ text: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var i = 0
        var para: [String] = []
        func flushPara() {
            if !para.isEmpty { blocks.append(.paragraph(para.joined(separator: "\n"))); para.removeAll() }
        }
        while i < lines.count {
            let line = lines[i]
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                flushPara()
                let fence = String(trimmed.prefix(3))
                let info = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                var body: [String] = []
                i += 1
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(fence) {
                    body.append(lines[i]); i += 1
                }
                i += 1
                let text = body.joined(separator: "\n")
                if info == "suggestion" { blocks.append(.suggestion(text)) }
                else { blocks.append(.code(language: info.isEmpty ? nil : info, text: text)) }
                continue
            }
            if trimmed.isEmpty { flushPara(); i += 1; continue }
            if let h = heading(trimmed) { flushPara(); blocks.append(h); i += 1; continue }
            if trimmed == "---" || trimmed == "***" || trimmed == "___" { flushPara(); blocks.append(.rule); i += 1; continue }
            if trimmed.hasPrefix(">") {
                flushPara()
                var q: [String] = []
                while i < lines.count, lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(">") {
                    q.append(String(lines[i].trimmingCharacters(in: .whitespaces).dropFirst()).trimmingCharacters(in: .whitespaces))
                    i += 1
                }
                blocks.append(.quote(q.joined(separator: "\n")))
                continue
            }
            if let (ordered, _) = listItem(trimmed) {
                flushPara()
                var items: [String] = []
                var tasks: [(Bool, String)] = []
                var allTasks = true
                while i < lines.count, let (o, text) = listItem(lines[i].trimmingCharacters(in: .whitespaces)), o == ordered {
                    if text.hasPrefix("[ ] ") { tasks.append((false, String(text.dropFirst(4)))) }
                    else if text.hasPrefix("[x] ") || text.hasPrefix("[X] ") { tasks.append((true, String(text.dropFirst(4)))) }
                    else { allTasks = false }
                    items.append(text)
                    i += 1
                }
                if allTasks && !tasks.isEmpty { blocks.append(.taskList(tasks.map { (done: $0.0, text: $0.1) })) }
                else { blocks.append(.list(ordered: ordered, items: items)) }
                continue
            }
            para.append(line)
            i += 1
        }
        flushPara()
        return blocks
    }

    static func heading(_ s: String) -> MarkdownBlock? {
        var level = 0
        for c in s { if c == "#" { level += 1 } else { break } }
        guard (1...6).contains(level), s.count > level, s[s.index(s.startIndex, offsetBy: level)] == " " else { return nil }
        return .heading(level: level, text: String(s.dropFirst(level + 1)))
    }

    static func listItem(_ s: String) -> (Bool, String)? {
        if s.hasPrefix("- ") || s.hasPrefix("* ") || s.hasPrefix("+ ") { return (false, String(s.dropFirst(2))) }
        var digits = 0
        for c in s { if c.isNumber { digits += 1 } else { break } }
        if digits > 0, digits < 10 {
            let rest = s.dropFirst(digits)
            if rest.hasPrefix(". ") || rest.hasPrefix(") ") { return (true, String(rest.dropFirst(2))) }
        }
        return nil
    }

    /// Text for a ```suggestion block replacing `lines`.
    public static func suggestionTemplate(for lines: [String]) -> String {
        "```suggestion\n" + lines.joined(separator: "\n") + "\n```"
    }
}
