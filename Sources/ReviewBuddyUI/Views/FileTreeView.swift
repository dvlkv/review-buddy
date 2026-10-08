import SwiftUI
import ReviewBuddyCore

/// Flattened file tree rows (folders compressed like GitHub's "src/app/models").
public struct FileTreeRow: Identifiable, Equatable, Sendable {
    public var id: String
    public var depth: Int
    public var name: String
    public var isFolder: Bool
    public var fileIndex: Int?
    /// A "Review first" / "All files" label row.
    public var isSection = false
    /// Position in the "Review first" list (1-based).
    public var rank: Int?
}

public enum FileTreeBuilder {
    final class Node {
        var name: String
        var children: [String: Node] = [:]
        var order: [String] = []
        var fileIndex: Int?
        init(_ name: String) { self.name = name }
        func child(_ n: String) -> Node {
            if let c = children[n] { return c }
            let c = Node(n)
            children[n] = c
            order.append(n)
            return c
        }
    }

    /// File indices in display order: `priority` first, then the rest in tree order.
    public static func displayOrder(paths: [String], priority: [Int] = []) -> [Int] {
        priority + rows(paths: paths, excluding: Set(priority)).compactMap(\.fileIndex)
    }

    /// Builds visible rows. `collapsed` holds folder ids; `filter` is a case-insensitive substring.
    /// `priority` files are listed flat under "Review first", above the tree of the rest.
    public static func rows(paths: [String], filter: String = "", collapsed: Set<String> = [],
                            priority: [Int] = []) -> [FileTreeRow] {
        let q = filter.lowercased()
        guard !priority.isEmpty else { return rows(paths: paths, filter: q, collapsed: collapsed, excluding: []) }
        var out = [FileTreeRow(id: "§first", depth: 0, name: "Review first", isFolder: false, isSection: true)]
        for (n, i) in priority.enumerated() where q.isEmpty || paths[i].lowercased().contains(q) {
            out.append(FileTreeRow(id: "§first/" + paths[i], depth: 0, name: (paths[i] as NSString).lastPathComponent,
                                   isFolder: false, fileIndex: i, rank: n + 1))
        }
        out.append(FileTreeRow(id: "§all", depth: 0, name: "All files", isFolder: false, isSection: true))
        return out + rows(paths: paths, filter: q, collapsed: collapsed, excluding: Set(priority))
    }

    static func rows(paths: [String], filter q: String = "", collapsed: Set<String> = [], excluding: Set<Int>) -> [FileTreeRow] {
        let root = Node("")
        for (i, p) in paths.enumerated() where !excluding.contains(i) {
            if !q.isEmpty && !p.lowercased().contains(q) { continue }
            var n = root
            let parts = p.split(separator: "/").map(String.init)
            for part in parts.dropLast() { n = n.child(part + "/") }
            n.child(parts.last ?? p).fileIndex = i
        }
        var out: [FileTreeRow] = []
        func walk(_ node: Node, depth: Int, prefix: String) {
            // Folders first, then files, alphabetical — GitHub's order.
            let kids = node.order.map { node.children[$0]! }
            // Plain lexicographic order like GitHub ("a.go" before "a_test.go").
            let folders = kids.filter { $0.fileIndex == nil }.sorted { $0.name.lowercased() < $1.name.lowercased() }
            let files = kids.filter { $0.fileIndex != nil }.sorted { $0.name.lowercased() < $1.name.lowercased() }
            for var f in folders {
                var name = f.name
                // Compress single-child folder chains.
                while f.children.count == 1, let only = f.children.values.first, only.fileIndex == nil {
                    name += only.name
                    f = only
                }
                let id = prefix + name
                out.append(FileTreeRow(id: id, depth: depth, name: String(name.dropLast()), isFolder: true, fileIndex: nil))
                if !collapsed.contains(id) { walk(f, depth: depth + 1, prefix: id) }
            }
            for f in files {
                out.append(FileTreeRow(id: prefix + f.name, depth: depth, name: f.name, isFolder: false, fileIndex: f.fileIndex))
            }
        }
        walk(root, depth: 0, prefix: "")
        return out
    }
}

/// Opens the PR summary screen (conversation, commits, merge status, AI pre-review) in
/// place of the diff.
struct OverviewRow: View {
    let store: PRStore

