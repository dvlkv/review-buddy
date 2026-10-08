import AppKit
import ReviewBuddyCore

/// Draws a file header (path, diffstat, Viewed toggle) into any context and
/// reports its hit regions. Used for in-canvas header rows and the sticky header.
@MainActor
struct FileHeaderRenderer {
    struct HitRects {
        var chevron = CGRect.zero
        var copy = CGRect.zero
        var viewed = CGRect.zero
    }

    /// Canvas strip above each header bar; 0 so files (and the first one) sit flush.
    static let gap: CGFloat = 0

    static func draw(store: PRStore, file: Int, in bounds: CGRect, ctx: CGContext, hoverViewed: Bool, appearance: NSAppearance) -> HitRects {
        var hits = HitRects()
        guard let f = store.files[safe: file] else { return hits }
        let w = bounds.width
        let top = bounds.minY
        let bar = CGRect(x: bounds.minX, y: top + gap, width: w, height: bounds.height - gap)
        ctx.setFillColor(Theme.canvas.cgColor); ctx.fill(CGRect(x: bounds.minX, y: top, width: w, height: gap))
        ctx.setFillColor(Theme.subtle.cgColor); ctx.fill(bar)
        ctx.setFillColor(Theme.border.cgColor)
        ctx.fill(CGRect(x: bounds.minX, y: bar.minY, width: w, height: 1))
        ctx.fill(CGRect(x: bounds.minX, y: bar.maxY - 1, width: w, height: 1))

        let midY = bar.midY
        let collapsed = store.options.collapsed.contains(file)
        var x: CGFloat = bounds.minX + 12
        hits.chevron = CGRect(x: bounds.minX + 4, y: bar.minY, width: 30, height: bar.height)
        Symbols.draw(collapsed ? "chevron.right" : "chevron.down", center: CGPoint(x: x + 4, y: midY), size: 11,
                     color: Theme.mutedText, weight: .semibold, appearance: appearance)
        x += 22

        switch f.status {
        case .added: Symbols.draw("plus.square", center: CGPoint(x: x + 6, y: midY), size: 12, color: Theme.addMarker, appearance: appearance); x += 20
        case .deleted: Symbols.draw("minus.square", center: CGPoint(x: x + 6, y: midY), size: 12, color: Theme.delMarker, appearance: appearance); x += 20
        case .renamed, .copied: Symbols.draw("arrow.right.square", center: CGPoint(x: x + 6, y: midY), size: 12, color: Theme.attention, appearance: appearance); x += 20
        case .modified: break
        }

        // Right side first so the path can truncate before it.
        let viewed = store.isViewed(file)
        let vText = attributed("Viewed", font: .systemFont(ofSize: 12), color: Theme.text)
        let vW = vText.size().width + 34
        hits.viewed = CGRect(x: bounds.maxX - vW - 12, y: midY - 12, width: vW, height: 24)
        let vPath = CGPath(roundedRect: hits.viewed.insetBy(dx: 0.5, dy: 0.5), cornerWidth: 6, cornerHeight: 6, transform: nil)
        if viewed {
            ctx.addPath(vPath); ctx.setFillColor(Theme.accent.withAlphaComponent(0.1).cgColor); ctx.fillPath()
        } else if hoverViewed {
            ctx.addPath(vPath); ctx.setFillColor(Theme.border.withAlphaComponent(0.25).cgColor); ctx.fillPath()
        }
        ctx.addPath(vPath)
        ctx.setStrokeColor((viewed ? Theme.accent.withAlphaComponent(0.45) : Theme.border).cgColor)
        ctx.setLineWidth(1); ctx.strokePath()
        Symbols.draw(viewed ? "checkmark.square.fill" : "square", center: CGPoint(x: hits.viewed.minX + 12, y: midY), size: 12,
                     color: viewed ? Theme.accent : Theme.mutedText, appearance: appearance)
        drawText(vText, at: CGPoint(x: hits.viewed.minX + 24, y: midY - vText.size().height / 2), ctx: ctx)

        var right = hits.viewed.minX - 14
        let threads = store.openThreadCountByPath[f.path] ?? 0
        if threads > 0 {
            let t = attributed("\(threads)", font: .systemFont(ofSize: 11), color: Theme.mutedText)
            right -= t.size().width
            drawText(t, at: CGPoint(x: right, y: midY - t.size().height / 2), ctx: ctx)
            right -= 10
            Symbols.draw("bubble.left", center: CGPoint(x: right, y: midY), size: 10, color: Theme.mutedText, appearance: appearance)
            right -= 12
        }

        // Path: dimmed directory + bold file name, truncated from the head.
        let mono = DiffMetrics.shared.font
        let dir = (f.path as NSString).deletingLastPathComponent
        let name = (f.path as NSString).lastPathComponent
        let stat = statString(f)
        let statW = stat.size().width + 5 * 9 + 6
        let maxPathW = max(60, right - x - statW - 40)
        let path = NSMutableAttributedString()
        if f.status == .renamed || f.status == .copied, f.oldPath != f.newPath {
            path.append(attributed(f.oldPath + " → ", font: mono, color: Theme.mutedText))
        }
        path.append(attributed(dir.isEmpty ? "" : dir + "/", font: mono, color: Theme.mutedText))
        path.append(attributed(name, font: DiffMetrics.shared.boldFont, color: Theme.text))
        var drawn: NSAttributedString = path
        if path.size().width > maxPathW {
            let full = path.string as NSString
            var cut = 1
            while cut < full.length {
                let sub = NSMutableAttributedString(attributedString: path.attributedSubstring(from: NSRange(location: cut, length: full.length - cut)))
                sub.insert(attributed("…", font: mono, color: Theme.mutedText), at: 0)
                if sub.size().width <= maxPathW { drawn = sub; break }
                cut += max(1, (full.length - cut) / 20)
            }
        }
        let pSize = drawn.size()
        drawText(drawn, at: CGPoint(x: x, y: midY - pSize.height / 2), ctx: ctx)
        x += pSize.width + 8
        hits.copy = CGRect(x: x - 4, y: midY - 10, width: 20, height: 20)
        Symbols.draw("doc.on.doc", center: CGPoint(x: x + 6, y: midY), size: 10, color: Theme.mutedText, appearance: appearance)
        x += 22
        drawText(stat, at: CGPoint(x: x, y: midY - stat.size().height / 2), ctx: ctx)
        x += stat.size().width + 6
        let total = max(1, f.additions + f.deletions)
        let greens = Int((Double(f.additions) / Double(total) * 5).rounded())
        for i in 0..<5 {
            ctx.setFillColor((i < greens ? Theme.addMarker : Theme.delMarker).cgColor)
            ctx.addPath(CGPath(roundedRect: CGRect(x: x + CGFloat(i) * 9, y: midY - 4, width: 8, height: 8),
                               cornerWidth: 1.5, cornerHeight: 1.5, transform: nil))
            ctx.fillPath()
        }
        return hits
    }

