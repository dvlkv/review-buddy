import AppKit
import Foundation
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

/// Wires a DiffTableCoordinator to a real (windowless) scroll view + canvas, like DiffView.makeNSView.
@MainActor
final class CanvasRig {
    let h: Harness
    let coordinator: DiffTableCoordinator
    let scroll: NSScrollView
    let canvas: DiffCanvasView
    let sticky: StickyHeaderView

    init(h: Harness) {
        self.h = h
        coordinator = DiffTableCoordinator(store: h.store)
        scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        canvas = DiffCanvasView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        canvas.coordinator = coordinator
        scroll.documentView = canvas
        sticky = StickyHeaderView()
        sticky.store = h.store
        scroll.addSubview(sticky)
        coordinator.canvas = canvas
        coordinator.scrollView = scroll
        coordinator.sticky = sticky
        h.store.view = coordinator
        coordinator.layoutDidChange(preserveScroll: false)
    }

    static func make(threadsEvery: Int = 25) async -> CanvasRig {
        let h = Harness(threadsEvery: threadsEvery)
        await h.load()
        return CanvasRig(h: h)
    }
}

@MainActor
@Suite struct CanvasGeometryTests {
    @Test func offsetsAreConsistent() async {
        let rig = await CanvasRig.make(threadsEvery: 2)
        let c = rig.coordinator
        let layout = rig.h.store.layout!
        var sum: CGFloat = 0
        for i in 0..<layout.rowCount {
            let r = c.rect(ofRow: i)
            #expect(r.minY == sum)
            #expect(r.height == c.height(ofRow: i))
            sum += r.height
            #expect(c.row(atY: r.minY) == i)
            #expect(c.row(atY: r.midY) == i)
        }
        #expect(c.contentHeight == sum)
        #expect(c.row(atY: -5) == 0)
        #expect(c.row(atY: sum + 100) == layout.rowCount - 1)
        #expect(rig.canvas.frame.height == max(sum, 600))
        #expect(c.rect(ofRow: -1) == .zero)
    }

    @Test func rowHeightsByKind() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        let m = DiffMetrics.shared
        let layout = rig.h.store.layout!
        for (i, r) in layout.rows.enumerated() {
            switch r.kind {
            case .line, .pair, .expanded: #expect(c.height(ofRow: i) == m.lineHeight)
            case .fileHeader: #expect(c.height(ofRow: i) == m.fileHeaderHeight)
            case .hunkHeader, .expandFooter: #expect(c.height(ofRow: i) == m.hunkHeaderHeight)
            case .placeholder: #expect(c.height(ofRow: i) == m.placeholderHeight)
            case .block: #expect(c.height(ofRow: i) > 0)
            }
        }
    }

    @Test func hitTestingUnifiedAndSplit() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        let s = rig.h.store
        let layout = s.layout!
        // Unified: deletion → left side, addition → right side.
        let delRow = layout.rows.firstIndex { $0.kind == .line && s.files[Int($0.file)].lines[Int($0.a)].kind == .deletion }!
        let addRow = layout.rows.firstIndex { $0.kind == .line && s.files[Int($0.file)].lines[Int($0.a)].kind == .addition }!
        let pDel = CGPoint(x: 300, y: c.rect(ofRow: delRow).midY)
        let hit = c.hit(at: pDel)!
        #expect(hit.side == .left && hit.row == delRow)
        #expect(hit.line == Int(s.files[0].lines[Int(layout.rows[delRow].a)].oldLine))
        #expect(!hit.inGutter)
        let hitAdd = c.hit(at: CGPoint(x: 5, y: c.rect(ofRow: addRow).midY))!
        #expect(hitAdd.side == .right && hitAdd.inGutter)
        // Non-code rows have no hit.
        #expect(c.hit(at: CGPoint(x: 300, y: c.rect(ofRow: 0).midY)) == nil)

        s.setMode(.split)
        let l2 = s.layout!
        let pairRow = l2.rows.firstIndex { $0.kind == .pair && $0.a >= 0 && $0.b >= 0 && $0.a != $0.b }!
        let y = c.rect(ofRow: pairRow).midY
        let left = c.hit(at: CGPoint(x: 200, y: y))!
        let right = c.hit(at: CGPoint(x: 600, y: y))!
        #expect(left.side == .left && right.side == .right)
        #expect(left.line == Int(s.files[0].lines[Int(l2.rows[pairRow].a)].oldLine))
        #expect(right.line == Int(s.files[0].lines[Int(l2.rows[pairRow].b)].newLine))
    }

    @Test func scrollingAndTiles() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        let layout = rig.h.store.layout!
        #expect(rig.canvas.subviews.contains { $0 is DiffTileView })
        let target = layout.fileStartRow[3]
        c.scrollToRow(target, select: false)
        #expect(abs(rig.scroll.contentView.bounds.minY - c.rect(ofRow: target).minY) < 1)
        #expect(layout.fileIndex(forRow: c.topVisibleRow) == 3)
        #expect(rig.h.store.currentFile == 3)
        // Tiles cover the visible rect after scrolling.
        let v = rig.scroll.contentView.documentVisibleRect
        let tiles = rig.canvas.subviews.compactMap { $0 as? DiffTileView }
        #expect(tiles.contains { $0.frame.intersects(v) })
        #expect(tiles.allSatisfy { $0.frame.height > 0 && $0.frame.height <= 512 })
        // Code row inside file 3 scrolls below the sticky header.
        let codeRow = target + 3
        c.scrollToRow(codeRow, select: false)
        #expect(abs(rig.scroll.contentView.bounds.minY - (c.rect(ofRow: codeRow).minY - DiffMetrics.shared.fileHeaderHeight)) < 1)
        #expect(!rig.sticky.isHidden)
        #expect(rig.sticky.file == 3)
    }

    @Test func blockHeightChangeUpdatesOffsets() async {
        let rig = await CanvasRig.make(threadsEvery: 2)
        let c = rig.coordinator
        let layout = rig.h.store.layout!
        let blockRow = layout.rows.firstIndex { $0.kind == .block }!
        let id = layout.blocks[Int(layout.rows[blockRow].a)].id
        let before = c.contentHeight
        let old = c.height(ofRow: blockRow)
        c.blockHeightChanged(id: id, height: old + 100)
        #expect(await eventually { c.contentHeight == before + 100 })
        #expect(c.rect(ofRow: blockRow + 1).minY == c.rect(ofRow: blockRow).maxY)
        // Same height again: no change.
        c.blockHeightChanged(id: id, height: old + 100)
        try? await Task.sleep(for: .milliseconds(20))
        #expect(c.contentHeight == before + 100)
    }

    @Test func layoutChangesPreserveFileOnModeSwitch() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        let s = rig.h.store
        c.scrollToRow(s.layout!.fileStartRow[4] + 5, select: false)
        s.setMode(.split)
        #expect(s.layout!.fileIndex(forRow: c.topVisibleRow) == 4)
    }

    @Test func tileRenderProducesContents() async {
        let rig = await CanvasRig.make()
        let tile = rig.canvas.subviews.compactMap { $0 as? DiffTileView }.first!
        tile.render()
        #expect(tile.isRendered)
        #expect(tile.layer?.contents != nil)
    }
}

