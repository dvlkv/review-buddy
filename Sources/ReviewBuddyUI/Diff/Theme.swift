import AppKit
import SwiftUI
import ReviewBuddyCore

/// GitHub Primer colors for the diff, light and dark.
public enum Theme {
    static func dyn(_ light: UInt32, _ dark: UInt32, _ la: CGFloat = 1, _ da: CGFloat = 1) -> NSColor {
        NSColor(name: nil) { ap in
            ap.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? NSColor(hex: dark, alpha: da) : NSColor(hex: light, alpha: la)
        }
    }

    // Surfaces
    public static let canvas = dyn(0xFFFFFF, 0x0D1117)
    public static let subtle = dyn(0xF6F8FA, 0x151B23)
    public static let border = dyn(0xD1D9E0, 0x3D444D)
    public static let mutedBorder = dyn(0xD1D9E0, 0x3D444D, 0.7, 0.7)
    public static let text = dyn(0x1F2328, 0xF0F6FC)
    public static let mutedText = dyn(0x59636E, 0x9198A1)
    public static let accent = dyn(0x0969DA, 0x4493F8)

    // Diff lines
    public static let addBg = dyn(0xDAFBE1, 0x2EA043, 0.55, 0.15)
    public static let addNumBg = dyn(0xACEEBB, 0x3FB950, 0.55, 0.3)
    public static let addWord = dyn(0xACEEBB, 0x2EA043, 1, 0.4)
    public static let delBg = dyn(0xFFEBE9, 0xF85149, 1, 0.1)
    public static let delNumBg = dyn(0xFFCECB, 0xF85149, 0.75, 0.3)
    public static let delWord = dyn(0xFF8182, 0xF85149, 0.4, 0.4)
    public static let hunkBg = dyn(0xDDF4FF, 0x388BFD, 1, 0.1)
    public static let hunkNumBg = dyn(0xB6E3FF, 0x388BFD, 1, 0.25)
    public static let emptyBg = dyn(0xF6F8FA, 0x151B23, 1, 0.6)
    public static let selectedBg = dyn(0xFFF8C5, 0xBB8009, 1, 0.15)
    public static let selectedNumBg = dyn(0xFFF1A6, 0xBB8009, 1, 0.3)
    public static let addMarker = dyn(0x1A7F37, 0x3FB950)
    public static let delMarker = dyn(0xD1242F, 0xF85149)
    public static let aiAccent = dyn(0x8250DF, 0xAB7DF8)
    public static let aiBg = dyn(0xFBEFFF, 0xAB7DF8, 1, 0.1)
    public static let pendingBg = dyn(0xFFF8C5, 0xBB8009, 1, 0.15)
    public static let attention = dyn(0x9A6700, 0xD29922)

    // Syntax (Primer "pl-" classes)
    static func syntax(_ kind: TokenKind) -> NSColor {
        switch kind {
        case .plain: text
        case .keyword: dyn(0xCF222E, 0xFF7B72)
        case .type: dyn(0x953800, 0xFFA657)
        case .string: dyn(0x0A3069, 0xA5D6FF)
        case .number, .constant, .property: dyn(0x0550AE, 0x79C0FF)
        case .comment: dyn(0x59636E, 0x9198A1)
        case .function: dyn(0x6639BA, 0xD2A8FF)
        case .attribute: dyn(0x0550AE, 0x79C0FF)
        case .tag: dyn(0x116329, 0x7EE787)
        }
    }
}

/// Colors pre-converted to the tile bitmap's color space. Drawing an sRGB color into a
/// Display P3 bitmap forces color matching on every fill and glyph run.
@MainActor
enum TargetSpace {
    static var space: CGColorSpace = CGColorSpace(name: CGColorSpace.sRGB)! {
        didSet { if space != oldValue { cache.removeAll() } }
    }
    private static var cache: [CGColor: CGColor] = [:]

