import AppKit
import CoreText
import ReviewBuddyCore

/// Geometry for the gutter/code columns of a row, shared by drawing and hit-testing.
@MainActor
struct RowGeometry {
    let mode: DiffViewMode
    let numWidth: CGFloat
    let markerWidth: CGFloat
    let width: CGFloat

    /// Unified: [old# | new# | ± | code]. Split: [# | ± | code] [# | ± | code].
    var halfWidth: CGFloat { floor(width / 2) }

    func codeX(side: DiffSide) -> CGFloat {
        switch mode {
        case .unified: numWidth * 2 + markerWidth
        case .split: (side == .left ? 0 : halfWidth) + numWidth + markerWidth
        }
    }

    func codeWidth(side: DiffSide) -> CGFloat {
        switch mode {
        case .unified: width - codeX(side: .right)
        case .split: halfWidth - numWidth - markerWidth
        }
    }

    /// Which side a point belongs to in split mode (unified: decided by the line kind).
    func side(atX x: CGFloat) -> DiffSide { mode == .split && x >= halfWidth ? .right : .left }

    func isGutter(x: CGFloat) -> Bool {
        if mode == .unified { return x < numWidth * 2 + markerWidth }
        let local = x >= halfWidth ? x - halfWidth : x
        return local < numWidth + markerWidth
    }
}

extension DiffTableCoordinator {
    struct SideLine {
        var kind: LineKind?
        var number: Int
        var lineIndex: Int?
        var expandedNew: Int?
    }

    func sideLines(for row: LayoutRow) -> (left: SideLine?, right: SideLine?, unified: SideLine?) {
        guard let diff = store?.diff else { return (nil, nil, nil) }
        let f = diff.files[Int(row.file)]
        switch row.kind {
        case .line:
            let l = f.lines[Int(row.a)]
            return (nil, nil, SideLine(kind: l.kind, number: 0, lineIndex: Int(row.a)))
        case .pair:
            let left = row.a >= 0 ? SideLine(kind: f.lines[Int(row.a)].kind, number: Int(f.lines[Int(row.a)].oldLine), lineIndex: Int(row.a)) : nil
            let right = row.b >= 0 ? SideLine(kind: f.lines[Int(row.b)].kind, number: Int(f.lines[Int(row.b)].newLine), lineIndex: Int(row.b)) : nil
            return (left, right, nil)
        case .expanded:
            return (SideLine(kind: .context, number: Int(row.b), expandedNew: Int(row.a)),
                    SideLine(kind: .context, number: Int(row.a), expandedNew: Int(row.a)), nil)
        default:
            return (nil, nil, nil)
        }
    }

    /// The text drawn for `side` of a code row.
    func renderedLine(row: LayoutRow, side: DiffSide) -> RenderedLine? {
        guard let renderer else { return nil }
        let lines = sideLines(for: row)
        if let u = lines.unified, let li = u.lineIndex { return renderer.rendered(file: Int(row.file), line: li) }
        // Expanded rows in unified mode draw one combined line (the right side).
        let geo = geometry(width: width)
        let sl = geo.mode == .unified ? lines.right : (side == .left ? lines.left : lines.right)
        guard let sl else { return nil }
        if let e = sl.expandedNew { return renderer.renderedExpanded(file: Int(row.file), newLine: e) }
        if let li = sl.lineIndex { return renderer.rendered(file: Int(row.file), line: li) }
        return nil
    }

