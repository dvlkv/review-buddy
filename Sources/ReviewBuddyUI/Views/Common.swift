import SwiftUI
import ReviewBuddyCore

/// Cached remote avatar (GitHub serves small PNGs; NSCache keeps them across rows).
struct Avatar: View {
    let url: URL?
    var size: CGFloat = 20
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image).resizable()
            } else {
                Circle().fill(Color.ghBorder)
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .task(id: url) {
            guard let url else { return }
            image = await AvatarCache.shared.image(for: url, size: size)
        }
    }
}

@MainActor
final class AvatarCache {
    static let shared = AvatarCache()
    private let cache = NSCache<NSURL, NSImage>()
    private var inflight: [URL: Task<NSImage?, Never>] = [:]

    func image(for url: URL, size: CGFloat) async -> NSImage? {
        var comps = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let items = comps?.queryItems ?? []
        comps?.queryItems = items + [URLQueryItem(name: "s", value: "\(Int(size * 2))")]
        let u = comps?.url ?? url
        if let i = cache.object(forKey: u as NSURL) { return i }
        if let t = inflight[u] { return await t.value }
        let t = Task<NSImage?, Never> {
            guard let (data, _) = try? await URLSession.shared.data(from: u) else { return nil }
            return NSImage(data: data)
        }
        inflight[u] = t
        let img = await t.value
        inflight[u] = nil
        if let img { cache.setObject(img, forKey: u as NSURL) }
        return img
    }
}

/// GitHub-flavored Markdown rendering for comment bodies.
struct MarkdownView: View {
    let text: String
    var suggestionContext: [String] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(MarkdownParser.parse(text).enumerated()), id: \.offset) { _, block in
                blockView(block)
            }
        }
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    func inline(_ s: String) -> Text {
        if let a = try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)) {
            return Text(a)
        }
        return Text(s)
    }

    @ViewBuilder
    func blockView(_ b: MarkdownBlock) -> some View {
        switch b {
        case .paragraph(let s):
            inline(s).font(.system(size: 13)).foregroundStyle(Color.ghText).fixedSize(horizontal: false, vertical: true)
        case .heading(let level, let s):
            inline(s).font(.system(size: level <= 2 ? 16 : 14, weight: .semibold))
        case .code(_, let s):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(s).font(.system(size: 12, design: .monospaced)).padding(10)
            }
            .background(Color.ghSubtle, in: RoundedRectangle(cornerRadius: 6))
        case .suggestion(let s):
            SuggestionPreview(old: suggestionContext, new: s.components(separatedBy: "\n"))
        case .quote(let s):
            HStack(spacing: 8) {
                Rectangle().fill(Color.ghBorder).frame(width: 3)
                inline(s).foregroundStyle(Color.ghMuted).font(.system(size: 13))
            }
        case .list(let ordered, let items):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(ordered ? "\(i + 1)." : "•").foregroundStyle(Color.ghMuted)
                        inline(item).fixedSize(horizontal: false, vertical: true)
                    }.font(.system(size: 13))
                }
            }
        case .taskList(let items):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(spacing: 6) {
                        Image(systemName: item.done ? "checkmark.square.fill" : "square")
                            .foregroundStyle(item.done ? Color.ghAccent : Color.ghMuted)
                        inline(item.text)
                    }.font(.system(size: 13))
                }
            }
        case .rule:
            Divider()
        }
    }
}

/// GitHub's "Suggested change" box: removed lines, then proposed lines.
struct SuggestionPreview: View {
    let old: [String]
    let new: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Image(systemName: "plus.forwardslash.minus")
                Text("Suggested change").fontWeight(.semibold)
                Spacer()
            }
            .font(.system(size: 12))
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(Color.ghSubtle)
            Divider()
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(old.enumerated()), id: \.offset) { _, l in line("-", l, Color(nsColor: Theme.delBg)) }
                ForEach(Array(new.enumerated()), id: \.offset) { _, l in line("+", l, Color(nsColor: Theme.addBg)) }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
    }

    func line(_ marker: String, _ text: String, _ bg: Color) -> some View {
        HStack(spacing: 6) {
            Text(marker).foregroundStyle(marker == "+" ? Color.ghAdd : Color.ghDel)
            Text(text.isEmpty ? " " : text)
            Spacer(minLength: 0)
        }
        .font(.system(size: 12, design: .monospaced))
        .padding(.horizontal, 10).padding(.vertical, 1)
        .background(bg)
    }
}

struct Pill: View {
    let text: String
    var color: Color = .ghMuted
    var filled = false
    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 7).padding(.vertical, 2)
            .foregroundStyle(filled ? Color.white : color)
            .background(filled ? color : Color.clear, in: Capsule())
            .overlay(Capsule().stroke(color.opacity(filled ? 0 : 0.6)))
    }
}

struct DiffStat: View {
    let additions: Int
    let deletions: Int
    var body: some View {
        HStack(spacing: 4) {
            Text("+\(additions)").foregroundStyle(Color.ghAdd)
            Text("−\(deletions)").foregroundStyle(Color.ghDel)
            HStack(spacing: 1) {
                let total = max(1, additions + deletions)
                let greens = Int((Double(additions) / Double(total) * 5).rounded())
                ForEach(0..<5, id: \.self) { i in
                    RoundedRectangle(cornerRadius: 1)
                        .fill(i < greens ? Color.ghAdd : (additions + deletions == 0 ? Color.ghBorder : Color.ghDel))
                        .frame(width: 7, height: 7)
                }
            }
        }
        .font(.system(size: 11, weight: .semibold, design: .monospaced))
    }
}

extension Date {
    var relative: String {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f.localizedString(for: self, relativeTo: Date())
    }
}

/// Button style matching GitHub's buttons.
struct GHButtonStyle: ButtonStyle {
    enum Kind { case normal, primary, danger, ai, invisible }
    var kind: Kind = .normal
    var compact = false

    func makeBody(configuration: Configuration) -> some View {
        StyledBody(configuration: configuration, style: self)
    }

    private struct StyledBody: View {
        let configuration: Configuration
        let style: GHButtonStyle
        @Environment(\.isEnabled) private var isEnabled
        var body: some View {
            style.styled(configuration).opacity(isEnabled ? 1 : 0.45)
        }
    }

    func styled(_ configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: compact ? 12 : 13, weight: .medium))
            .padding(.horizontal, compact ? 8 : 12)
            .padding(.vertical, compact ? 3 : 5)
            .foregroundStyle(foreground)
            .background(background(configuration.isPressed), in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(border, lineWidth: kind == .invisible ? 0 : 1))
            .contentShape(Rectangle())
    }

    var foreground: Color {
        switch kind {
        case .primary, .ai: .white
        case .danger: .ghDel
        case .normal: .ghText
        case .invisible: .ghMuted
        }
    }

    func background(_ pressed: Bool) -> Color {
        let base: Color = switch kind {
        case .primary: Color(nsColor: NSColor(hex: 0x1F883D))
        case .ai: .ghAI
        case .normal, .danger: .ghSubtle
        case .invisible: .clear
        }
        return pressed ? base.opacity(0.8) : base
    }

    var border: Color {
        switch kind {
        case .primary, .ai: .black.opacity(0.1)
        default: .ghBorder
        }
    }
}