    /// Draws with AppKit string drawing in a flipped CG context.
    static func drawText(_ s: NSAttributedString, at p: CGPoint, ctx: CGContext) {
        let g = NSGraphicsContext(cgContext: ctx, flipped: true)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = g
        s.draw(at: p)
        NSGraphicsContext.restoreGraphicsState()
    }

    static func statString(_ f: FileDiff) -> NSAttributedString {
        let font = NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold)
        let s = NSMutableAttributedString()
        s.append(attributed("+\(f.additions) ", font: font, color: Theme.addMarker))
        s.append(attributed("−\(f.deletions)", font: font, color: Theme.delMarker))
        return s
    }

    static func attributed(_ s: String, font: NSFont, color: NSColor) -> NSAttributedString {
        NSAttributedString(string: s, attributes: [.font: font, .foregroundColor: color])
    }
}

/// Tinted SF Symbols cached per name/size/color/appearance.
@MainActor
enum Symbols {
    private static var cache: [String: NSImage] = [:]

    static func draw(_ name: String, center: CGPoint, size: CGFloat, color: NSColor, weight: NSFont.Weight = .regular,
                     appearance: NSAppearance) {
        let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let key = "\(name)|\(size)|\(weight.rawValue)|\(ObjectIdentifier(color).hashValue)|\(dark)"
        let img: NSImage
        if let c = cache[key] { img = c } else {
            guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: size, weight: weight)) else { return }
            var resolved = color
            appearance.performAsCurrentDrawingAppearance { resolved = NSColor(cgColor: color.cgColor) ?? color }
            img = NSImage(size: base.size, flipped: false) { r in
                base.draw(in: r); resolved.set(); r.fill(using: .sourceAtop); return true
            }
            cache[key] = img
        }
        let r = CGRect(x: center.x - img.size.width / 2, y: center.y - img.size.height / 2, width: img.size.width, height: img.size.height)
        guard let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
        // Flipped context: draw the image upright.
        if let ctx = NSGraphicsContext.current?.cgContext {
            ctx.saveGState()
            ctx.translateBy(x: r.minX, y: r.maxY)
            ctx.scaleBy(x: 1, y: -1)
            ctx.draw(cg, in: CGRect(origin: .zero, size: r.size))
            ctx.restoreGState()
        }
    }
}

/// The sticky header pinned to the top of the diff while its file scrolls by.
@MainActor
final class StickyHeaderView: NSView {
    weak var store: PRStore?
    var file = -1 { didSet { if file != oldValue { needsDisplay = true } } }
    private var hits = FileHeaderRenderer.HitRects()
    private var hoverViewed = false

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
        layer?.isOpaque = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        guard let store, file >= 0, let ctx = NSGraphicsContext.current?.cgContext else { return }
        hits = FileHeaderRenderer.draw(store: store, file: file, in: bounds, ctx: ctx, hoverViewed: hoverViewed,
                                       appearance: effectiveAppearance)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let h = hits.viewed.contains(p)
        if h != hoverViewed { hoverViewed = h; needsDisplay = true }
        (h || hits.chevron.contains(p) || hits.copy.contains(p) ? NSCursor.pointingHand : NSCursor.arrow).set()
    }

    override func mouseExited(with event: NSEvent) {
        if hoverViewed { hoverViewed = false; needsDisplay = true }
        NSCursor.arrow.set()
    }

    override func mouseDown(with event: NSEvent) {
        guard let store, file >= 0 else { return }
        FileHeaderActions.handleClick(store: store, file: file, at: convert(event.locationInWindow, from: nil),
                                      hits: hits, clickCount: event.clickCount)
    }
}

@MainActor
enum FileHeaderActions {
    static func handleClick(store: PRStore, file: Int, at p: CGPoint, hits: FileHeaderRenderer.HitRects, clickCount: Int) {
        if hits.viewed.contains(p) {
            store.toggleViewed(file)
        } else if hits.copy.contains(p), let f = store.files[safe: file] {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(f.path, forType: .string)
            store.banner = "Copied \(f.path)"
        } else if hits.chevron.contains(p) || clickCount == 2 {
            store.toggleCollapsed(file)
        }
    }
}
