import AppKit
import CoreText
import ReviewBuddyCore

/// Prepared, drawable line: a Core Text line plus emphasis ranges (in points).
final class RenderedLine {
    let line: CTLine
    /// Intraline "changed word" spans, x-offsets relative to the text origin.
    let emphasis: [ClosedRange<CGFloat>]
    let width: CGFloat
    init(line: CTLine, emphasis: [ClosedRange<CGFloat>], width: CGFloat) {
        self.line = line; self.emphasis = emphasis; self.width = width
    }
}

/// Syntax highlighting computed off the main thread, one file at a time, with
/// visible files prioritized. Results are immutable arrays shared with the renderer.
final class HighlightCache: @unchecked Sendable {
    private let lock = NSLock()
    private var tokens: [Int: [[SyntaxToken]]] = [:]
    private var queued: Set<Int> = []
    private var wanted: [Int] = []
    private var running = false
    private let diff: DiffSet
    /// Called on the main thread when a file's tokens are ready.
    var onReady: (@MainActor (Int) -> Void)?

    init(diff: DiffSet) { self.diff = diff }

    func tokens(file: Int) -> [[SyntaxToken]]? { lock.withLock { tokens[file] } }

    /// Highlights small files synchronously (≈0.1 ms each) so a tile never renders
    /// twice (plain, then colored). Large files stay on the background queue.
    func tokensOrHighlightNow(file: Int) -> [[SyntaxToken]]? {
        if let t = tokens(file: file) { return t }
        guard diff.files[file].lines.count <= 4000 else { request([file]); return nil }
        let t = DiffHighlighter.highlight(file: diff.files[file], in: diff)
        lock.withLock { tokens[file] = t; queued.remove(file); wanted.removeAll { $0 == file } }
        return t
    }

    /// Requests files in priority order (first = most urgent).
    func request(_ files: [Int]) {
        lock.lock()
        // Small files are highlighted on demand while rendering; only big ones go async.
        let fresh = files.filter { tokens[$0] == nil && !queued.contains($0) && diff.files[$0].lines.count > 4000 }
        // Newly requested files jump the queue: they're what the user is looking at.
        wanted.insert(contentsOf: fresh, at: 0)
        queued.formUnion(fresh)
        let start = !running && !wanted.isEmpty
        if start { running = true }
        lock.unlock()
        if start { DispatchQueue.global(qos: .userInitiated).async { self.drain() } }
    }

    private func drain() {
        while true {
            lock.lock()
            guard !wanted.isEmpty else { running = false; lock.unlock(); return }
            let f = wanted.removeFirst()
            lock.unlock()
            if lock.withLock({ tokens[f] != nil }) { lock.withLock { _ = queued.remove(f) }; continue }
            let t = Perf.shared.measure("highlight.file") { DiffHighlighter.highlight(file: diff.files[f], in: diff) }
            let isNew = lock.withLock { () -> Bool in
                defer { queued.remove(f) }
                if tokens[f] != nil { return false }
                tokens[f] = t
                return true
            }
            if isNew { DispatchQueue.main.async { MainActor.assumeIsolated { self.onReady?(f) } } }
        }
    }
}

/// Builds and caches `RenderedLine`s for diff rows.
@MainActor
final class LineRenderer {
    let diff: DiffSet
    let highlights: HighlightCache
    private var cache: [Int64: RenderedLine] = [:]
    private var cacheOrder: [Int64] = []
    private let cacheLimit = 4000
    private var partners: [Int: ContiguousArray<Int32>] = [:]
    private var expandedText: [Int: [String]] = [:]
    var metrics: DiffMetrics { DiffMetrics.shared }
    private var appearanceKey = 0

    init(diff: DiffSet, highlights: HighlightCache) {
        self.diff = diff
        self.highlights = highlights
    }

    func invalidateAll() {
        cache.removeAll(keepingCapacity: true)
        cacheOrder.removeAll(keepingCapacity: true)
    }

    func invalidate(file: Int) {
        let prefix = Int64(file) << 32
        cache = cache.filter { ($0.key & ~0xFFFF_FFFF) >> 1 != prefix >> 1 }
    }

    func setExpandedContent(file: Int, lines: [String]) { expandedText[file] = lines }
    func expandedContent(file: Int) -> [String]? { expandedText[file] }

    /// Paired line index (deletion ↔ addition) used for intraline emphasis.
    func partner(file: Int, line: Int) -> Int? {
        if partners[file] == nil {
            let f = diff.files[file]
            var map = ContiguousArray<Int32>(repeating: -1, count: f.lines.count)
            for p in SplitAligner.align(file: f) where p.left >= 0 && p.right >= 0 && p.left != p.right {
                map[Int(p.left)] = p.right
                map[Int(p.right)] = p.left
            }
            partners[file] = map
        }
        let v = partners[file]![line]
        return v >= 0 ? Int(v) : nil
    }

    private func key(file: Int, line: Int, highlighted: Bool) -> Int64 {
        Int64(file) << 32 | Int64(line) << 1 | (highlighted ? 1 : 0)
    }

    /// Appearance the CGColors are resolved for.
    var appearance: NSAppearance = NSApp?.effectiveAppearance ?? NSAppearance(named: .aqua)!

