import AppKit
import SwiftUI
import ReviewBuddyCore

/// Document view of the diff scroll view. It draws nothing itself: opaque tile
/// subviews draw the rows, and SwiftUI hosting views are placed only for inline
/// blocks (threads, composers, AI items) near the viewport.
@MainActor
final class DiffCanvasView: NSView {
    weak var coordinator: DiffTableCoordinator?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
    }

    required init?(coder: NSCoder) { fatalError() }

    // The canvas never draws; tiles and block views do.
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() {}
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func becomeFirstResponder() -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for t in trackingAreas where t.owner === self { removeTrackingArea(t) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) { coordinator?.mouseMoved(to: convert(event.locationInWindow, from: nil)) }
    override func mouseExited(with event: NSEvent) { coordinator?.mouseMoved(to: nil) }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        guard let c = coordinator, c.handleMouseDown(at: p, event: event) else { return }
        while let next = window?.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            let q = convert(next.locationInWindow, from: nil)
            if next.type == .leftMouseUp { c.handleMouseUp(at: q); break }
            autoscroll(with: next)
            c.handleDrag(to: q)
        }
    }

    override func scrollWheel(with event: NSEvent) {
        if let c = coordinator, abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) * 1.2 {
            c.scrollHorizontally(by: event.scrollingDeltaX)
            return
        }
        super.scrollWheel(with: event)
    }

    override func keyDown(with event: NSEvent) {
        if let c = coordinator, c.handleKey(event) { return }
        super.keyDown(with: event)
    }

    @objc func copy(_ sender: Any?) { coordinator?.copySelection() }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        coordinator?.appearanceChanged()
    }

    override func accessibilityRole() -> NSAccessibility.Role? { .textArea }
    override func accessibilityLabel() -> String? { "Diff" }
}

/// An opaque strip of rows rendered into its own bitmap and set as the layer's
/// contents. This bypasses AppKit's draw → display-list → replay pipeline, which
/// re-rasterized each tile several times; here a tile is rasterized exactly once
/// per (re)attach or invalidation.
@MainActor
final class DiffTileView: NSView {
    weak var coordinator: DiffTableCoordinator?
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override var wantsUpdateLayer: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
        layer?.isOpaque = true
        layer?.contentsGravity = .topLeft
    }

    required init?(coder: NSCoder) { fatalError() }

    // Let clicks fall through to the canvas.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// False until the bitmap reflects the current content.
    var isRendered = false

    override func updateLayer() {
        // AppKit marks newly added views for display; off-screen tiles wait for idle.
        if let c = coordinator, !c.shouldRenderNow(self) { c.deferRender(); return }
        render()
    }

    func render() {
        isRendered = true
        guard let c = coordinator, let layer, bounds.width > 0, bounds.height > 0 else { return }
        Perf.shared.count("tile.render")
        let scale = window?.backingScaleFactor ?? 2
        let pw = Int(ceil(bounds.width * scale)), ph = Int(ceil(bounds.height * scale))
        // Render in the window's color space so Core Animation doesn't convert each tile.
        let space = window?.colorSpace?.cgColorSpace ?? CGColorSpace(name: CGColorSpace.sRGB)!
        guard let ctx = CGContext(data: nil, width: pw, height: ph, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return }
        ctx.scaleBy(x: scale, y: scale)
        ctx.translateBy(x: 0, y: bounds.height)
        ctx.scaleBy(x: 1, y: -1)
        ctx.setShouldSmoothFonts(false)
        if TargetSpace.space != space {
            TargetSpace.space = space
            c.renderer?.appearanceChanged()  // cached CTLines hold colors in the old space
        }
        let appearance = effectiveAppearance
        let gctx = NSGraphicsContext(cgContext: ctx, flipped: true)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = gctx
        appearance.performAsCurrentDrawingAppearance {
            c.drawTile(origin: frame.origin, dirty: bounds, ctx: ctx, appearance: appearance)
        }
        NSGraphicsContext.restoreGraphicsState()
        layer.contentsScale = scale
        layer.contents = ctx.makeImage()
    }
}

/// The blue "+" shown next to the hovered line. An overlay, so hovering never
/// re-renders tiles.
@MainActor
final class HoverPlusView: NSView {
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        ctx.addPath(CGPath(roundedRect: r, cornerWidth: 5, cornerHeight: 5, transform: nil))
        ctx.setFillColor(Theme.accent.cgColor)
        ctx.fillPath()
        ctx.setStrokeColor(NSColor.white.cgColor)
        ctx.setLineWidth(1.6)
        ctx.move(to: CGPoint(x: r.midX - 4.5, y: r.midY)); ctx.addLine(to: CGPoint(x: r.midX + 4.5, y: r.midY))
        ctx.move(to: CGPoint(x: r.midX, y: r.midY - 4.5)); ctx.addLine(to: CGPoint(x: r.midX, y: r.midY + 4.5))
        ctx.strokePath()
    }
}

/// Owns the canvas: layout geometry, tiles, inline block views, sticky header,
/// hover/drag selection and keyboard handling.
@MainActor
public final class DiffTableCoordinator: NSObject, DiffViewDriving {
    weak var store: PRStore?
    weak var canvas: DiffCanvasView?
    weak var scrollView: NSScrollView?
    var tableView: NSView? { canvas }
    var renderer: LineRenderer?
    var sticky: StickyHeaderView?
    private(set) var hOffset: CGFloat = 0
    private var maxCodeWidth: CGFloat = 0
    var hover: (row: Int, side: DiffSide)?
    private var hoverPlus: HoverPlusView?
    private var numWidth: CGFloat = 40
    private var dragAnchor: (file: Int, side: DiffSide)?
    private var selectionBar: NSHostingView<AnyView>?
    private var installedDiff: ObjectIdentifier?
    private var metricsVersion = -1