@MainActor
@Suite struct CanvasInteractionTests {
    func key(_ chars: String, keyCode: UInt16 = 0) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
                         context: nil, characters: chars, charactersIgnoringModifiers: chars, isARepeat: false,
                         keyCode: keyCode)!
    }

    @Test func keyboardShortcuts() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        let s = rig.h.store
        #expect(c.handleKey(key("s")))
        #expect(s.viewMode == .split)
        #expect(c.handleKey(key("s")))
        #expect(s.viewMode == .unified)
        let fileBefore = s.currentFile
        #expect(c.handleKey(key("v")))
        // Viewed files collapse, so the top of the viewport moves on to the next file.
        #expect(s.isViewed(fileBefore))
        #expect(s.options.collapsed.contains(fileBefore))
        #expect(c.handleKey(key("x")))
        s.select(file: 1, side: .right, line: 13, extend: false)
        #expect(c.handleKey(key("\u{1b}", keyCode: 53)))
        #expect(s.selection == nil)
        s.select(file: 1, side: .right, line: 13, extend: false)
        #expect(c.handleKey(key("c")))
        #expect(s.composers.count == 1)
        s.select(file: 1, side: .right, line: 14, extend: false)
        #expect(c.handleKey(key("d")))
        #expect(s.activeDiscussId != nil)
        #expect(!c.handleKey(key("z")))
    }

    @Test func copySelectionWritesPasteboard() async {
        let rig = await CanvasRig.make()
        let s = rig.h.store
        s.select(file: 0, side: .right, line: 10, extend: false)
        s.select(file: 0, side: .right, line: 11, extend: true)
        // Don't clobber the user's clipboard.
        let saved = NSPasteboard.general.string(forType: .string)
        defer {
            NSPasteboard.general.clearContents()
            if let saved { NSPasteboard.general.setString(saved, forType: .string) }
        }
        rig.coordinator.copySelection()
        let text = NSPasteboard.general.string(forType: .string)
        #expect(text == s.lines(for: s.selection!.anchor).joined(separator: "\n"))
    }

    @Test func horizontalScrollIsClamped() async {
        let rig = await CanvasRig.make()
        let c = rig.coordinator
        c.scrollHorizontally(by: 50) // scrolling left past 0 is clamped
        #expect(c.hOffset == 0)
        c.scrollHorizontally(by: -100_000)
        #expect(c.hOffset > 0)
        let maxed = c.hOffset
        c.scrollHorizontally(by: -100)
        #expect(c.hOffset == maxed)
    }

    @Test func selectionBarAppearsForSelection() async {
        let rig = await CanvasRig.make()
        let s = rig.h.store
        s.select(file: 1, side: .right, line: 13, extend: false)
        rig.coordinator.updateSelectionBar()
        #expect(rig.canvas.subviews.contains { String(describing: type(of: $0)).contains("NSHostingView") })
        rig.coordinator.clearSelection()
        #expect(s.selection == nil)
    }
}

