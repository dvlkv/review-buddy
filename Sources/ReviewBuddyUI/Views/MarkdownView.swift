import SwiftUI
import ReviewBuddyCore

/// GitHub-flavored Markdown: comment bodies, PR descriptions and Claude's replies.
/// Structure comes from `MarkdownParser`; inline formatting from `MarkdownInline`.
struct MarkdownView: View {
    let text: String
    var suggestionContext: [String] = []

    var body: some View {
        MarkdownBlocksView(blocks: MarkdownParser.parse(text), style: MarkdownStyle(suggestionContext: suggestionContext))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct MarkdownStyle {
    var suggestionContext: [String] = []
    var fontSize: CGFloat = 13
    /// List nesting, for the bullet glyph.
    var listDepth = 0
}

struct MarkdownBlocksView: View {
    let blocks: [MarkdownBlock]
    let style: MarkdownStyle
    var spacing: CGFloat = 8

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            ForEach(blocks.indices, id: \.self) { i in
                MarkdownBlockView(block: blocks[i], style: style)
            }
        }
    }
}

struct MarkdownBlockView: View {
    let block: MarkdownBlock
    let style: MarkdownStyle

    var body: some View {
        switch block {
        case .paragraph(let s):
            Text(MarkdownInline.attributed(s))
                .font(.system(size: style.fontSize))
                .foregroundStyle(Color.ghText)
                .fixedSize(horizontal: false, vertical: true)
        case .heading(let level, let s):
            VStack(alignment: .leading, spacing: 4) {
                Text(MarkdownInline.attributed(s))
                    .font(.system(size: Self.headingSize(level, base: style.fontSize), weight: .semibold))
                    .foregroundStyle(level >= 6 ? Color.ghMuted : Color.ghText)
                    .fixedSize(horizontal: false, vertical: true)
                if level <= 2 { Rectangle().fill(Color.ghBorder).frame(height: 1) }
            }
            .padding(.top, level <= 2 ? 4 : 2)
        case .code(let lang, let s):
            MarkdownCodeBlock(language: lang, text: s)
        case .suggestion(let s):
            SuggestionPreview(old: style.suggestionContext, new: s.components(separatedBy: "\n"))
        case .quote(let blocks):
            MarkdownBlocksView(blocks: blocks, style: style, spacing: 6)
                .foregroundStyle(Color.ghMuted)
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Rectangle().fill(Color.ghBorder).frame(width: 3)
                }
        case .list(let l):
            MarkdownListView(list: l, style: style)
        case .table(let t):
            MarkdownTableView(table: t, fontSize: style.fontSize - 0.5)
        case .rule:
            Rectangle().fill(Color.ghBorder).frame(height: 2).padding(.vertical, 2)
        }
    }

    static func headingSize(_ level: Int, base: CGFloat) -> CGFloat {
        switch level {
        case 1: base + 7
        case 2: base + 4
        case 3: base + 2
        case 4: base + 1
        default: base
        }
    }
}

struct MarkdownListView: View {
    let list: MarkdownList
    let style: MarkdownStyle

    var body: some View {
        var inner = style
        inner.listDepth += 1
        // Wide enough for the longest number so item text lines up.
        let last = list.start + max(0, list.items.count - 1)
        let markerWidth: CGFloat = list.ordered ? CGFloat(String(last).count + 1) * style.fontSize * 0.62 : 10
        return VStack(alignment: .leading, spacing: 4) {
            ForEach(list.items.indices, id: \.self) { i in
                let item = list.items[i]
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if let checked = item.checked {
                        Image(systemName: checked ? "checkmark.square.fill" : "square")
                            .foregroundStyle(checked ? Color.ghAccent : Color.ghMuted)
                            .font(.system(size: style.fontSize - 1))
                    } else {
                        Text(list.ordered ? "\(list.start + i)." : bullet)
                            .font(.system(size: style.fontSize).monospacedDigit())
                            .foregroundStyle(Color.ghMuted)
                            .frame(minWidth: markerWidth, alignment: .trailing)
                    }
                    if item.blocks.isEmpty {
                        Text(" ").font(.system(size: style.fontSize))
                    } else {
                        MarkdownBlocksView(blocks: item.blocks, style: inner, spacing: 4)
                    }
                }
            }
        }
    }

    var bullet: String { ["•", "◦", "▪︎"][style.listDepth % 3] }
}

// MARK: - Tables

