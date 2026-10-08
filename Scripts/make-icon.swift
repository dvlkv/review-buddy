// Draws the app icon and writes Resources/AppIcon.icns.
// Usage: swift Scripts/make-icon.swift   (from the repo root)
import AppKit
import CoreGraphics

let size = 1024.0
let space = CGColorSpace(name: CGColorSpace.displayP3)!

func color(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [CGFloat((hex >> 16) & 0xFF) / 255, CGFloat((hex >> 8) & 0xFF) / 255,
                                            CGFloat(hex & 0xFF) / 255, a])!
}

func render(_ px: Int) -> CGImage {
    let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let s = CGFloat(px) / size
    ctx.scaleBy(x: s, y: s)

    // macOS icon grid: 824 pt body inset 100 pt, with a soft drop shadow.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let squircle = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: color(0x000000, 0.35))
    ctx.addPath(squircle); ctx.setFillColor(color(0x24124F)); ctx.fillPath()
    ctx.restoreGState()

    // Background: indigo → violet diagonal.
    ctx.saveGState()
    ctx.addPath(squircle); ctx.clip()
    let bg = CGGradient(colorsSpace: space, colors: [color(0x2B2F8F), color(0x6E3FE0), color(0xA45CFF)] as CFArray,
                        locations: [0, 0.6, 1])!
    ctx.drawLinearGradient(bg, start: CGPoint(x: 100, y: 100), end: CGPoint(x: 924, y: 924), options: [])
    // Soft top highlight.
    let hi = CGGradient(colorsSpace: space, colors: [color(0xFFFFFF, 0.18), color(0xFFFFFF, 0)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(hi, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 560), options: [])
    ctx.restoreGState()

    // Diff card.
    let card = CGRect(x: 200, y: 250, width: 520, height: 560)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 30, color: color(0x10062E, 0.45))
    ctx.addPath(CGPath(roundedRect: card, cornerWidth: 56, cornerHeight: 56, transform: nil))
    ctx.setFillColor(color(0xFFFFFF)); ctx.fillPath()
    ctx.restoreGState()

    // Diff lines: context, removed, added, added, context.
    struct Line { var bg: UInt32?; var bar: UInt32; var width: CGFloat; var mark: String? }
    let lines = [
        Line(bg: nil, bar: 0xC9CDD6, width: 300, mark: nil),
        Line(bg: 0xFFE3E3, bar: 0xE5534B, width: 340, mark: "−"),
        Line(bg: 0xDDF7E3, bar: 0x2DA44E, width: 260, mark: "+"),
        Line(bg: 0xDDF7E3, bar: 0x2DA44E, width: 330, mark: "+"),
        Line(bg: nil, bar: 0xC9CDD6, width: 220, mark: nil),
    ]
    ctx.saveGState()
    ctx.addPath(CGPath(roundedRect: card, cornerWidth: 56, cornerHeight: 56, transform: nil)); ctx.clip()
    let rowH: CGFloat = 84, top = card.maxY - 64
    for (i, l) in lines.enumerated() {
        let y = top - CGFloat(i + 1) * rowH
        if let b = l.bg { ctx.setFillColor(color(b)); ctx.fill(CGRect(x: card.minX, y: y, width: card.width, height: rowH)) }
        let bar = CGRect(x: card.minX + 110, y: y + rowH / 2 - 14, width: l.width, height: 28)
        ctx.addPath(CGPath(roundedRect: bar, cornerWidth: 14, cornerHeight: 14, transform: nil))
        ctx.setFillColor(color(l.bar, l.bg == nil ? 1 : 0.85)); ctx.fillPath()
        if let m = l.mark {
            let attr = NSAttributedString(string: m, attributes: [
                .font: NSFont.systemFont(ofSize: 56, weight: .heavy),
                .foregroundColor: NSColor(cgColor: color(l.bar))!])
            let line = CTLineCreateWithAttributedString(attr)
            let w = CTLineGetTypographicBounds(line, nil, nil, nil)
            ctx.textPosition = CGPoint(x: card.minX + 58 - w / 2, y: y + rowH / 2 - 20)
            CTLineDraw(line, ctx)
        }
    }
    ctx.restoreGState()

    // Chat bubble with a sparkle: the AI buddy.
    let bubble = CGRect(x: 560, y: 190, width: 290, height: 250)
    let bp = CGMutablePath()
    bp.addRoundedRect(in: bubble, cornerWidth: 90, cornerHeight: 90)
    bp.move(to: CGPoint(x: bubble.minX + 70, y: bubble.minY + 30))
    bp.addLine(to: CGPoint(x: bubble.minX + 10, y: bubble.minY - 50))
    bp.addLine(to: CGPoint(x: bubble.minX + 150, y: bubble.minY + 8))
    bp.closeSubpath()
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 26, color: color(0x10062E, 0.5))
    ctx.addPath(bp)
    ctx.setFillColor(color(0xFFFFFF)); ctx.fillPath()
    ctx.restoreGState()

    // Four-point sparkle.
    func sparkle(center c: CGPoint, r: CGFloat, fill: CGColor) {
        let p = CGMutablePath()
        let k = r * 0.28
        p.move(to: CGPoint(x: c.x, y: c.y + r))
        p.addQuadCurve(to: CGPoint(x: c.x + r, y: c.y), control: CGPoint(x: c.x + k, y: c.y + k))
        p.addQuadCurve(to: CGPoint(x: c.x, y: c.y - r), control: CGPoint(x: c.x + k, y: c.y - k))
        p.addQuadCurve(to: CGPoint(x: c.x - r, y: c.y), control: CGPoint(x: c.x - k, y: c.y - k))
        p.addQuadCurve(to: CGPoint(x: c.x, y: c.y + r), control: CGPoint(x: c.x - k, y: c.y + k))
        ctx.addPath(p); ctx.setFillColor(fill); ctx.fillPath()
    }
    sparkle(center: CGPoint(x: bubble.midX - 18, y: bubble.midY - 6), r: 78, fill: color(0x7C3AED))
    sparkle(center: CGPoint(x: bubble.midX + 66, y: bubble.midY + 58), r: 30, fill: color(0xA45CFF))
    return ctx.makeImage()!
}

func png(_ img: CGImage, to url: URL) {
    let rep = NSBitmapImageRep(cgImage: img)
    try! rep.representation(using: .png, properties: [:])!.write(to: url)
}

let fm = FileManager.default
let root = URL(fileURLWithPath: fm.currentDirectoryPath)
let iconset = fm.temporaryDirectory.appendingPathComponent("AppIcon-\(UUID().uuidString).iconset")
try! fm.createDirectory(at: iconset, withIntermediateDirectories: true)
for pt in [16, 32, 128, 256, 512] {
    png(render(pt), to: iconset.appendingPathComponent("icon_\(pt)x\(pt).png"))
    png(render(pt * 2), to: iconset.appendingPathComponent("icon_\(pt)x\(pt)@2x.png"))
}
let res = root.appendingPathComponent("Resources")
try! fm.createDirectory(at: res, withIntermediateDirectories: true)
png(render(1024), to: res.appendingPathComponent("AppIcon.png"))
let p = Process()
p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
p.arguments = ["-c", "icns", iconset.path, "-o", res.appendingPathComponent("AppIcon.icns").path]
try! p.run(); p.waitUntilExit()
try? fm.removeItem(at: iconset)
print(p.terminationStatus == 0 ? "Resources/AppIcon.icns" : "iconutil failed")