@MainActor
@Suite struct RendererTests {
    @Test func rowGeometryMath() {
        let u = RowGeometry(mode: .unified, numWidth: 40, markerWidth: 22, width: 1000)
        #expect(u.codeX(side: .right) == 102)
        #expect(u.codeX(side: .left) == 102)
        #expect(u.codeWidth(side: .right) == 898)
        #expect(u.isGutter(x: 101) && !u.isGutter(x: 103))
        #expect(u.side(atX: 900) == .left) // unified: side decided by the line kind
        let s = RowGeometry(mode: .split, numWidth: 40, markerWidth: 22, width: 1001)
        #expect(s.halfWidth == 500)
        #expect(s.codeX(side: .left) == 62 && s.codeX(side: .right) == 562)
        #expect(s.codeWidth(side: .right) == 438)
        #expect(s.side(atX: 499) == .left && s.side(atX: 500) == .right)
        #expect(s.isGutter(x: 510) && !s.isGutter(x: 600) && s.isGutter(x: 10))
    }

    @Test func partnersAndEmphasis() {
        let d = UnifiedDiffParser.parse("--- a/x.swift\n+++ b/x.swift\n@@ -1,2 +1,2 @@\n ctx\n-let value = computeOld(x)\n+let value = computeNew(x)\n")
        let r = LineRenderer(diff: d, highlights: HighlightCache(diff: d))
        #expect(r.partner(file: 0, line: 1) == 2)
        #expect(r.partner(file: 0, line: 2) == 1)
        #expect(r.partner(file: 0, line: 0) == nil)
        let del = r.rendered(file: 0, line: 1)
        #expect(del.emphasis.count == 1)
        #expect(del.width > 0)
        #expect(r.rendered(file: 0, line: 1) === del) // cached
        #expect(r.rendered(file: 0, line: 0).emphasis.isEmpty)
        r.invalidateAll()
        #expect(r.rendered(file: 0, line: 1) !== del)
        r.setExpandedContent(file: 0, lines: ["a", "b"])
        #expect(r.renderedExpanded(file: 0, newLine: 2) != nil)
        #expect(r.renderedExpanded(file: 0, newLine: 3) == nil)
        #expect(r.expandedContent(file: 0) == ["a", "b"])
    }

    @Test func highlightCacheSyncForSmallFiles() async {
        let d = UnifiedDiffParser.parse(SyntheticDiff.make(.init(files: 2, hunksPerFile: 1, linesPerHunk: 12)))
        let cache = HighlightCache(diff: d)
        #expect(cache.tokens(file: 0) == nil)
        let t = cache.tokensOrHighlightNow(file: 0)
        #expect(t?.count == d.files[0].lines.count)
        #expect(cache.tokens(file: 0) != nil)
        // Small files are never queued for async work.
        var ready: [Int] = []
        cache.onReady = { ready.append($0) }
        cache.request([1])
        try? await Task.sleep(for: .milliseconds(50))
        #expect(ready.isEmpty)
    }

    @Test func highlightCacheAsyncForLargeFiles() async {
        let d = UnifiedDiffParser.parse(SyntheticDiff.make(.init(files: 1, hunksPerFile: 220, linesPerHunk: 20)))
        #expect(d.files[0].lines.count > 4000)
        let cache = HighlightCache(diff: d)
        #expect(cache.tokensOrHighlightNow(file: 0) == nil)
        var ready: [Int] = []
        cache.onReady = { ready.append($0) }
        #expect(await eventually(timeout: 10) { ready == [0] })
        #expect(cache.tokens(file: 0)?.count == d.files[0].lines.count)
    }

    @Test func opaqueColorsAreOpaque() {
        for ap in [NSAppearance(named: .aqua)!, NSAppearance(named: .darkAqua)!] {
            for color in [Theme.addBg, Theme.delBg, Theme.hunkBg, Theme.canvas] {
                let cg = OpaqueColors.cg(color, appearance: ap)
                #expect(cg.alpha == 1)
            }
        }
        let light = OpaqueColors.cg(Theme.addBg, appearance: NSAppearance(named: .aqua)!)
        let dark = OpaqueColors.cg(Theme.addBg, appearance: NSAppearance(named: .darkAqua)!)
        #expect(light.components! != dark.components!)
    }