struct MarkdownTableView: View {
    let table: MarkdownTable
    var fontSize: CGFloat = 12.5

    private struct Cell { var row: Int; var col: Int; var text: String }

    private var cells: [Cell] {
        var out: [Cell] = []
        for (c, h) in table.header.enumerated() { out.append(Cell(row: 0, col: c, text: h)) }
        for (r, row) in table.rows.enumerated() {
            for (c, v) in row.enumerated() { out.append(Cell(row: r + 1, col: c, text: v)) }
        }
        return out
    }

    /// Width the table may use; measured outside the scroll view, which proposes no width.
    @State private var available: CGFloat?

    var body: some View {
        ScrollView(.horizontal) {
            MarkdownTableLayout(columns: max(1, table.columnCount), floors: wordFloors, available: available) {
                ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                    cellView(cell)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(Color.ghBorder, lineWidth: 1))
        }
        .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
        .frame(maxWidth: .infinity, alignment: .leading)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { available = $0 }
    }

    /// Per column, the width of its longest unbreakable run (plus cell padding), so columns
    /// get squeezed by wrapping between words, never by breaking "REFERRAL_BOUND" mid-word.
    private var wordFloors: [CGFloat] {
        let n = max(1, table.columnCount)
        var out = [CGFloat](repeating: 0, count: n)
        let regular = NSFont.systemFont(ofSize: fontSize)
        let bold = NSFont.systemFont(ofSize: fontSize, weight: .semibold)
        let mono = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        for cell in cells where cell.col < n {
            for word in MarkdownTableLayout.unbreakableRuns(cell.text) {
                let font = word.contains("`") ? mono : cell.row == 0 ? bold : regular
                let w = (word.replacingOccurrences(of: "`", with: "").replacingOccurrences(of: "*", with: "") as NSString)
                    .size(withAttributes: [.font: font]).width
                // 16 padding + slack: SwiftUI's line breaker wants a few points more than the raw width.
                out[cell.col] = max(out[cell.col], ceil(w * 1.04) + 22)
            }
        }
        return out
    }

    private func cellView(_ cell: Cell) -> some View {
        let align = cell.col < table.alignments.count ? table.alignments[cell.col] : .none
        let header = cell.row == 0
        return Text(MarkdownInline.attributed(cell.text))
            .font(.system(size: fontSize, weight: header ? .semibold : .regular))
            .foregroundStyle(Color.ghText)
            .multilineTextAlignment(align.textAlignment)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: align.frameAlignment)
            .background(header || cell.row % 2 == 0 ? Color.ghSubtle : Color.clear)
            .border(Color.ghBorder, width: 0.5)
    }
}

extension MarkdownTable.Alignment {
    var textAlignment: TextAlignment {
        switch self {
        case .center: .center
        case .right: .trailing
        default: .leading
        }
    }

    var frameAlignment: Alignment {
        switch self {
        case .center: .center
        case .right: .trailing
        default: .leading
        }
    }
}

/// Lays cells out row-major in `columns` columns, like a browser table: columns get their
/// natural width when everything fits, otherwise each keeps a small minimum and the rest
/// of the width goes to the columns with the most text, whose cells then wrap.
struct MarkdownTableLayout: Layout {
    let columns: Int
    /// Width each column would like to keep (its longest word), clamped to minColumn…maxFloor.
    var floors: [CGFloat] = []
    var minColumn: CGFloat = 40
    var maxFloor: CGFloat = 200
    /// Width to fit into; overrides the proposal (inside a horizontal scroll view it's nil).
    var available: CGFloat?

    struct Metrics { var widths: [CGFloat]; var heights: [CGFloat] }

    struct Cache {
        var ideal: [CGFloat]?
        var byWidth: [CGFloat: Metrics] = [:]
    }

