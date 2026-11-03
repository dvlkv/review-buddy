import Foundation

/// Block-level GitHub-flavored Markdown for comment bodies and Claude's replies. Inline
/// formatting (`code`, **bold**, links) is left to `AttributedString(markdown:)` in the UI;
/// this handles structure: headings, lists (nested, task), quotes, tables, code fences
/// (including ```suggestion blocks, which GitHub renders as a mini diff) and rules.
///
/// It must never fail: text streams in a token at a time, so half a table or an
/// unterminated fence has to render as something sensible.
public enum MarkdownBlock: Equatable, Sendable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case code(language: String?, text: String)
    case suggestion(String)
    case quote([MarkdownBlock])
    case list(MarkdownList)
    case table(MarkdownTable)
    case rule
}

public struct MarkdownList: Equatable, Sendable {
    public var ordered: Bool
    /// Number of the first item (ordered lists).
    public var start: Int
    public var items: [MarkdownListItem]

    public init(ordered: Bool, start: Int = 1, items: [MarkdownListItem]) {
        self.ordered = ordered
        self.start = start
        self.items = items
    }
}

public struct MarkdownListItem: Equatable, Sendable {
    /// nil for a plain item; the checkbox state for a task item (`- [x] …`).
    public var checked: Bool?
    public var blocks: [MarkdownBlock]

    public init(checked: Bool? = nil, blocks: [MarkdownBlock]) {
        self.checked = checked
        self.blocks = blocks
    }
}

public struct MarkdownTable: Equatable, Sendable {
    public enum Alignment: Equatable, Sendable { case none, left, center, right }
    public var header: [String]
    public var alignments: [Alignment]
    /// Every row has exactly `header.count` cells (short rows padded, long ones cut).
    public var rows: [[String]]

    public init(header: [String], alignments: [Alignment], rows: [[String]]) {
        self.header = header
        self.alignments = alignments
        self.rows = rows
    }

    public var columnCount: Int { header.count }
}

public enum MarkdownParser {
    /// Nesting deeper than this (quotes in lists in quotes…) is shown as plain text.
    static let maxDepth = 16

    public static func parse(_ text: String) -> [MarkdownBlock] {
        let normalized = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        if normalized.isEmpty { return [] }
        return parseBlocks(normalized.components(separatedBy: "\n").map(expandLeadingTabs), depth: 0)
    }

    // MARK: Blocks