    func drawCodeRow(_ row: LayoutRow, rowIndex: Int, in bounds: CGRect, ctx: CGContext) {
        guard let store, let diff = store.diff, let renderer else { return }
        let m = DiffMetrics.shared
        let geo = geometry(width: bounds.width)
        let file = Int(row.file)
        let f = diff.files[file]
        let h = bounds.height
        let sel = store.selection

        let appearance = renderer.appearance
        func fill(_ color: NSColor, _ rect: CGRect) {
            ctx.setFillColor(OpaqueColors.cg(color, appearance: appearance))
            ctx.fill(rect)
        }

        func drawText(_ r: RenderedLine, x: CGFloat, clipWidth: CGFloat, emphasisColor: NSColor?, highlight: Range<Int>? = nil) {
            ctx.saveGState()
            ctx.clip(to: CGRect(x: x, y: 0, width: max(0, clipWidth), height: h))
            let ox = x + 8 - hOffset
            if let highlight {
                // Text selection: the dragged-over characters, plus a sliver for the
                // newline when the selection continues onto the next line.
                let len = CTLineGetStringRange(r.line).length
                let x0 = CTLineGetOffsetForStringIndex(r.line, min(highlight.lowerBound, len), nil)
                var x1 = CTLineGetOffsetForStringIndex(r.line, min(highlight.upperBound, len), nil)
                if highlight.upperBound > len { x1 += m.charWidth / 2 }
                ctx.setFillColor(TargetSpace.convert(Theme.textSelection.cgColor))
                ctx.fill(CGRect(x: ox + x0, y: 0, width: max(0, x1 - x0), height: h))
            }
            if let emphasisColor {
                ctx.setFillColor(TargetSpace.convert(emphasisColor.cgColor))
                for span in r.emphasis {
                    ctx.fill(CGRect(x: ox + span.lowerBound, y: 1, width: span.upperBound - span.lowerBound, height: h - 2))
                }
            }
            ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
            ctx.textPosition = CGPoint(x: ox, y: floor((h + m.ascent - m.font.descender * -1) / 2 + 0.5))
            CTLineDraw(r.line, ctx)
            ctx.restoreGState()
        }

        func drawNumber(_ n: Int, right: CGFloat) {
            guard n > 0 else { return }
            let l = renderer.numberLine(n)
            let w = CGFloat(CTLineGetTypographicBounds(l, nil, nil, nil))
            ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
            ctx.textPosition = CGPoint(x: right - 10 - w, y: floor((h + m.ascent - m.font.descender * -1) / 2 + 0.5))
            CTLineDraw(l, ctx)
        }

        func drawMarker(_ kind: LineKind?, x: CGFloat) {
            guard let kind, kind == .addition || kind == .deletion else { return }
            ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
            ctx.textPosition = CGPoint(x: x + 7, y: floor((h + m.ascent - m.font.descender * -1) / 2 + 0.5))
            CTLineDraw(renderer.markerLine(addition: kind == .addition), ctx)
        }

        func colors(_ kind: LineKind?, selected: Bool) -> (bg: NSColor, num: NSColor, word: NSColor?) {
            if selected { return (Theme.selectedBg, Theme.selectedNumBg, nil) }
            switch kind {
            case .addition: return (Theme.addBg, Theme.addNumBg, Theme.addWord)
            case .deletion: return (Theme.delBg, Theme.delNumBg, Theme.delWord)
            case .none: return (Theme.emptyBg, Theme.emptyBg, nil)
            default: return (Theme.canvas, Theme.canvas, nil)
            }
        }

        func plus(at x: CGFloat) {
            let r = CGRect(x: x - 11, y: (h - 20) / 2, width: 20, height: 20)
            let path = CGPath(roundedRect: r, cornerWidth: 5, cornerHeight: 5, transform: nil)
            ctx.addPath(path)
            ctx.setFillColor(Theme.accent.cgColor)
            ctx.fillPath()
            ctx.setStrokeColor(NSColor.white.cgColor)
            ctx.setLineWidth(1.6)
            ctx.move(to: CGPoint(x: r.midX - 4.5, y: r.midY)); ctx.addLine(to: CGPoint(x: r.midX + 4.5, y: r.midY))
            ctx.move(to: CGPoint(x: r.midX, y: r.midY - 4.5)); ctx.addLine(to: CGPoint(x: r.midX, y: r.midY + 4.5))
            ctx.strokePath()
        }

        let lines = sideLines(for: row)
        if let u = lines.unified, let li = u.lineIndex {
            // Unified
            let dl = f.lines[li]
            let side: DiffSide = dl.kind == .deletion ? .left : .right
            let n = dl.lineNumber(on: side)
            let selected = sel.map { $0.file == file && $0.contains(side: side, line: n) } ?? false
                || (dl.kind == .context && (sel.map { $0.file == file && $0.contains(side: .left, line: Int(dl.oldLine)) } ?? false))
            // A text selection highlights characters; the gutter still marks the lines.
            let partial = selected && sel?.isPartial == true
            let c = colors(dl.kind, selected: selected && !partial)
            fill(c.bg, bounds)
            fill(partial ? Theme.selectedNumBg : c.num, CGRect(x: 0, y: 0, width: geo.numWidth * 2, height: h))
            drawNumber(Int(dl.oldLine), right: geo.numWidth)
            drawNumber(Int(dl.newLine), right: geo.numWidth * 2)
            drawMarker(dl.kind, x: geo.numWidth * 2)
            let selLine = sel?.side == .left ? Int(dl.oldLine) : Int(dl.newLine)
            drawText(renderer.rendered(file: file, line: li), x: geo.codeX(side: .right),
                     clipWidth: geo.codeWidth(side: .right), emphasisColor: c.word,
                     highlight: partial ? sel?.columnRange(line: selLine) : nil)
            return
        }

        // Split / expanded: draw each half.
        for side in [DiffSide.left, .right] {
            let sl = side == .left ? lines.left : lines.right
            let x0: CGFloat
            let halfW: CGFloat
            if geo.mode == .split {
                x0 = side == .left ? 0 : geo.halfWidth
                halfW = side == .left ? geo.halfWidth : bounds.width - geo.halfWidth
            } else {
                // Expanded row in unified mode: one combined line.
                if side == .left { continue }
                x0 = 0; halfW = bounds.width
            }
            let n = sl?.number ?? 0
            let selected = sl != nil && (sel.map { $0.file == file && $0.contains(side: side, line: n) } ?? false)
            let partial = selected && sel?.isPartial == true
            let c = colors(sl?.kind, selected: selected && !partial)
            let numColor = partial ? Theme.selectedNumBg : c.num
            fill(c.bg, CGRect(x: x0, y: 0, width: halfW, height: h))
            if geo.mode == .unified {
                fill(numColor, CGRect(x: 0, y: 0, width: geo.numWidth * 2, height: h))
                drawNumber(lines.left?.number ?? 0, right: geo.numWidth)
                drawNumber(lines.right?.number ?? 0, right: geo.numWidth * 2)
            } else {
                fill(numColor, CGRect(x: x0, y: 0, width: geo.numWidth, height: h))
                drawNumber(n, right: x0 + geo.numWidth)
                drawMarker(sl?.kind, x: x0 + geo.numWidth)
            }
            guard let sl else { continue }
            let rendered: RenderedLine?
            if let e = sl.expandedNew { rendered = renderer.renderedExpanded(file: file, newLine: e) }
            else if let li = sl.lineIndex { rendered = renderer.rendered(file: file, line: li) }
            else { rendered = nil }
            if let rendered {
                drawText(rendered, x: geo.codeX(side: side), clipWidth: geo.codeWidth(side: side), emphasisColor: c.word,
                         highlight: partial ? sel?.columnRange(line: n) : nil)
            }
        }
        if geo.mode == .split {
            fill(Theme.mutedBorder, CGRect(x: geo.halfWidth - 0.5, y: 0, width: 1, height: h))
        }
    }