    func makeCache(subviews: Subviews) -> Cache { Cache() }
    func updateCache(_ cache: inout Cache, subviews: Subviews) { cache = Cache() }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) -> CGSize {
        let m = metrics(width: available ?? proposal.width, subviews: subviews, cache: &cache)
        return CGSize(width: m.widths.reduce(0, +), height: m.heights.reduce(0, +))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) {
        let m = metrics(width: available ?? proposal.width ?? bounds.width, subviews: subviews, cache: &cache)
        var y = bounds.minY
        for r in m.heights.indices {
            var x = bounds.minX
            for c in 0..<columns {
                let i = r * columns + c
                guard i < subviews.count else { break }
                subviews[i].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(width: m.widths[c], height: m.heights[r]))
                x += m.widths[c]
            }
            y += m.heights[r]
        }
    }

    func metrics(width: CGFloat?, subviews: Subviews, cache: inout Cache) -> Metrics {
        let key = width ?? -1
        if let m = cache.byWidth[key] { return m }
        let ideal = cache.ideal ?? idealWidths(subviews)
        cache.ideal = ideal
        let wanted = (0..<columns).map { c in min(maxFloor, max(minColumn, c < floors.count ? floors[c] : 0)) }
        let widths = Self.distribute(ideal: ideal, available: width, floors: wanted)
        let rowCount = (subviews.count + columns - 1) / columns
        var heights = [CGFloat](repeating: 0, count: rowCount)
        for (i, s) in subviews.enumerated() {
            let h = s.sizeThatFits(ProposedViewSize(width: widths[i % columns], height: nil)).height
            heights[i / columns] = max(heights[i / columns], h.isFinite ? h : 0)
        }
        let m = Metrics(widths: widths, heights: heights)
        cache.byWidth[key] = m
        return m
    }

    /// Runs of text a line can't break inside: split at whitespace, and after `/` and `-`
    /// where Text also breaks.
    static func unbreakableRuns(_ s: String) -> [Substring] {
        var out: [Substring] = []
        var start = s.startIndex
        var i = s.startIndex
        while i < s.endIndex {
            let c = s[i]
            if c == " " || c == "\n" {
                if start < i { out.append(s[start..<i]) }
                start = s.index(after: i)
            } else if c == "/" || c == "-" {
                out.append(s[start...i])
                start = s.index(after: i)
            }
            i = s.index(after: i)
        }
        if start < s.endIndex { out.append(s[start...]) }
        return out
    }

    private func idealWidths(_ subviews: Subviews) -> [CGFloat] {
        var ideal = [CGFloat](repeating: 0, count: columns)
        for (i, s) in subviews.enumerated() {
            let w = s.sizeThatFits(.unspecified).width
            ideal[i % columns] = max(ideal[i % columns], w.isFinite ? w : 0)
        }
        return ideal.map { ceil($0) }
    }

    /// Column widths for `available` points (nil: unconstrained).
    static func distribute(ideal: [CGFloat], available: CGFloat?, floors wanted: [CGFloat]) -> [CGFloat] {
        let total = ideal.reduce(0, +)
        guard let available, available.isFinite, available > 0, total > available else { return ideal }
        let floors = ideal.indices.map { min(ideal[$0], $0 < wanted.count ? wanted[$0] : 0) }
        let floorTotal = floors.reduce(0, +)
        // Even the longest words don't fit: keep them whole and let the table scroll.
        if floorTotal >= available { return floors }
        let extra = available - floorTotal
        let want = zip(ideal, floors).map { $0 - $1 }
        let wantTotal = want.reduce(0, +)
        return zip(floors, want).map { $0 + (wantTotal > 0 ? extra * $1 / wantTotal : 0) }
    }
}

// MARK: - Code

struct MarkdownCodeBlock: View {
    let language: String?
    let text: String
    @State private var hovering = false
    @State private var copied = false

    var body: some View {
        ScrollView(.horizontal) {
            Text(MarkdownInline.highlighted(text, language: language))
                .font(.system(size: 12, design: .monospaced))
                .fixedSize(horizontal: true, vertical: true)
                .padding(10)
                .padding(.trailing, language == nil ? 0 : 24)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.ghSubtle, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.ghBorder.opacity(0.6), lineWidth: 0.5))
        .overlay(alignment: .topTrailing) {
            HStack(spacing: 6) {
                if let language, !hovering {
                    Text(language).font(.system(size: 10)).foregroundStyle(Color.ghMuted)
                }
                if hovering {
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(text, forType: .string)
                        copied = true
                    } label: {
                        Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 11))
                    }
                    .buttonStyle(.borderless)
                    .help("Copy")
                }
            }
            .padding(6)
        }
        .onHover { hovering = $0; if !$0 { copied = false } }
    }
}

// MARK: - Inline