    var body: some View {
        let conv = store.detail?.conversation
        let comments = (conv?.comments.count ?? 0) + store.threads.count
        let active = store.showOverview
        Button { store.showOverview.toggle() } label: {
            HStack(spacing: 8) {
                Image(systemName: "bubble.left.and.bubble.right").frame(width: 16)
                Text("Summary").fontWeight(.medium).lineLimit(1).fixedSize()
                Spacer(minLength: 4)
                if !store.openFindings.isEmpty {
                    Label("\(store.openFindings.count)", systemImage: "sparkles").foregroundStyle(Color.ghAI)
                        .help("Open AI pre-review findings")
                }
                if comments > 0 { Label("\(comments)", systemImage: "bubble.left").help("Comments and threads") }
                if let n = conv?.commits.count, n > 0 { Label("\(n)", systemImage: "point.3.connected.trianglepath.dotted").help("Commits") }
            }
            .labelStyle(.titleAndIcon)
            .font(.system(size: 12))
            .foregroundStyle(active ? Color.ghText : Color.ghMuted)
            .padding(.horizontal, 8).padding(.vertical, 6)
            // Opaque like the filter field: this sits under the toolbar's glass edge.
            .background {
                RoundedRectangle(cornerRadius: 6).fill(Color.ghCanvas)
                    .overlay(RoundedRectangle(cornerRadius: 6).fill(active ? Color.ghAccent.opacity(0.18) : Color.clear))
            }
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(active ? Color.ghAccent : Color.ghBorder))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Conversation, commits, merge status and AI pre-review")
    }
}

struct FileTreeView: View {
    let store: PRStore
    @State private var collapsed: Set<String> = []
    @State private var rows: [FileTreeRow] = []
    @State private var selection: String?

    var body: some View {
        VStack(spacing: 0) {
            OverviewRow(store: store)
                .padding(.horizontal, 8).padding(.top, 8)
            HStack(spacing: 6) {
                Image(systemName: "line.3.horizontal.decrease").foregroundStyle(Color.ghMuted)
                TextField("Filter files…", text: Bindable(store).fileFilter)
                    .textFieldStyle(.plain)
            }
            .font(.system(size: 12))
            .padding(.horizontal, 8).padding(.vertical, 6)
            .background(Color.ghCanvas, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            .padding(8)

            FileTreeList(store: store, rows: rows, collapsed: $collapsed, selection: $selection)
                .onChange(of: store.viewed) { }  // observed so the table refreshes viewed icons
                .onChange(of: store.openThreadCountByPath) { }
                .onChange(of: store.discussPaths) { }

            Divider()
            HStack {
                let total = store.files.count
                Text("\(store.viewedCount) / \(total) files viewed")
                Spacer()
                ProgressView(value: Double(store.viewedCount), total: Double(max(1, total)))
                    .progressViewStyle(.linear).frame(width: 60)
            }
            .font(.system(size: 11)).foregroundStyle(Color.ghMuted)
            .padding(.horizontal, 10).padding(.vertical, 7)
        }
        .onAppear(perform: rebuild)
        .onChange(of: store.fileFilter) { rebuild() }
        .onChange(of: collapsed) { rebuild() }
        // Any new diff (refresh, PR update), not just a different file count: an update
        // can add one file and drop another.
        .onChange(of: store.diff.map(ObjectIdentifier.init)) { rebuild() }
    }

    func rebuild() {
        rows = FileTreeBuilder.rows(paths: store.files.map(\.path), filter: store.fileFilter, collapsed: collapsed,
                                    priority: Array(0..<store.reviewFirstCount))
    }
}

/// The list itself: an AppKit table. SwiftUI's List diffs every row on each
/// selection change, which is too slow for PRs with thousands of files.
struct FileTreeList: NSViewRepresentable {
    let store: PRStore
    let rows: [FileTreeRow]
    @Binding var collapsed: Set<String>
    @Binding var selection: String?