    // Geometry
    private var offsets: [CGFloat] = [0]
    private var blockHeights: [String: CGFloat] = [:]
    private var rowsForBlockId: [String: Int] = [:]
    private let tileHeight: CGFloat = 512
    /// Free-floating tiles: frames are arbitrary row-aligned strips, so after an insertion
    /// tiles below simply move instead of re-rendering.
    private var tiles: [DiffTileView] = []
    private var tilePool: [DiffTileView] = []
    private var blockViews: [String: NSHostingView<AnyView>] = [:]
    private var blockViewWidth: [String: CGFloat] = [:]
    /// Hosting views kept for reuse, per block kind (creating one is several ms).
    private var blockPool: [InlineBlockRef.Kind: [NSHostingView<AnyView>]] = [:]
    private var blockKinds: [String: InlineBlockRef.Kind] = [:]

    private func returnToPool(_ id: String) {
        guard let v = blockViews.removeValue(forKey: id) else { return }
        v.removeFromSuperview()
        if let kind = blockKinds.removeValue(forKey: id), (blockPool[kind]?.count ?? 0) < 4 {
            if kind != .composer { v.rootView = AnyView(EmptyView()) }
            blockPool[kind, default: []].append(v)
        }
    }
    private var headerHits: [Int: FileHeaderRenderer.HitRects] = [:]
    private var loadDiffRects: [Int: CGRect] = [:]
    private var lastWidth: CGFloat = 0

    init(store: PRStore) {
        self.store = store
        super.init()
    }

    /// The layout `offsets` were built for. It trails `store.layout` until
    /// `layoutDidChange` runs, so anchors are captured against the old geometry.
    private var shownLayout: DiffLayout?
    var layout: DiffLayout? { shownLayout }
    var width: CGFloat { canvas?.bounds.width ?? 0 }

    func geometry(width: CGFloat) -> RowGeometry {
        RowGeometry(mode: store?.options.mode ?? .unified, numWidth: numWidth,
                    markerWidth: DiffMetrics.shared.markerWidth, width: width)
    }

    // MARK: Geometry

    func height(ofRow i: Int) -> CGFloat {
        guard let layout else { return 0 }
        let m = DiffMetrics.shared
        let r = layout.rows[i]
        switch r.kind {
        case .line, .pair, .expanded: return m.lineHeight
        case .fileHeader: return m.fileHeaderHeight
        case .hunkHeader, .expandFooter: return m.hunkHeaderHeight
        case .placeholder: return m.placeholderHeight
        case .block:
            let b = layout.blocks[Int(r.a)]
            return blockHeights[b.id] ?? estimatedHeight(b)
        }
    }

    private func estimatedHeight(_ b: InlineBlockRef) -> CGFloat {
        switch b.kind {
        case .discuss: return 46
        case .composer: return 200
        case .finding: return 150
        case .reviewThread:
            if let t = store?.threads[b.id] { return t.isResolved ? 46 : CGFloat(80 + t.comments.count * 80) }
            return 120
        }
    }

    /// Prefix sums of row heights; O(rows), ~0.3 ms for 200k rows.
    private func rebuildOffsets() {
        guard let layout else { offsets = [0]; return }
        let m = DiffMetrics.shared
        let n = layout.rowCount
        var o = [CGFloat](repeating: 0, count: n + 1)
        var y: CGFloat = 0
        layout.rows.withUnsafeBufferPointer { rows in
            for i in 0..<n {
                o[i] = y
                switch rows[i].kind {
                case .line, .pair, .expanded: y += m.lineHeight
                default: y += height(ofRow: i)
                }
            }
        }
        o[n] = y
        offsets = o
    }

    var contentHeight: CGFloat { offsets.last ?? 0 }

    func rect(ofRow i: Int) -> CGRect {
        guard i >= 0, i + 1 < offsets.count else { return .zero }
        return CGRect(x: 0, y: offsets[i], width: width, height: offsets[i + 1] - offsets[i])
    }