    func drawHunkHeader(_ row: LayoutRow, in bounds: CGRect, ctx: CGContext) {
        guard let store, let diff = store.diff else { return }
        let m = DiffMetrics.shared
        let geo = geometry(width: bounds.width)
        let hunk = diff.files[Int(row.file)].hunks[Int(row.a)]
        let ap = renderer?.appearance ?? NSApp.effectiveAppearance
        ctx.setFillColor(OpaqueColors.cg(Theme.hunkBg, appearance: ap))
        ctx.fill(bounds)
        let gutterW = geo.mode == .unified ? geo.numWidth * 2 : geo.numWidth
        ctx.setFillColor(OpaqueColors.cg(Theme.hunkNumBg, appearance: ap))
        ctx.fill(CGRect(x: 0, y: 0, width: gutterW, height: bounds.height))
        if store.canExpandAbove(file: Int(row.file), hunk: Int(row.a)) {
            let glyph = NSAttributedString(string: "⤒", attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .semibold),
                                                                     .foregroundColor: Theme.mutedText])
            let sz = glyph.size()
            glyph.draw(at: CGPoint(x: (gutterW - sz.width) / 2, y: (bounds.height - sz.height) / 2))
        }
        let text = NSAttributedString(string: hunk.headerText, attributes: [.font: m.font, .foregroundColor: Theme.mutedText])
        text.draw(at: CGPoint(x: gutterW + (geo.mode == .unified ? geo.markerWidth : geo.markerWidth) + 8,
                              y: (bounds.height - m.font.ascender + m.font.descender) / 2 - 1))
    }

    func drawExpandFooter(_ row: LayoutRow, in bounds: CGRect, ctx: CGContext) {
        guard let store else { return }
        let geo = geometry(width: bounds.width)
        let ap = renderer?.appearance ?? NSApp.effectiveAppearance
        ctx.setFillColor(OpaqueColors.cg(Theme.hunkBg, appearance: ap))
        ctx.fill(bounds)
        let gutterW = geo.mode == .unified ? geo.numWidth * 2 : geo.numWidth
        ctx.setFillColor(OpaqueColors.cg(Theme.hunkNumBg, appearance: ap))
        ctx.fill(CGRect(x: 0, y: 0, width: gutterW, height: bounds.height))
        guard store.canExpandBelow(file: Int(row.file)) else { return }
        let glyph = NSAttributedString(string: "⤓", attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .semibold),
                                                                 .foregroundColor: Theme.mutedText])
        let sz = glyph.size()
        glyph.draw(at: CGPoint(x: (gutterW - sz.width) / 2, y: (bounds.height - sz.height) / 2))
    }
}