    func makeCoordinator() -> FileTreeTableCoordinator { FileTreeTableCoordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let c = context.coordinator
        let table = NSTableView()
        table.headerView = nil
        table.style = .plain
        table.rowHeight = 24
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.backgroundColor = Theme.canvas
        table.selectionHighlightStyle = .regular
        table.focusRingType = .none
        table.addTableColumn(NSTableColumn(identifier: .init("file")))
        table.dataSource = c
        table.delegate = c
        table.target = c
        table.action = #selector(FileTreeTableCoordinator.clicked(_:))
        table.menu = c.makeMenu()
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = Theme.canvas
        c.table = table
        return scroll
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? 250, height: proposal.height ?? 400)
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        let c = context.coordinator
        c.store = store
        c.onToggleFolder = { id in
            if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
        }
        c.update(rows: rows, collapsed: collapsed, selection: selection,
                 viewed: store.viewed, counts: store.openThreadCountByPath, discuss: store.discussPaths)
        // Follow the diff's scroll position directly, outside SwiftUI.
        store.onCurrentFileChange["fileTree"] = { [weak c] f in c?.follow(file: f) }
        c.follow(file: store.currentFile)
    }
}

@MainActor
final class FileTreeTableCoordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSMenuDelegate {
    weak var table: NSTableView?
    weak var store: PRStore?
    var onToggleFolder: ((String) -> Void)?
    private var models: [FileTreeRowView.Model] = []
    private var suppressSelection = false

    func update(rows: [FileTreeRow], collapsed: Set<String>, selection: String?, viewed: Set<String>,
                counts: [String: Int], discuss: Set<String>) {
        let files = store?.files ?? []
        let new = rows.map { row -> FileTreeRowView.Model in
            let file = row.fileIndex.flatMap { files[safe: $0] }
            return .init(row: row, collapsed: collapsed.contains(row.id), status: file?.status,
                         viewed: file.map { viewed.contains($0.path) } ?? false,
                         threads: file.map { counts[$0.path] ?? 0 } ?? 0,
                         hasDiscuss: file.map { discuss.contains($0.path) } ?? false)
        }
        guard let table else { return }
        if new.map(\.row.id) != models.map(\.row.id) {
            models = new
            table.reloadData()
        } else if new != models {
            // Same rows: reload only the ones whose state changed.
            let changed = IndexSet(new.indices.filter { new[$0] != models[$0] })
            models = new
            table.reloadData(forRowIndexes: changed, columnIndexes: IndexSet(integer: 0))
        }
        rowForFile = [:]
        for (i, m) in models.enumerated() { if let f = m.row.fileIndex { rowForFile[f] = i } }
    }

    private var rowForFile: [Int: Int] = [:]

    /// Highlights the file currently at the top of the diff.
    func follow(file: Int) {
        guard let table, let i = rowForFile[file], table.selectedRow != i else { return }
        suppressSelection = true
        table.selectRowIndexes(IndexSet(integer: i), byExtendingSelection: false)
        // Only scroll the tree when the row is off-screen (cheap check first). Near the top,
        // show the very top so the "Review first" label above the first file stays visible.
        if i <= 1 { table.scroll(.zero) }
        else if !table.visibleRect.intersects(table.rect(ofRow: i)) { table.scrollRowToVisible(i) }
        suppressSelection = false
    }