    static func convert(_ c: CGColor) -> CGColor {
        if let hit = cache[c] { return hit }
        let out = c.converted(to: space, intent: .defaultIntent, options: nil) ?? c
        if cache.count > 512 { cache.removeAll() }
        cache[c] = out
        return out
    }
}

/// Opaque versions of translucent theme colors, pre-composited over the canvas
/// for the current appearance. Opaque fills skip blending, which matters when
/// filling thousands of row backgrounds.
@MainActor
enum OpaqueColors {
    private static var cache: [ObjectIdentifier: CGColor] = [:]
    private static var appearanceKey = ""

    static func cg(_ color: NSColor, appearance: NSAppearance) -> CGColor {
        let key = appearance.name.rawValue + "|" + (TargetSpace.space.name as String? ?? "")
        if key != appearanceKey { cache.removeAll(); appearanceKey = key }
        if let c = cache[ObjectIdentifier(color)] { return c }
        var result = color.cgColor
        appearance.performAsCurrentDrawingAppearance {
            guard let fg = color.usingColorSpace(.sRGB), let bg = Theme.canvas.usingColorSpace(.sRGB) else { return }
            let a = fg.alphaComponent
            result = NSColor(srgbRed: fg.redComponent * a + bg.redComponent * (1 - a),
                             green: fg.greenComponent * a + bg.greenComponent * (1 - a),
                             blue: fg.blueComponent * a + bg.blueComponent * (1 - a), alpha: 1).cgColor
        }
        result = TargetSpace.convert(result)
        cache[ObjectIdentifier(color)] = result
        return result
    }
}

extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }
}

extension Color {
    static let ghCanvas = Color(nsColor: Theme.canvas)
    static let ghSubtle = Color(nsColor: Theme.subtle)
    static let ghBorder = Color(nsColor: Theme.border)
    static let ghText = Color(nsColor: Theme.text)
    static let ghMuted = Color(nsColor: Theme.mutedText)
    static let ghAccent = Color(nsColor: Theme.accent)
    static let ghAI = Color(nsColor: Theme.aiAccent)
    static let ghAIBg = Color(nsColor: Theme.aiBg)
    static let ghPendingBg = Color(nsColor: Theme.pendingBg)
    static let ghAdd = Color(nsColor: Theme.addMarker)
    static let ghDel = Color(nsColor: Theme.delMarker)
    static let ghAttention = Color(nsColor: Theme.attention)
}

/// Metrics shared by drawing and layout. Monospaced, so widths are arithmetic.
@MainActor
public final class DiffMetrics {
    public static let shared = DiffMetrics(fontSize: 12)
    public private(set) var font: NSFont
    public private(set) var boldFont: NSFont
    public private(set) var charWidth: CGFloat
    public private(set) var lineHeight: CGFloat
    public private(set) var ascent: CGFloat
    public let fileHeaderHeight: CGFloat = 46
    public let hunkHeaderHeight: CGFloat = 24
    public let placeholderHeight: CGFloat = 64
    public let markerWidth: CGFloat = 22
    public var version = 0

    init(fontSize: CGFloat) {
        font = .monospacedSystemFont(ofSize: fontSize, weight: .regular)
        boldFont = .monospacedSystemFont(ofSize: fontSize, weight: .semibold)
        charWidth = 0; lineHeight = 0; ascent = 0
        recompute()
    }

    public func setFontSize(_ size: CGFloat) {
        guard size != font.pointSize else { return }
        font = .monospacedSystemFont(ofSize: size, weight: .regular)
        boldFont = .monospacedSystemFont(ofSize: size, weight: .semibold)
        recompute()
        version += 1
    }

    private func recompute() {
        charWidth = ("M" as NSString).size(withAttributes: [.font: font]).width
        lineHeight = ceil(font.pointSize * 1.65)
        ascent = font.ascender
    }

    /// Width of a line-number column for numbers up to `maxLine`.
    public func numberColumnWidth(maxLine: Int) -> CGFloat {
        let digits = max(3, String(maxLine).count)
        return CGFloat(digits) * charWidth + 20
    }
}