    static func parseBlocks(_ lines: [String], depth: Int) -> [MarkdownBlock] {
        if depth > maxDepth {
            let text = lines.map { $0.trimmingCharacters(in: .whitespaces) }.joined(separator: "\n")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return text.isEmpty ? [] : [.paragraph(text)]
        }
        var blocks: [MarkdownBlock] = []
        var para: [String] = []
        func flushPara() {
            if !para.isEmpty { blocks.append(.paragraph(para.joined(separator: "\n"))); para.removeAll() }
        }
        var i = 0
        let n = lines.count
        while i < n {
            let line = lines[i]
            let indent = leadingSpaces(line)
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.isEmpty { flushPara(); i += 1; continue }

            if let fence = openingFence(t) {
                flushPara()
                var body: [String] = []
                i += 1
                while i < n, !isClosingFence(lines[i], fence) {
                    body.append(dropSpaces(lines[i], upTo: indent))
                    i += 1
                }
                i += 1 // closing fence (or past the end when the fence is still streaming)
                let text = body.joined(separator: "\n")
                if fence.language == "suggestion" { blocks.append(.suggestion(text)) }
                else { blocks.append(.code(language: fence.language, text: text)) }
                continue
            }
            if t.hasPrefix("<!--") {
                // HTML comments (PR templates) are invisible on GitHub; an unclosed one hides the rest.
                flushPara()
                while i < n, !lines[i].contains("-->") { i += 1 }
                i += 1
                continue
            }
            if let h = heading(t) { flushPara(); blocks.append(h); i += 1; continue }
            if !para.isEmpty, let level = setextLevel(t) {
                blocks.append(.heading(level: level, text: para.joined(separator: " ")))
                para.removeAll()
                i += 1
                continue
            }
            if isRule(t) { flushPara(); blocks.append(.rule); i += 1; continue }
            if t.hasPrefix(">") {
                flushPara()
                var inner: [String] = []
                var lastBlank = false
                while i < n {
                    let qt = lines[i].trimmingCharacters(in: .whitespaces)
                    if qt.hasPrefix(">") {
                        var rest = qt.dropFirst()
                        if rest.first == " " { rest = rest.dropFirst() }
                        inner.append(String(rest))
                        lastBlank = rest.trimmingCharacters(in: .whitespaces).isEmpty
                    } else if !qt.isEmpty, !lastBlank, !startsBlock(lines[i]) {
                        inner.append(qt) // lazy continuation of the quoted paragraph
                    } else {
                        break
                    }
                    i += 1
                }
                blocks.append(.quote(parseBlocks(inner, depth: depth + 1)))
                continue
            }
            if i + 1 < n, t.contains("|"), let aligns = delimiterRow(lines[i + 1]) {
                let header = splitCells(t)
                if header.count == aligns.count {
                    flushPara()
                    i += 2
                    var rows: [[String]] = []
                    while i < n {
                        let rt = lines[i].trimmingCharacters(in: .whitespaces)
                        guard !rt.isEmpty, rt.contains("|"), !startsBlock(lines[i]) else { break }
                        var cells = splitCells(rt)
                        if cells.count < header.count { cells += Array(repeating: "", count: header.count - cells.count) }
                        rows.append(Array(cells.prefix(header.count)))
                        i += 1
                    }
                    blocks.append(.table(MarkdownTable(header: header, alignments: aligns, rows: rows)))
                    continue
                }
            }
            if let m = listMarker(line), para.isEmpty || m.canInterruptParagraph {
                flushPara()
                blocks.append(.list(parseList(lines, &i, depth: depth)))
                continue
            }
            para.append(t)
            i += 1
        }
        flushPara()
        return blocks
    }

    /// Consumes one list starting at `lines[i]`.
    static func parseList(_ lines: [String], _ i: inout Int, depth: Int) -> MarkdownList {
        let first = listMarker(lines[i])!
        var items: [MarkdownListItem] = []
        var cur: [String] = [first.content]
        var contentCol = first.contentColumn
        var sawBlank = false
        func finishItem() {
            while cur.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { cur.removeLast() }
            var checked: Bool?
            if let head = cur.first {
                for (box, state) in [("[ ]", false), ("[x]", true), ("[X]", true)] where head == box || head.hasPrefix(box + " ") {
                    checked = state
                    cur[0] = String(head.dropFirst(box.count)).trimmingCharacters(in: .whitespaces)
                    break
                }
            }
            items.append(MarkdownListItem(checked: checked, blocks: parseBlocks(cur, depth: depth + 1)))
        }
        i += 1
        while i < lines.count {
            let line = lines[i]
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.isEmpty { cur.append(""); sawBlank = true; i += 1; continue }
            let ind = leadingSpaces(line)
            if ind <= first.indent + 1 {
                if let m = listMarker(line), !isRule(t) {
                    guard m.ordered == first.ordered else { break }
                    finishItem()
                    cur = [m.content]
                    contentCol = m.contentColumn
                    sawBlank = false
                    i += 1
                    continue
                }
                // Not indented under the item: a lazy paragraph continuation, unless a blank
                // line or another block ends the list.
                if sawBlank || startsBlock(line) || isTableStart(lines, i) { break }
                cur.append(t)
                i += 1
                continue
            }
            // Indented under the item (nested list, code, continuation paragraph). Models
            // nest with 2, 3 or 4 spaces regardless of the marker width, so be lenient.
            cur.append(dropSpaces(line, upTo: min(ind, contentCol)))
            sawBlank = false
            i += 1
        }
        finishItem()
        return MarkdownList(ordered: first.ordered, start: first.start, items: items)
    }

    // MARK: Line classifiers

    struct Fence { var char: Character; var count: Int; var language: String? }