    /// Row containing y (binary search).
    func row(atY y: CGFloat) -> Int {
        let n = offsets.count - 1
        guard n > 0 else { return -1 }
        if y < 0 { return 0 }
        if y >= offsets[n] { return n - 1 }
        var lo = 0, hi = n - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if offsets[mid] <= y { lo = mid } else { hi = mid - 1 }
        }
        return lo
    }

    func row(at p: CGPoint) -> Int {
        guard p.y >= 0, p.y < contentHeight else { return -1 }
        return row(atY: p.y)
    }

    var visibleRect: CGRect { scrollView?.contentView.documentVisibleRect ?? .zero }

    // MARK: Install / reload

    func installDiffIfNeeded() {
        guard let store, let diff = store.diff else { return }
        let id = ObjectIdentifier(diff)
        guard installedDiff != id || metricsVersion != DiffMetrics.shared.version else { return }
        installedDiff = id
        metricsVersion = DiffMetrics.shared.version
        blockHeights.removeAll()
        let cache = store.highlights.cache ?? HighlightCache(diff: diff)
        store.highlights.cache = cache
        renderer = LineRenderer(diff: diff, highlights: cache)
        if let c = canvas { renderer?.appearance = c.effectiveAppearance }
        cache.onReady = { [weak self] file in self?.reloadFile(file) }
        store.expandedLinesProvider = { [weak self] file, lines in self?.renderer?.setExpandedContent(file: file, lines: lines) }
        for (path, lines) in store.fileContentCache {
            if let f = diff.fileIndex(path: path) { renderer?.setExpandedContent(file: f, lines: lines) }
        }
        var maxLine = 1
        var maxLen: Int32 = 0
        for f in diff.files {
            if let h = f.hunks.last { maxLine = max(maxLine, h.oldStart + h.oldCount, h.newStart + h.newCount) }
            for l in f.lines where l.textLength > maxLen { maxLen = l.textLength }
        }
        numWidth = DiffMetrics.shared.numberColumnWidth(maxLine: maxLine)
        maxCodeWidth = CGFloat(min(Int(maxLen), 4000)) * DiffMetrics.shared.charWidth + 40
        hOffset = 0
    }

    public func layoutDidChange(preserveScroll: Bool) {
        guard canvas != nil else { return }
        Perf.shared.count("layoutDidChange")
        var t0 = ContinuousClock.now
        func seg(_ n: String) { Perf.shared.record("ldc.\(n)", ms: (ContinuousClock.now - t0).milliseconds); t0 = ContinuousClock.now }
        let oldLayout = shownLayout
        let oldOffsets = offsets
        let sameDiff = installedDiff == store.flatMap { $0.diff.map(ObjectIdentifier.init) }
            && metricsVersion == DiffMetrics.shared.version
        installDiffIfNeeded()
        let anchor = preserveScroll ? captureScrollAnchor() : nil
        seg("anchor")
        shownLayout = store?.layout
        rebuildBlockIndex()
        seg("blockIndex")
        Perf.shared.measure("canvas.relayout") {
            rebuildOffsets()
            resizeCanvas()
        }
        seg("offsets")
        if let anchor { restoreScrollAnchor(anchor) } else if !preserveScroll { scrollTo(y: 0) }
        seg("restore")
        if sameDiff, let old = oldLayout, let new = shownLayout, old.options.mode == new.options.mode {
            applyGeometryChange(oldRows: old.rows, oldBlocks: old.blocks, oldOffsets: oldOffsets,
                                newRows: new.rows, newBlocks: new.blocks)
        } else {
            recycleAllTiles()
        }
        seg("geometry")
        updateViewport(force: true)
        seg("viewport")
        updateSelectionBar()
        seg("selbar")
        requestVisibleHighlights()
        prewarmIfNeeded()
    }

    private var prewarmed = false

    /// Builds one composer offscreen during idle time after the first render, so the
    /// first real "Comment" doesn't pay SwiftUI's one-time setup for that view tree.
    private func prewarmIfNeeded() {
        guard !prewarmed, let store, let canvas else { return }
        prewarmed = true
        RunLoop.main.perform(inModes: [.default]) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.canvas != nil else { return }
                // Same view structure as a real composer block, so the pooled host is reused
                // by diffing rather than rebuilding.
                let dummy = Composer(id: "prewarm", kind: .newThread(CodeAnchor(path: "", side: .right, startLine: 1, endLine: 1)))
                store.setPrewarmComposer(dummy)
                let block = InlineBlockRef(id: dummy.id, kind: .composer, side: .right, line: 1)
                let host = NSHostingView(rootView: AnyView(
                    BlockView(store: store, block: block)
                        .id("composer")
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { _ in }
                ))
                host.sizingOptions = []
                host.translatesAutoresizingMaskIntoConstraints = true
                host.frame = CGRect(x: -20_000, y: 0, width: max(600, canvas.bounds.width), height: 220)
                canvas.addSubview(host)
                host.layoutSubtreeIfNeeded()
                host.displayIfNeeded()
                DispatchQueue.main.async {
                    host.removeFromSuperview()
                    store.setPrewarmComposer(nil)
                    self.blockPool[.composer, default: []].append(host)
                }
            }
        }
    }

    private func resizeCanvas() {
        guard let canvas, let sv = scrollView else { return }
        let w = sv.contentView.bounds.width
        let h = max(contentHeight, sv.contentView.bounds.height)
        if canvas.frame.size != CGSize(width: w, height: h) {
            canvas.setFrameSize(CGSize(width: w, height: h))
        }
    }

    private func rebuildBlockIndex() {
        rowsForBlockId.removeAll(keepingCapacity: true)
        guard let layout else { return }
        for (i, r) in layout.rows.enumerated() where r.kind == .block {
            rowsForBlockId[layout.blocks[Int(r.a)].id] = i
        }
        // Drop views for blocks that no longer exist.
        for (id, v) in blockViews where rowsForBlockId[id] == nil {
            v.removeFromSuperview()
            returnToPool(id)
        }
    }

    public func reloadFile(_ file: Int) {
        guard let layout, file < layout.fileStartRow.count else { return }
        Perf.shared.count("reloadFile")
        let range = layout.rowRange(forFile: file)
        invalidate(rows: range)
    }

    func invalidate(rows: Range<Int>) {
        guard !rows.isEmpty, rows.upperBound < offsets.count else { return }
        let rect = CGRect(x: 0, y: offsets[rows.lowerBound], width: width, height: offsets[rows.upperBound] - offsets[rows.lowerBound])
        markStale(tiles.filter { $0.frame.intersects(rect) })
    }

    func invalidate(row: Int) { invalidate(rows: row..<(row + 1)) }

    /// Keeps tiles above the changed region, slides tiles below it by the height delta
    /// (no re-render), and recycles only tiles that overlap the change.
    private func applyGeometryChange(oldRows: ContiguousArray<LayoutRow>, oldBlocks: [InlineBlockRef], oldOffsets: [CGFloat],
                                     newRows: ContiguousArray<LayoutRow>, newBlocks: [InlineBlockRef]) {
        let oldN = oldRows.count, newN = newRows.count
        guard oldOffsets.count == oldN + 1, offsets.count == newN + 1 else { recycleAllTiles(); return }
        @inline(__always) func same(_ a: LayoutRow, _ b: LayoutRow) -> Bool {
            guard a.kind == b.kind, a.file == b.file else { return false }
            if a.kind == .block { return oldBlocks[Int(a.a)] == newBlocks[Int(b.a)] }
            return a.a == b.a && a.b == b.b
        }
        var i = 0
        let limit = min(oldN, newN)
        while i < limit, same(oldRows[i], newRows[i]), oldOffsets[i + 1] == offsets[i + 1] { i += 1 }
        var j = 0
        while j < limit - i {
            let oi = oldN - 1 - j, ni = newN - 1 - j
            guard same(oldRows[oi], newRows[ni]),
                  oldOffsets[oi + 1] - oldOffsets[oi] == offsets[ni + 1] - offsets[ni] else { break }
            j += 1
        }
        let keepAbove = offsets[i]                 // unchanged region ends here (both layouts)
        let oldSuffixY = oldOffsets[oldN - j]      // unchanged tail starts here (old)
        let delta = offsets[newN - j] - oldSuffixY
        var kept: [DiffTileView] = []
        for t in tiles {
            if t.frame.maxY <= keepAbove {
                kept.append(t)
            } else if t.frame.minY >= oldSuffixY {
                t.frame.origin.y += delta
                kept.append(t)
                Perf.shared.count("tile.shifted")
            } else {
                recycle(t)
            }
        }
        tiles = kept
    }

    private func recycle(_ t: DiffTileView) {
        t.removeFromSuperview()
        t.isRendered = false
        tilePool.append(t)
    }

    func recycleAllTiles() {
        for t in tiles { recycle(t) }
        tiles.removeAll()
    }

    func invalidateAllTiles() {
        Perf.shared.count("invalidateAll")
        markStale(tiles)
    }

    /// Visible tiles re-render now; off-screen ones on idle.
    private func markStale<S: Sequence>(_ ts: S) where S.Element == DiffTileView {
        let v = visibleRect
        var deferred = false
        for t in ts {
            t.isRendered = false
            if t.frame.intersects(v) { t.needsDisplay = true } else { deferred = true }
        }
        if deferred { scheduleIdleRender() }
    }

    private var idleRenderScheduled = false

    func shouldRenderNow(_ t: DiffTileView) -> Bool { t.frame.intersects(visibleRect) }
    func deferRender() { scheduleIdleRender() }

    /// Renders one stale off-screen tile per runloop turn so frames stay short.
    private func scheduleIdleRender() {
        guard !idleRenderScheduled else { return }
        idleRenderScheduled = true
        RunLoop.main.perform(inModes: [.default, .eventTracking]) { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.idleRenderScheduled = false
                let v = self.visibleRect
                // Nearest stale tile first.
                let stale = self.tiles.filter { !$0.isRendered }
                    .sorted { abs($0.frame.midY - v.midY) < abs($1.frame.midY - v.midY) }
                guard let t = stale.first else { return }
                Perf.shared.count("tile.idleRender")
                t.render()
                if stale.count > 1 { self.scheduleIdleRender() }
            }
        }
    }

    func redrawVisible() {
        invalidateAllTiles()
        sticky?.needsDisplay = true
    }

    func appearanceChanged() {
        guard let canvas else { return }
        renderer?.appearance = canvas.effectiveAppearance
        renderer?.appearanceChanged()
        redrawVisible()
    }

    func requestVisibleHighlights() {
        guard let layout, layout.rowCount > 0, let renderer else { return }
        let v = visibleRect
        let first = layout.fileIndex(forRow: max(0, row(atY: v.minY)))
        let last = layout.fileIndex(forRow: max(0, row(atY: v.maxY)))
        let ahead = min(layout.fileStartRow.count - 1, last + 3)
        renderer.highlights.request(Array(first...max(first, ahead)))
    }

    // MARK: Viewport: tiles, blocks, sticky header

    func viewportDidScroll() {
        updateViewport(force: false)
        store?.reportTopRow(topVisibleRow)
        requestVisibleHighlights()
        if hover != nil { hover = nil; hoverPlus?.isHidden = true }
    }

    func updateViewport(force: Bool) {
        guard let canvas, layout != nil else { return }
        let v = visibleRect
        let w = canvas.bounds.width
        if abs(w - lastWidth) > 0.5 {
            Perf.shared.count("widthChange")
            lastWidth = w
            recycleAllTiles()
            blockHeights.removeAll()
            blockViewWidth.removeAll()
            rebuildOffsets()
            resizeCanvas()
        }
        // Coverage: visible range plus about one tile of overscan, rounded out to the tile
        // grid so scrolling creates full tiles rather than thin strips. Tiles are kept a bit
        // longer than they're needed (hysteresis) so small back-and-forth scrolls don't churn.
        let lo = max(0, floor((v.minY - tileHeight) / tileHeight) * tileHeight)
        let hi = min(contentHeight, ceil((v.maxY + tileHeight) / tileHeight) * tileHeight)
        let keepLo = lo - tileHeight, keepHi = hi + tileHeight
        var kept: [DiffTileView] = []
        for t in tiles {
            if t.frame.maxY <= keepLo || t.frame.minY >= keepHi || t.frame.minY >= contentHeight || t.frame.height < 0.5 {
                recycle(t)
            } else {
                kept.append(t)
            }
        }
        tiles = kept.sorted { $0.frame.minY < $1.frame.minY }
        // Fill gaps with new tiles, cut at grid lines so tiles stay ~tileHeight tall.
        var gaps: [(CGFloat, CGFloat)] = []
        var cursor = lo
        for t in tiles where t.frame.maxY > lo && t.frame.minY < hi {
            if t.frame.minY > cursor + 0.5 { gaps.append((cursor, min(t.frame.minY, hi))) }
            cursor = max(cursor, t.frame.maxY)
        }
        if cursor < hi - 0.5 { gaps.append((cursor, hi)) }
        for (g0, g1) in gaps {
            var y = g0
            while y < g1 - 0.5 {
                let next = min(g1, (floor(y / tileHeight) + 1) * tileHeight)
                Perf.shared.count("tile.attach")
                let t = tilePool.popLast() ?? DiffTileView(frame: .zero)
                t.coordinator = self
                t.frame = CGRect(x: 0, y: y, width: w, height: next - y)
                t.isRendered = false
                t.layer?.contents = nil
                canvas.addSubview(t, positioned: .below, relativeTo: nil)
                tiles.append(t)
                y = next
            }
        }
        if force {
            for t in tiles where t.frame.width != w { t.frame.size.width = w; t.isRendered = false }
        }
        // On-screen tiles must be ready this frame; overscan tiles render when idle.
        var deferred = false
        for t in tiles where !t.isRendered {
            if t.frame.intersects(v) { t.needsDisplay = true } else { deferred = true }
        }
        if deferred { scheduleIdleRender() }
        updateBlockViews(visible: v)
        updateSticky(visible: v)
    }

    private func updateBlockViews(visible v: CGRect) {
        guard let layout, let store, let canvas else { return }
        let margin = v.height
        let lo = row(atY: max(0, v.minY - margin)), hi = row(atY: v.maxY + margin)
        guard lo >= 0, hi >= lo else { return }
        var live = Set<String>()
        let geo = geometry(width: canvas.bounds.width)
        for r in lo...hi where layout.rows[r].kind == .block {
            let b = layout.blocks[Int(layout.rows[r].a)]
            live.insert(b.id)
            let x: CGFloat, w: CGFloat
            if geo.mode == .split {
                x = b.side == .left ? 0 : geo.halfWidth; w = geo.halfWidth
            } else {
                x = 0; w = canvas.bounds.width
            }
            let host: NSHostingView<AnyView>
            if let existing = blockViews[b.id] {
                host = existing
            } else {
                let content = AnyView(
                    BlockView(store: store, block: b)
                        // Fresh SwiftUI state per block when a host is reused, except composers:
                        // they keep their view tree (rebuilding the editor is the expensive part)
                        // and reset their state when the composer id changes.
                        .id(b.kind == .composer ? "composer" : b.id)
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { [weak self] h in
                            self?.blockHeightChanged(id: b.id, height: h)
                        }
                )
                if let pooled = blockPool[b.kind]?.popLast() {
                    Perf.shared.count("block.reuse")
                    host = pooled
                    host.rootView = content
                } else {
                    Perf.shared.count("block.create")
                    host = NSHostingView(rootView: content)
                    host.sizingOptions = []
                    host.safeAreaRegions = []
                    host.translatesAutoresizingMaskIntoConstraints = true
                }
                blockViews[b.id] = host
                blockKinds[b.id] = b.kind
            }
            let isNew = host.superview == nil
            if isNew { canvas.addSubview(host) }
            let rr = rect(ofRow: r)
            let f = CGRect(x: x, y: rr.minY, width: w, height: max(rr.height, 1))
            if host.frame != f { host.frame = f }
        }
        // Detach far-away blocks; keep composers alive so drafts survive scrolling.
        for (id, v) in blockViews where !live.contains(id) && v.superview != nil {
            let isComposer = store.composer(id) != nil
            v.removeFromSuperview()
            if !isComposer && blockViews.count > 80 { returnToPool(id) }
        }
    }

    private func updateSticky(visible v: CGRect) {
        guard let sticky, let layout, let sv = scrollView, layout.rowCount > 0 else { sticky?.isHidden = true; return }
        let headerH = DiffMetrics.shared.fileHeaderHeight
        let topRow = row(atY: v.minY)
        let file = layout.fileIndex(forRow: max(0, topRow))
        let headerY = offsets[layout.fileStartRow[file]]
        // Hidden when the file's own header is fully on screen.
        guard v.minY > headerY else { sticky.isHidden = true; return }
        var push: CGFloat = 0
        if file + 1 < layout.fileStartRow.count {
            let nextY = offsets[layout.fileStartRow[file + 1]]
            if nextY - v.minY < headerH { push = nextY - v.minY - headerH }
        }
        sticky.isHidden = false
        sticky.file = file
        let top = sv.contentView.frame.minY + sv.contentInsets.top
        let f = CGRect(x: sv.contentView.frame.minX, y: top + push, width: sv.contentView.frame.width, height: headerH)
        if sticky.frame != f { sticky.frame = f }
    }

    // MARK: Drawing

    func drawTile(origin: CGPoint, dirty: CGRect, ctx: CGContext, appearance: NSAppearance) {
        guard let layout, layout.rowCount > 0, let store else {
            ctx.setFillColor(Theme.canvas.cgColor); ctx.fill(dirty); return
        }
        let absDirty = dirty.offsetBy(dx: origin.x, dy: origin.y)
        // Rows paint their full width, so only the area past the content needs clearing.
        if absDirty.maxY > contentHeight {
            ctx.setFillColor(Theme.canvas.cgColor)
            ctx.fill(CGRect(x: dirty.minX, y: max(dirty.minY, contentHeight - origin.y), width: dirty.width, height: dirty.height))
        }
        guard absDirty.minY < contentHeight else { return }
        let first = row(atY: absDirty.minY)
        let last = row(atY: min(contentHeight - 0.01, absDirty.maxY))
        guard first >= 0 else { return }
        let w = width
        for i in first...max(first, last) {
            let y0 = offsets[i] - origin.y, h = offsets[i + 1] - offsets[i]
            let r = layout.rows[i]
            ctx.saveGState()
            ctx.translateBy(x: 0, y: y0)
            let bounds = CGRect(x: 0, y: 0, width: w, height: h)
            ctx.clip(to: bounds)
            switch r.kind {
            case .line, .pair, .expanded: drawCodeRow(r, rowIndex: i, in: bounds, ctx: ctx)
            case .hunkHeader: drawHunkHeader(r, in: bounds, ctx: ctx)
            case .expandFooter: drawExpandFooter(r, in: bounds, ctx: ctx)
            case .fileHeader:
                headerHits[Int(r.file)] = FileHeaderRenderer.draw(store: store, file: Int(r.file), in: bounds, ctx: ctx,
                                                                  hoverViewed: false, appearance: appearance)
            case .placeholder:
                loadDiffRects[Int(r.file)] = drawPlaceholder(r, in: bounds, ctx: ctx)
            case .block:
                ctx.setFillColor(Theme.canvas.cgColor)  // SwiftUI view on top
                ctx.fill(bounds)
            }
            ctx.restoreGState()
        }
    }

    private func drawPlaceholder(_ r: LayoutRow, in bounds: CGRect, ctx: CGContext) -> CGRect {
        guard let store else { return .zero }
        let n = store.files[safe: Int(r.file)]?.changedLineCount ?? 0
        let reason = PlaceholderReason(rawValue: r.a) ?? .large
        let msg: String
        switch reason {
        case .binary: msg = "Binary file not shown."
        case .large: msg = "Large diffs are not rendered by default (\(n.formatted()) lines changed)."
        case .generated: msg = "Some generated files are not rendered by default."
        case .deletedOnly: msg = "This file was deleted."
        case .empty: msg = "File mode changed or empty file."
        }
        let text = FileHeaderRenderer.attributed(msg, font: .systemFont(ofSize: 13), color: Theme.mutedText)
        let ts = text.size()
        let canLoad = reason == .large || reason == .generated
        let y = canLoad ? bounds.midY - ts.height - 2 : bounds.midY - ts.height / 2
        FileHeaderRenderer.drawText(text, at: CGPoint(x: (bounds.width - ts.width) / 2, y: y), ctx: ctx)
        guard canLoad else { return .zero }
        let link = FileHeaderRenderer.attributed("Load diff", font: .systemFont(ofSize: 13, weight: .medium), color: Theme.accent)
        let ls = link.size()
        let p = CGPoint(x: (bounds.width - ls.width) / 2, y: bounds.midY + 3)
        FileHeaderRenderer.drawText(link, at: p, ctx: ctx)
        return CGRect(origin: p, size: ls).insetBy(dx: -6, dy: -4)
    }

    // MARK: Block heights

    private var pendingHeightIds = Set<String>()
    private var heightFlushScheduled = false

    /// Coalesces SwiftUI height reports and applies them on the next runloop turn.
    func blockHeightChanged(id: String, height: CGFloat) {
        let h = ceil(height)
        guard h > 0, abs((blockHeights[id] ?? -1) - h) > 0.5 else { return }
        blockHeights[id] = h
        pendingHeightIds.insert(id)
        guard !heightFlushScheduled else { return }
        heightFlushScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.heightFlushScheduled = false
            let ids = self.pendingHeightIds
            self.pendingHeightIds.removeAll()
            guard !ids.isEmpty, self.layout != nil else { return }
            Perf.shared.count("heightFlush")
            // Keep the content under the top of the viewport still while blocks above grow.
            let anchor = self.captureScrollAnchor()
            let oldOffsets = self.offsets
            self.rebuildOffsets()
            self.resizeCanvas()
            if let anchor { self.restoreScrollAnchor(anchor) }
            if let l = self.layout {
                // Rows are identical; only block heights moved. Tiles below slide.
                self.applyGeometryChange(oldRows: l.rows, oldBlocks: l.blocks, oldOffsets: oldOffsets,
                                         newRows: l.rows, newBlocks: l.blocks)
            }
            self.updateViewport(force: true)
            self.updateSelectionBar()
        }
    }

    // MARK: Scroll anchoring

    private struct ScrollAnchor {
        var file: Int
        var side: DiffSide
        var line: Int
        var blockId: String?
        var offset: CGFloat
        var row: Int
    }

    private func captureScrollAnchor() -> ScrollAnchor? {
        guard let layout, layout.rowCount > 0 else { return nil }
        let top = visibleRect.minY
        guard top > 0 else { return nil }
        let row = row(atY: top + 1)
        guard row >= 0, row < layout.rowCount else { return nil }
        let r = layout.rows[row]
        var a = ScrollAnchor(file: Int(r.file), side: .right, line: 0, blockId: nil, offset: top - offsets[row], row: row)
        let lines = sideLines(for: r)
        if let u = lines.unified, let li = u.lineIndex, let diff = store?.diff {
            let dl = diff.files[Int(r.file)].lines[li]
            a.side = dl.kind == .deletion ? .left : .right
            a.line = dl.lineNumber(on: a.side)
        } else if let rl = lines.right, rl.number > 0 {
            a.line = rl.number
        } else if let ll = lines.left {
            a.side = .left; a.line = ll.number
        } else if r.kind == .block {
            a.blockId = layout.blocks[Int(r.a)].id
        }
        return a
    }

    private func restoreScrollAnchor(_ a: ScrollAnchor) {
        guard let layout else { return }
        var row: Int?
        if let b = a.blockId { row = rowsForBlockId[b] }
        if row == nil, a.line > 0 { row = layout.row(file: a.file, side: a.side, line: a.line) }
        if row == nil, a.file < layout.fileStartRow.count { row = layout.fileStartRow[a.file] }
        guard let r = row ?? (a.row < layout.rowCount ? a.row : nil) else { return }
        scrollTo(y: offsets[r] + a.offset)
    }

    private func scrollTo(y: CGFloat) {
        guard let sv = scrollView else { return }
        let maxY = max(0, contentHeight - sv.contentView.bounds.height)
        sv.contentView.scroll(to: CGPoint(x: 0, y: min(maxY, max(0, y))))
        sv.reflectScrolledClipView(sv.contentView)
    }

    // MARK: DiffViewDriving

    public var topVisibleRow: Int {
        max(0, row(atY: visibleRect.minY + DiffMetrics.shared.fileHeaderHeight + 2))
    }

    public func scrollToRow(_ row: Int, select: Bool) {
        guard let layout, row < layout.rowCount else { return }
        var y = offsets[row]
        if layout.rows[row].kind != .fileHeader { y -= DiffMetrics.shared.fileHeaderHeight }
        scrollTo(y: y)
        viewportDidScroll()
    }

    public func focusDiff() {
        guard let canvas else { return }
        canvas.window?.makeFirstResponder(canvas)
    }

    // MARK: Mouse

    struct Hit {
        var row: Int
        var file: Int
        var side: DiffSide
        var line: Int
        var inGutter: Bool
    }

    func hit(at p: CGPoint) -> Hit? {
        guard let layout, let diff = store?.diff else { return nil }
        let row = row(at: p)
        guard row >= 0, row < layout.rowCount else { return nil }
        let r = layout.rows[row]
        let geo = geometry(width: width)
        let file = Int(r.file)
        let gutter = geo.isGutter(x: p.x)
        switch r.kind {
        case .line:
            let dl = diff.files[file].lines[Int(r.a)]
            guard dl.kind != .noNewline else { return nil }
            let side: DiffSide = dl.kind == .deletion ? .left : .right
            return Hit(row: row, file: file, side: side, line: dl.lineNumber(on: side), inGutter: gutter)
        case .pair, .expanded:
            let side = geo.mode == .split ? geo.side(atX: p.x) : .right
            let lines = sideLines(for: r)
            guard let sl = side == .left ? lines.left : lines.right, sl.number > 0, sl.kind != .noNewline else { return nil }
            return Hit(row: row, file: file, side: side, line: sl.number, inGutter: gutter)
        default:
            return nil
        }
    }

    func mouseMoved(to p: CGPoint?) {
        let new = p.flatMap { hit(at: $0) }.map { (row: $0.row, side: $0.side) }
        if new?.row == hover?.row && new?.side == hover?.side { return }
        hover = new
        positionHoverPlus()
        var hand = new != nil
        if let p, let layout {
            let r = row(at: p)
            if r >= 0, r < layout.rowCount {
                let lr = layout.rows[r]
                if lr.kind == .fileHeader, let hits = headerHits[Int(lr.file)] {
                    let local = CGPoint(x: p.x, y: p.y - offsets[r])
                    hand = hits.viewed.contains(local) || hits.chevron.contains(local) || hits.copy.contains(local)
                } else if lr.kind == .hunkHeader || lr.kind == .expandFooter {
                    let geo = geometry(width: width)
                    hand = p.x < (geo.mode == .unified ? geo.numWidth * 2 : geo.numWidth) + geo.markerWidth
                } else if lr.kind == .placeholder, let rect = loadDiffRects[Int(lr.file)] {
                    hand = rect.contains(CGPoint(x: p.x, y: p.y - offsets[r]))
                }
            }
        }
        (hand ? NSCursor.pointingHand : NSCursor.arrow).set()
    }

    private func positionHoverPlus() {
        guard let canvas else { return }
        guard let h = hover, let layout, h.row < layout.rowCount else { hoverPlus?.isHidden = true; return }
        let plus = hoverPlus ?? {
            let v = HoverPlusView(frame: .zero)
            canvas.addSubview(v, positioned: .above, relativeTo: nil)
            hoverPlus = v
            return v
        }()
        let geo = geometry(width: width)
        let x: CGFloat
        if geo.mode == .split, layout.rows[h.row].kind == .pair {
            x = (h.side == .left ? 0 : geo.halfWidth) + geo.numWidth + 2
        } else {
            x = geo.numWidth * 2 + 2
        }
        let rr = rect(ofRow: h.row)
        plus.frame = CGRect(x: x - 11, y: rr.midY - 10, width: 20, height: 20)
        plus.isHidden = false
        if plus.superview !== canvas || canvas.subviews.last !== plus {
            canvas.addSubview(plus, positioned: .above, relativeTo: nil)
        }
    }

    /// Returns true if the click starts a line selection.
    func handleMouseDown(at p: CGPoint, event: NSEvent) -> Bool {
        guard let store, let layout else { return false }
        let row = row(at: p)
        if row >= 0, row < layout.rowCount {
            let r = layout.rows[row]
            let geo = geometry(width: width)
            let gutterW = geo.mode == .unified ? geo.numWidth * 2 : geo.numWidth
            let local = CGPoint(x: p.x, y: p.y - offsets[row])
            switch r.kind {
            case .hunkHeader where p.x < gutterW + geo.markerWidth:
                store.expandAbove(file: Int(r.file), hunk: Int(r.a)); return false
            case .expandFooter where p.x < gutterW + geo.markerWidth:
                store.expandBelow(file: Int(r.file)); return false
            case .fileHeader:
                if let hits = headerHits[Int(r.file)] {
                    FileHeaderActions.handleClick(store: store, file: Int(r.file), at: local, hits: hits, clickCount: event.clickCount)
                }
                return false
            case .placeholder:
                if loadDiffRects[Int(r.file)]?.contains(local) == true { store.forceLoad(Int(r.file)) }
                return false
            default: break
            }
        }
        guard let h = hit(at: p) else {
            if store.selection != nil { clearSelection() }
            return false
        }
        let extend = event.modifierFlags.contains(.shift)
        let before = store.selection
        store.select(file: h.file, side: h.side, line: h.line, extend: extend)
        dragAnchor = (h.file, h.side)
        selectionBar?.isHidden = true
        invalidateSelection(before)
        invalidateSelection(store.selection)
        return true
    }

    func handleDrag(to p: CGPoint) {
        guard let store, let a = dragAnchor, let h = hit(at: p), h.file == a.file, let layout else { return }
        let line: Int
        if h.side == a.side { line = h.line } else {
            let lines = sideLines(for: layout.rows[h.row])
            guard let sl = a.side == .left ? (lines.left ?? lines.unified) : (lines.right ?? lines.unified), sl.number > 0 else { return }
            line = sl.number
        }
        let before = store.selection
        store.select(file: a.file, side: a.side, line: line, extend: true)
        invalidateSelection(before)
        invalidateSelection(store.selection)
    }

    func handleMouseUp(at p: CGPoint) {
        dragAnchor = nil
        updateSelectionBar()
    }

    func clearSelection() {
        let before = store?.selection
        store?.clearSelection()
        invalidateSelection(before)
        updateSelectionBar()
    }

    /// Redraws only the rows of a selection.
    func invalidateSelection(_ sel: DiffSelection?) {
        guard let sel, let layout else { return }
        let range = layout.rowRange(forFile: sel.file)
        guard let a = layout.row(file: sel.file, side: sel.side, line: min(sel.start, sel.end)),
              let b = layout.row(file: sel.file, side: sel.side, line: max(sel.start, sel.end)) else {
            invalidate(rows: range); return
        }
        invalidate(rows: a..<(b + 1))
    }

    func updateSelectionBar() {
        guard let canvas, let store, let layout else { return }
        guard let sel = store.selection,
              let row = layout.row(file: sel.file, side: sel.side, line: max(sel.start, sel.end)) else {
            selectionBar?.removeFromSuperview()
            selectionBar = nil
            return
        }
        let bar: NSHostingView<AnyView>
        if let b = selectionBar { bar = b } else {
            bar = NSHostingView(rootView: AnyView(EmptyView()))
            // Frame-based sizing: intrinsic-size constraints would make every show/hide
            // re-measure the whole window's SwiftUI root.
            bar.sizingOptions = []
            bar.translatesAutoresizingMaskIntoConstraints = true
            canvas.addSubview(bar, positioned: .above, relativeTo: nil)
            selectionBar = bar
        }
        bar.rootView = AnyView(SelectionBar(store: store, count: abs(sel.end - sel.start) + 1))
        bar.isHidden = false
        let size = bar.fittingSize
        let rr = rect(ofRow: row)
        let x = geometry(width: width).codeX(side: sel.side) + 8
        bar.frame = CGRect(x: x, y: rr.maxY + 2, width: size.width, height: size.height)
    }

    // MARK: Keyboard

    func handleKey(_ event: NSEvent) -> Bool {
        guard let store else { return false }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let key = event.charactersIgnoringModifiers ?? ""
        let stroke = KeyStroke(key, command: flags.contains(.command), shift: flags.contains(.shift),
                               option: flags.contains(.option), control: flags.contains(.control))
        guard let action = KeyBindings.action(for: stroke, textInputFocused: false) else { return false }
        switch action {
        case .nextFile, .previousFile, .nextHunk, .previousHunk, .nextThread, .previousThread:
            store.navigate(action)
        case .toggleViewed: store.toggleViewed(store.currentFile)
        case .collapseFile: store.toggleCollapsed(store.currentFile)
        case .toggleSplit: store.toggleMode()
        case .comment: store.startComment()
        case .discuss: store.startDiscuss()
        case .reply: store.replyToFocusedThread()
        case .cancel: clearSelection()
        default: return false
        }
        return true
    }

    func copySelection() {
        guard let store, let sel = store.selection else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(store.lines(for: sel.anchor).joined(separator: "\n"), forType: .string)
    }

    func scrollHorizontally(by dx: CGFloat) {
        let geo = geometry(width: width)
        let maxOffset = max(0, maxCodeWidth - geo.codeWidth(side: .right))
        let new = min(maxOffset, max(0, hOffset - dx))
        guard new != hOffset else { return }
        hOffset = new
        invalidateAllTiles()
    }
}