    func rendered(file: Int, line: Int) -> RenderedLine {
        let tokens = highlights.tokensOrHighlightNow(file: file)
        let k = key(file: file, line: line, highlighted: tokens != nil)
        if let r = cache[k] { return r }
        var r: RenderedLine!
        appearance.performAsCurrentDrawingAppearance {
            r = build(file: file, line: line, tokens: tokens?[line])
        }
        cache[k] = r
        cacheOrder.append(k)
        if cacheOrder.count > cacheLimit {
            for old in cacheOrder.prefix(cacheLimit / 4) { cache.removeValue(forKey: old) }
            cacheOrder.removeFirst(cacheLimit / 4)
        }
        return r
    }

    /// Line from the full file (expanded context), plain.
    func renderedExpanded(file: Int, newLine: Int) -> RenderedLine? {
        guard let lines = expandedText[file], newLine >= 1, newLine <= lines.count else { return nil }
        let k = Int64(-1 - file) << 32 | Int64(newLine)
        if let r = cache[k] { return r }
        let lexer = Lexer(spec: LanguageRegistry.language(forPath: diff.files[file].path))
        var st = LexState.normal
        let text = lines[newLine - 1]
        var r: RenderedLine!
        appearance.performAsCurrentDrawingAppearance {
            r = make(text: text, tokens: lexer.tokenize(text, state: &st), emphasis: nil)
        }
        cache[k] = r
        cacheOrder.append(k)
        return r
    }

    private func build(file: Int, line: Int, tokens: [SyntaxToken]?) -> RenderedLine {
        let f = diff.files[file]
        let dl = f.lines[line]
        let text = diff.text(of: dl)
        if dl.kind == .noNewline {
            return make(text: text, tokens: nil, emphasis: nil, color: Theme.mutedText)
        }
        var emphasis: Range<Int>?
        if dl.kind != .context, let p = partner(file: file, line: line), text.utf16.count < 1000 {
            let other = diff.text(of: f.lines[p])
            if dl.kind == .deletion {
                emphasis = IntralineDiff.compute(old: text, new: other)?.old
            } else {
                emphasis = IntralineDiff.compute(old: other, new: text)?.new
            }
        }
        return make(text: text, tokens: tokens, emphasis: emphasis)
    }

    private lazy var paragraphStyle: NSParagraphStyle = {
        let p = NSMutableParagraphStyle()
        p.defaultTabInterval = metrics.charWidth * 4
        p.tabStops = []
        return p
    }()

    func make(text rawText: String, tokens: [SyntaxToken]?, emphasis: Range<Int>?, color: NSColor = Theme.text) -> RenderedLine {
        // Very long lines (minified code) are clipped for drawing speed.
        let text = rawText.utf16.count > 4000 ? String(rawText.prefix(4000)) + "…" : rawText
        let s = NSMutableAttributedString(string: text, attributes: [
            .font: metrics.font, ctColor: TargetSpace.convert(color.cgColor), .paragraphStyle: paragraphStyle,
            .ligature: 0,
        ])
        let len = s.length
        if let tokens {
            for t in tokens {
                let start = Int(t.start), end = min(len, start + Int(t.length))
                guard start < end, t.kind != .plain else { continue }
                s.addAttribute(ctColor, value: syntaxColor(t.kind), range: NSRange(location: start, length: end - start))
            }
        }
        let line = CTLineCreateWithAttributedString(s)
        var spans: [ClosedRange<CGFloat>] = []
        if let e = emphasis, !e.isEmpty, e.upperBound <= len {
            let x0 = CTLineGetOffsetForStringIndex(line, e.lowerBound, nil)
            let x1 = CTLineGetOffsetForStringIndex(line, e.upperBound, nil)
            if x1 > x0 { spans.append(x0...x1) }
        }
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        return RenderedLine(line: line, emphasis: spans, width: width)
    }

    /// CoreText wants CGColors; dynamic NSColors are resolved for the current drawing
    /// appearance, so caches are cleared when the appearance changes.
    private let ctColor = NSAttributedString.Key(kCTForegroundColorAttributeName as String)
    private var syntaxColors: [TokenKind: CGColor] = [:]
    private func syntaxColor(_ k: TokenKind) -> CGColor {
        if let c = syntaxColors[k] { return c }
        let c = TargetSpace.convert(Theme.syntax(k).cgColor)
        syntaxColors[k] = c
        return c
    }

    func appearanceChanged() {
        syntaxColors.removeAll()
        numberCache.removeAll()
        markerCache.removeAll()
        invalidateAll()
    }

    private var markerCache: [Bool: CTLine] = [:]
    /// Cached "+" / "-" marker lines.
    func markerLine(addition: Bool) -> CTLine {
        if let l = markerCache[addition] { return l }
        var color: CGColor!
        appearance.performAsCurrentDrawingAppearance {
            color = TargetSpace.convert((addition ? Theme.addMarker : Theme.delMarker).cgColor)
        }
        let l = CTLineCreateWithAttributedString(NSAttributedString(string: addition ? "+" : "-",
                                                                    attributes: [.font: metrics.font, ctColor: color!]))
        markerCache[addition] = l
        return l
    }

    /// Cached small CTLines for gutter numbers.
    private var numberCache: [Int: CTLine] = [:]
    func numberLine(_ n: Int) -> CTLine {
        if let l = numberCache[n] { return l }
        var color: CGColor!
        appearance.performAsCurrentDrawingAppearance { color = TargetSpace.convert(Theme.mutedText.cgColor) }
        let s = NSAttributedString(string: String(n), attributes: [.font: metrics.font, ctColor: color!])
        let l = CTLineCreateWithAttributedString(s)
        if numberCache.count > 20_000 { numberCache.removeAll() }
        numberCache[n] = l
        return l
    }
}