@MainActor
enum MarkdownInline {
    private final class Box { let value: AttributedString; init(_ v: AttributedString) { value = v } }
    private static let inlineCache: NSCache<NSString, Box> = { let c = NSCache<NSString, Box>(); c.countLimit = 2000; return c }()
    private static let codeCache: NSCache<NSString, Box> = { let c = NSCache<NSString, Box>(); c.countLimit = 200; return c }()
    private static let linkDetector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// Inline Markdown (`code`, **bold**, *italic*, ~~strike~~, links, bare URLs);
    /// plain text if it doesn't parse. Cached: streaming re-renders every earlier block.
    static func attributed(_ s: String) -> AttributedString {
        if let hit = inlineCache.object(forKey: s as NSString) { return hit.value }
        var a = (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
        for run in a.runs where run.inlinePresentationIntent?.contains(.code) == true {
            a[run.range].backgroundColor = Color.ghMuted.opacity(0.14)
        }
        autolink(&a)
        inlineCache.setObject(Box(a), forKey: s as NSString)
        return a
    }

    private static func autolink(_ a: inout AttributedString) {
        let plain = String(a.characters)
        guard plain.contains("://") || plain.contains("www."), let detector = linkDetector else { return }
        let ns = plain as NSString
        for m in detector.matches(in: plain, range: NSRange(location: 0, length: ns.length)) {
            guard let url = m.url, let r = Range(m.range, in: plain),
                  let lo = AttributedString.Index(r.lowerBound, within: a),
                  let hi = AttributedString.Index(r.upperBound, within: a) else { continue }
            let range = lo..<hi
            let taken = a[range].runs.contains { $0.link != nil || $0.inlinePresentationIntent?.contains(.code) == true }
            if !taken { a[range].link = url }
        }
    }

    /// Code block text with syntax colors from the diff highlighter's lexers.
    static func highlighted(_ text: String, language: String?) -> AttributedString {
        let key = "\(language ?? "")\u{0}\(text)" as NSString
        if let hit = codeCache.object(forKey: key) { return hit.value }
        let out = buildHighlighted(text, language: language?.lowercased())
        codeCache.setObject(Box(out), forKey: key)
        return out
    }

    static let languageAliases: [String: String] = [
        "typescript": "ts", "javascript": "js", "python": "py", "bash": "sh", "shell": "sh", "zsh": "sh",
        "console": "sh", "shellsession": "sh", "golang": "go", "rust": "rs", "ruby": "rb", "kotlin": "kt",
        "c++": "cpp", "csharp": "cs", "c#": "cs", "objc": "m", "objective-c": "m", "yaml": "yml", "markdown": "md",
    ]

    private static func buildHighlighted(_ text: String, language: String?) -> AttributedString {
        guard let language, text.utf16.count < 40_000 else { return AttributedString(text) }
        if language == "diff" || language == "patch" { return diffColored(text) }
        let ext = languageAliases[language] ?? language
        let spec = LanguageRegistry.language(forPath: "snippet.\(ext)")
        guard spec.name != LanguageSpec.plain.name else { return AttributedString(text) }
        let lexer = Lexer(spec: spec)
        var state = LexState.normal
        var out = AttributedString()
        for (n, line) in text.components(separatedBy: "\n").enumerated() {
            if n > 0 { out += AttributedString("\n") }
            let u = Array(line.utf16)
            var pos = 0
            for t in lexer.tokenize(utf16: u, state: &state) {
                let start = min(Int(t.start), u.count), end = min(start + Int(t.length), u.count)
                if start > pos { out += AttributedString(String(decoding: u[pos..<start], as: UTF16.self)) }
                var seg = AttributedString(String(decoding: u[start..<end], as: UTF16.self))
                if t.kind != .plain { seg.foregroundColor = Color(nsColor: Theme.syntax(t.kind)) }
                out += seg
                pos = max(pos, end)
            }
            if pos < u.count { out += AttributedString(String(decoding: u[pos...], as: UTF16.self)) }
        }
        return out
    }

    private static func diffColored(_ text: String) -> AttributedString {
        var out = AttributedString()
        for (n, line) in text.components(separatedBy: "\n").enumerated() {
            if n > 0 { out += AttributedString("\n") }
            var seg = AttributedString(line)
            if line.hasPrefix("+") && !line.hasPrefix("+++") { seg.foregroundColor = Color.ghAdd }
            else if line.hasPrefix("-") && !line.hasPrefix("---") { seg.foregroundColor = Color.ghDel }
            else if line.hasPrefix("@@") { seg.foregroundColor = Color.ghMuted }
            out += seg
        }
        return out
    }
}