    func numberOfRows(in tableView: NSTableView) -> Int { models.count }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        (tableView.makeView(withIdentifier: FileTreeRowBackground.id, owner: nil) as? FileTreeRowBackground) ?? FileTreeRowBackground()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("treeRow")
        let cell = (tableView.makeView(withIdentifier: id, owner: nil) as? FileTreeCell) ?? FileTreeCell(identifier: id)
        cell.configure(models[row])
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        row < models.count && !models[row].row.isSection
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !suppressSelection, let table, table.selectedRow >= 0, table.selectedRow < models.count else { return }
        let m = models[table.selectedRow]
        if let f = m.row.fileIndex { store?.scrollToFile(f) }
    }

    @objc func clicked(_ sender: NSTableView) {
        let r = sender.clickedRow
        guard r >= 0, r < models.count, models[r].row.isFolder else { return }
        onToggleFolder?(models[r].row.id)
    }

    func makeMenu() -> NSMenu {
        let m = NSMenu()
        m.delegate = self
        return m
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        guard let table, let store, table.clickedRow >= 0, table.clickedRow < models.count,
              let f = models[table.clickedRow].row.fileIndex else { return }
        let path = store.files[f].path
        if let rank = store.reviewOrder.firstIndex(of: path) {
            if rank > 0 { menu.addItem(ClosureMenuItem("Move up") { store.moveInReviewOrder(path, by: -1) }) }
            if rank < store.reviewOrder.count - 1 { menu.addItem(ClosureMenuItem("Move down") { store.moveInReviewOrder(path, by: 1) }) }
            menu.addItem(ClosureMenuItem("Remove from Review first") { store.removeFromReviewOrder(path) })
        } else {
            menu.addItem(ClosureMenuItem("Review first") { store.addToReviewOrder(path) })
        }
        if store.customReviewOrder != nil, store.summary?.readingOrder.isEmpty == false {
            menu.addItem(ClosureMenuItem("Reset to AI reading order") { store.resetReviewOrder() })
        }
        menu.addItem(.separator())
        let viewed = store.isViewed(f)
        menu.addItem(ClosureMenuItem(viewed ? "Mark as not viewed" : "Mark as viewed") { store.toggleViewed(f) })
        menu.addItem(ClosureMenuItem("Copy path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(store.files[f].path, forType: .string)
        })
    }
}

final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void
    init(_ title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError() }
    @objc private func fire() { handler() }
}

/// GitHub-style selection: a soft accent rounded rect, same in key and non-key windows.
final class FileTreeRowBackground: NSTableRowView {
    static let id = NSUserInterfaceItemIdentifier("treeRowBg")
    override init(frame: NSRect) { super.init(frame: frame); identifier = Self.id }
    required init?(coder: NSCoder) { fatalError() }
    override func drawSelection(in dirtyRect: NSRect) {
        Theme.accent.withAlphaComponent(0.14).setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 6, dy: 1), xRadius: 6, yRadius: 6).fill()
        Theme.accent.setFill()
        NSBezierPath(roundedRect: CGRect(x: 6, y: 5, width: 3, height: bounds.height - 10), xRadius: 1.5, yRadius: 1.5).fill()
    }
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
}

/// Drawn file tree cell (no subviews, no Auto Layout).
final class FileTreeCell: NSTableCellView {
    private var model: FileTreeRowView.Model?
    private static var symbols: [String: NSImage] = [:]

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func configure(_ m: FileTreeRowView.Model) {
        guard m != model else { return }
        model = m
        needsDisplay = true
    }