    static func openingFence(_ t: String) -> Fence? {
        guard let c = t.first, c == "`" || c == "~" else { return nil }
        let count = t.prefix(while: { $0 == c }).count
        guard count >= 3 else { return nil }
        let info = t.dropFirst(count).trimmingCharacters(in: .whitespaces)
        if c == "`", info.contains("`") { return nil } // inline code like ```x```
        let lang = info.split(separator: " ").first.map(String.init)
        return Fence(char: c, count: count, language: lang?.isEmpty == false ? lang : nil)
    }

    static func isClosingFence(_ line: String, _ f: Fence) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        return t.count >= f.count && t.allSatisfy { $0 == f.char }
    }

    static func heading(_ s: String) -> MarkdownBlock? {
        let level = s.prefix(while: { $0 == "#" }).count
        guard (1...6).contains(level) else { return nil }
        let rest = s.dropFirst(level)
        guard rest.isEmpty || rest.first == " " else { return nil }
        var text = rest.trimmingCharacters(in: .whitespaces)
        // Optional closing sequence: "## Title ##".
        if let r = text.range(of: #"(^|\s)#+$"#, options: .regularExpression) {
            text = String(text[..<r.lowerBound]).trimmingCharacters(in: .whitespaces)
        }
        return .heading(level: level, text: text)
    }

    static func setextLevel(_ t: String) -> Int? {
        guard t.count >= 3 else { return nil }
        if t.allSatisfy({ $0 == "=" }) { return 1 }
        if t.allSatisfy({ $0 == "-" }) { return 2 }
        return nil
    }

    static func isRule(_ t: String) -> Bool {
        let chars = t.filter { $0 != " " }
        guard chars.count >= 3, let c = chars.first, c == "-" || c == "*" || c == "_" else { return false }
        return chars.allSatisfy { $0 == c }
    }

    struct ListMarker {
        var indent: Int
        var ordered: Bool
        var start: Int
        var contentColumn: Int
        var content: String
        /// CommonMark: an empty item, or an ordered list not starting at 1, can't interrupt a paragraph.
        var canInterruptParagraph: Bool { !content.isEmpty && (!ordered || start == 1) }
    }

    static func listMarker(_ line: String) -> ListMarker? {
        let indent = leadingSpaces(line)
        let s = line.dropFirst(indent)
        guard let c = s.first else { return nil }
        var markerLen: Int
        var ordered = false
        var start = 1
        if c == "-" || c == "*" || c == "+" {
            markerLen = 1
        } else {
            let digits = s.prefix(while: { $0.isASCII && $0.isNumber })
            guard (1...9).contains(digits.count) else { return nil }
            let after = s.dropFirst(digits.count).first
            guard after == "." || after == ")" else { return nil }
            ordered = true
            start = Int(digits) ?? 1
            markerLen = digits.count + 1
        }
        let rest = s.dropFirst(markerLen)
        if let f = rest.first, f != " ", f != "\t" { return nil }
        let spaces = rest.first == "\t" ? 1 : rest.prefix(while: { $0 == " " }).count
        let content = rest.trimmingCharacters(in: .whitespaces)
        // 1–4 spaces after the marker set the content column; more means indented content.
        let gap = content.isEmpty || spaces > 4 ? 1 : spaces
        return ListMarker(indent: indent, ordered: ordered, start: start, contentColumn: indent + markerLen + gap,
                          content: content)
    }

    /// Whether `line` opens a block that ends a paragraph, list item or table.
    static func startsBlock(_ line: String) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        return openingFence(t) != nil || heading(t) != nil || isRule(t) || t.hasPrefix(">") || t.hasPrefix("<!--")
            || (listMarker(line)?.canInterruptParagraph ?? false)
    }

    static func isTableStart(_ lines: [String], _ i: Int) -> Bool {
        guard i + 1 < lines.count, lines[i].contains("|"), let a = delimiterRow(lines[i + 1]) else { return false }
        return splitCells(lines[i].trimmingCharacters(in: .whitespaces)).count == a.count
    }

    // MARK: Tables

    /// `| :--- | :---: | ---: |` → alignments, or nil if the line isn't a delimiter row.
    static func delimiterRow(_ line: String) -> [MarkdownTable.Alignment]? {
        let t = line.trimmingCharacters(in: .whitespaces)
        guard t.contains("|") || t.contains(":"), t.contains("-") else { return nil }
        var cells = t.split(separator: "|", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
        if t.hasPrefix("|") { cells.removeFirst() }
        if t.hasSuffix("|"), !cells.isEmpty { cells.removeLast() }
        guard !cells.isEmpty else { return nil }
        var out: [MarkdownTable.Alignment] = []
        for c in cells {
            let left = c.hasPrefix(":"), right = c.hasSuffix(":") && c.count > 1
            let dashes = c.dropFirst(left ? 1 : 0).dropLast(right ? 1 : 0)
            guard !dashes.isEmpty, dashes.allSatisfy({ $0 == "-" }) else { return nil }
            out.append(left && right ? .center : left ? .left : right ? .right : .none)
        }
        // A lone `---` is a rule or a setext underline, not a one-column table.
        guard t.contains("|") else { return nil }
        return out
    }

    /// Cells of a table row. `\|` is a literal pipe; pipes inside a closed `code span` don't
    /// split (GitHub would, but models write `a | b` in code and mean one cell).
    static func splitCells(_ row: String) -> [String] {
        var t = Substring(row.trimmingCharacters(in: .whitespaces))
        if t.hasPrefix("|") { t = t.dropFirst() }
        if t.hasSuffix("|"), !t.hasSuffix("\\|") { t = t.dropLast() }
        let chars = Array(t)
        var cells: [String] = []
        var cur = ""
        var k = 0
        while k < chars.count {
            let c = chars[k]
            if c == "\\", k + 1 < chars.count, chars[k + 1] == "|" {
                cur.append("|"); k += 2; continue
            }
            if c == "`" {
                var run = 0
                while k + run < chars.count, chars[k + run] == "`" { run += 1 }
                if let close = closingBackticks(chars, from: k + run, run: run) {
                    cur.append(contentsOf: chars[k..<(close + run)])
                    k = close + run
                } else {
                    cur.append(contentsOf: chars[k..<(k + run)])
                    k += run
                }
                continue
            }
            if c == "|" { cells.append(cur); cur = ""; k += 1; continue }
            cur.append(c)
            k += 1
        }
        cells.append(cur)
        return cells.map { cleanCell($0) }
    }

    private static func closingBackticks(_ chars: [Character], from: Int, run: Int) -> Int? {
        var k = from
        while k < chars.count {
            if chars[k] == "`" {
                var r = 0
                while k + r < chars.count, chars[k + r] == "`" { r += 1 }
                if r == run { return k }
                k += r
            } else {
                k += 1
            }
        }
        return nil
    }

    private static func cleanCell(_ s: String) -> String {
        s.replacingOccurrences(of: #"<br\s*/?>"#, with: "\n", options: [.regularExpression, .caseInsensitive])
            .trimmingCharacters(in: .whitespaces)
    }

    // MARK: Helpers

    static func leadingSpaces(_ s: String) -> Int { s.prefix(while: { $0 == " " }).count }

    static func dropSpaces(_ s: String, upTo n: Int) -> String {
        String(s.dropFirst(min(n, leadingSpaces(s))))
    }

    static func expandLeadingTabs(_ s: String) -> String {
        guard s.first == "\t" || (s.first == " " && s.contains("\t")) else { return s }
        var out = ""
        var idx = s.startIndex
        while idx < s.endIndex, s[idx] == " " || s[idx] == "\t" {
            if s[idx] == "\t" { out += String(repeating: " ", count: 4 - out.count % 4) } else { out += " " }
            idx = s.index(after: idx)
        }
        return out + s[idx...]
    }

    /// Text for a ```suggestion block replacing `lines`.
    public static func suggestionTemplate(for lines: [String]) -> String {
        "```suggestion\n" + lines.joined(separator: "\n") + "\n```"
    }
}