    @Test func metricsNumberColumn() {
        let m = DiffMetrics.shared
        #expect(m.numberColumnWidth(maxLine: 9) == m.numberColumnWidth(maxLine: 999))
        #expect(m.numberColumnWidth(maxLine: 10_000) > m.numberColumnWidth(maxLine: 999))
        #expect(m.lineHeight > m.font.pointSize)
    }
}

@MainActor
@Suite struct FileTreeBuilderTests {
    @Test func displayOrderMatchesTree() {
        // Folders first, then files, case-insensitive — and "Feature10" before "Feature2".
        let paths = ["b/x.go", "a.go", "a/z.go", "B.go", "f/Feature2/y.go", "f/Feature10/y.go"]
        #expect(FileTreeBuilder.displayOrder(paths: paths) == [2, 0, 5, 4, 1, 3])
        #expect(FileTreeBuilder.displayOrder(paths: paths, priority: [3, 1]) == [3, 1, 2, 0, 5, 4])
        let rows = FileTreeBuilder.rows(paths: paths, priority: [3])
        #expect(rows.first?.isSection == true && rows[1].fileIndex == 3 && rows[1].rank == 1)
        #expect(rows[2].name == "All files")
        #expect(!rows.dropFirst(3).contains { $0.fileIndex == 3 }) // listed once
    }

    @Test func compressionAndOrdering() {
        let paths = ["src/app/main.go", "src/app/main_test.go", "src/app/a/x.go", "README.md", "docs/guide/intro.md"]
        let rows = FileTreeBuilder.rows(paths: paths)
        #expect(rows.map(\.name) == ["docs/guide", "intro.md", "src/app", "a", "x.go", "main.go", "main_test.go", "README.md"])
        #expect(rows.map(\.depth) == [0, 1, 0, 1, 2, 1, 1, 0])
        #expect(rows.first { $0.name == "main.go" }!.fileIndex == 0)
        #expect(rows.filter(\.isFolder).count == 3)
    }

    @Test func filterAndCollapse() {
        let paths = ["src/a.go", "src/b.go", "lib/c.go"]
        #expect(FileTreeBuilder.rows(paths: paths, filter: "B.GO").map(\.name) == ["src", "b.go"])
        let collapsed = FileTreeBuilder.rows(paths: paths, collapsed: ["src/"])
        #expect(collapsed.map(\.name) == ["lib", "c.go", "src"])
        #expect(FileTreeBuilder.rows(paths: paths, filter: "zzz").isEmpty)
    }
}

@MainActor
@Suite struct InboxAndAppTests {
    @Test func inboxSectionsAndRefresh() async {
        let h = Harness()
        h.services.settings.pinnedRepos = ["o/r"]
        let inbox = InboxStore(services: h.services)
        #expect(inbox.sections.map(\.id) == ["reviewRequested", "mine", "repo:o/r"])
        #expect(inbox.sections[2].query.contains("repo:o/r"))
        await inbox.refresh()
        #expect(inbox.sections.allSatisfy { $0.items.count == 1 && !$0.isLoading && $0.error == nil })
        #expect(inbox.lastRefresh != nil)
        h.services.settings.pinnedRepos = []
        inbox.pinnedReposChanged()
        #expect(inbox.sections.count == 2)
        #expect(inbox.sections[0].items.count == 1) // items kept across rebuilds
    }

    @Test func inboxPerSectionErrors() async {
        var flaky: FlakyGitHub!
        let h = Harness(github: { flaky = FlakyGitHub(base: $0); flaky.failSearch = true; return flaky })
        let inbox = InboxStore(services: h.services)
        await inbox.refresh()
        #expect(inbox.sections[0].error == nil && inbox.sections[0].items.count == 1)
        #expect(inbox.sections[1].error == "search failed")
    }

    @Test func appModelCachesStores() async {
        let h = Harness()
        let app = AppModel()
        app.install(services: h.services)
        #expect(app.inbox != nil)
        await app.open(Harness.ref)
        let first = app.current
        #expect(first?.loadState == .loaded)
        await app.open(Harness.ref)
        #expect(app.current === first)
        #expect(await app.open(urlString: "not a url") == false)
        #expect(app.current === first)
    }

    @Test func openByURLParses() async {
        let h = Harness()
        let app = AppModel()
        app.install(services: h.services)
        #expect(await app.open(urlString: "https://github.com/demo/synthetic/pull/1/files"))
        #expect(app.current?.ref == Harness.ref)
    }
}