    override var backgroundStyle: NSView.BackgroundStyle { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        guard let m = model else { return }
        let selected = false
        var x = 4 + CGFloat(m.row.depth) * 12
        let midY = bounds.midY
        if m.row.isSection {
            let t = NSAttributedString(string: m.row.name.uppercased(), attributes: [
                .font: NSFont.systemFont(ofSize: 10, weight: .semibold), .foregroundColor: Theme.mutedText, .kern: 0.4])
            t.draw(at: CGPoint(x: x + 4, y: midY - t.size().height / 2 + 2))
            return
        }
        if let rank = m.row.rank {
            let t = NSAttributedString(string: "\(rank)", attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .semibold), .foregroundColor: Theme.aiAccent])
            // Clear of the selection marker drawn at x 6–9.
            t.draw(at: CGPoint(x: x + 12 - t.size().width / 2, y: midY - t.size().height / 2))
            x += 18
        }
        let iconName: String
        let iconColor: NSColor
        let textColor: NSColor
        if m.row.isFolder {
            iconName = m.collapsed ? "folder" : "folder.fill"
            iconColor = Theme.mutedText; textColor = Theme.mutedText
        } else {
            let status = m.status ?? .modified
            iconName = m.viewed ? "checkmark.circle.fill" : FileTreeRowView.icon(status)
            iconColor = m.viewed ? Theme.accent : NSColor(FileTreeRowView.color(status))
            textColor = m.viewed ? Theme.mutedText : Theme.text
        }
        drawSymbol(iconName, center: CGPoint(x: x + 8, y: midY), color: selected ? .white : iconColor, size: 11)
        x += 21
        var right = bounds.width - 6
        if m.hasDiscuss {
            drawSymbol("sparkle", center: CGPoint(x: right - 5, y: midY), color: selected ? .white : Theme.aiAccent, size: 9)
            right -= 14
        }
        if m.threads > 0 {
            let t = NSAttributedString(string: "\(m.threads)", attributes: [.font: NSFont.systemFont(ofSize: 10),
                                                                          .foregroundColor: selected ? NSColor.white : Theme.mutedText])
            right -= t.size().width
            t.draw(at: CGPoint(x: right, y: midY - t.size().height / 2))
            drawSymbol("bubble.left.fill", center: CGPoint(x: right - 7, y: midY), color: selected ? .white : Theme.mutedText, size: 8)
            right -= 18
        }
        let p = NSMutableParagraphStyle()
        p.lineBreakMode = .byTruncatingMiddle
        let name = NSAttributedString(string: m.row.name, attributes: [
            .font: NSFont.systemFont(ofSize: 12), .foregroundColor: selected ? NSColor.white : textColor, .paragraphStyle: p])
        let h = name.size().height
        name.draw(in: CGRect(x: x, y: midY - h / 2, width: max(0, right - x - 4), height: h))
    }

    private func drawSymbol(_ name: String, center: CGPoint, color: NSColor, size: CGFloat) {
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let key = "\(name)|\(size)|\(ObjectIdentifier(color).hashValue)|\(dark)"
        let img: NSImage
        if let c = Self.symbols[key] { img = c } else {
            guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: size, weight: .regular)) else { return }
            var resolved = color
            effectiveAppearance.performAsCurrentDrawingAppearance { resolved = NSColor(cgColor: color.cgColor) ?? color }
            img = NSImage(size: base.size, flipped: false) { r in
                base.draw(in: r); resolved.set(); r.fill(using: .sourceAtop); return true
            }
            Self.symbols[key] = img
        }
        img.draw(in: CGRect(x: center.x - img.size.width / 2, y: center.y - img.size.height / 2,
                            width: img.size.width, height: img.size.height),
                 from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }
}

struct FileTreeRowView: View, Equatable {
    struct Model: Equatable {
        var row: FileTreeRow
        var collapsed: Bool
        var status: FileStatus?
        var viewed: Bool
        var threads: Int
        var hasDiscuss: Bool
    }
    let model: Model

    var body: some View {
        let row = model.row
        HStack(spacing: 5) {
            if row.isFolder {
                Image(systemName: model.collapsed ? "chevron.right" : "chevron.down")
                    .font(.system(size: 9, weight: .bold)).foregroundStyle(Color.ghMuted).frame(width: 10)
                Image(systemName: "folder.fill").foregroundStyle(Color.ghMuted.opacity(0.8)).font(.system(size: 11))
                Text(row.name).lineLimit(1).truncationMode(.middle)
            } else if let status = model.status {
                Image(systemName: model.viewed ? "checkmark.circle.fill" : Self.icon(status))
                    .foregroundStyle(model.viewed ? Color.ghAccent : Self.color(status))
                    .font(.system(size: 11)).frame(width: 14)
                Text(row.name).lineLimit(1).truncationMode(.middle)
                    .foregroundStyle(model.viewed ? Color.ghMuted : Color.ghText)
                Spacer(minLength: 4)
                if model.threads > 0 {
                    HStack(spacing: 2) { Image(systemName: "bubble.left.fill"); Text("\(model.threads)") }
                        .font(.system(size: 9)).foregroundStyle(Color.ghMuted)
                }
                if model.hasDiscuss {
                    Image(systemName: "sparkle").font(.system(size: 9)).foregroundStyle(Color.ghAI)
                }
            }
            Spacer(minLength: 0)
        }
        .font(.system(size: 12))
        .padding(.leading, CGFloat(row.depth) * 12)
        .padding(.vertical, 2)
    }

    static func icon(_ s: FileStatus) -> String {
        switch s {
        case .added: "plus.square.fill"
        case .deleted: "minus.square.fill"
        case .renamed, .copied: "arrow.right.square.fill"
        case .modified: "doc"
        }
    }

    static func color(_ s: FileStatus) -> Color {
        switch s {
        case .added: .ghAdd
        case .deleted: .ghDel
        case .renamed, .copied: .ghAttention
        case .modified: .ghMuted
        }
    }
}
