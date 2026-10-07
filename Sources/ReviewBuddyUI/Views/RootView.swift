import SwiftUI
import ReviewBuddyCore

public struct RootView: View {
    @Bindable var app: AppModel
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    public init(app: AppModel) { self.app = app }

    public var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            Group {
                if let inbox = app.inbox { SidebarView(app: app, inbox: inbox) } else { ProgressView() }
            }
            .navigationSplitViewColumnWidth(min: 220, ideal: 280, max: 400)
        } content: {
            Group {
                if let pr = app.current, pr.diff != nil { FileTreeView(store: pr) }
                else { Color.clear }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 250, max: 420)
        } detail: {
            detail
        }
        .inspector(isPresented: $app.showDiscussPanel) {
            Group {
                if let pr = app.current { DiscussPanel(store: pr) } else { Color.ghCanvas }
            }
            .inspectorColumnWidth(min: 300, ideal: 380, max: 640)
        }
        .toolbar { toolbar }
        .sheet(isPresented: $app.showOpenURL) { OpenURLSheet(app: app) }
        .sheet(isPresented: $app.showPalette) { CommandPalette(app: app) }
        .sheet(isPresented: $app.showPushSheet) {
            if let pr = app.current { PushSheet(store: pr, isPresented: $app.showPushSheet) }
        }
        .onReceive(NotificationCenter.default.publisher(for: .rbShowPush)) { _ in app.showPushSheet = true }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            app.inbox?.refreshIfStale()
        }
        .onChange(of: app.current?.activeDiscussId) { _, new in if new != nil { app.showDiscussPanel = true } }
        .navigationTitle(app.current?.detail?.title ?? "Review Buddy")
        .navigationSubtitle(app.current.map { $0.ref.description } ?? "")
    }

    @ViewBuilder
    var detail: some View {
        if let pr = app.current {
            switch pr.loadState {
            case .failed(let e):
                ContentUnavailableView {
                    Label("Couldn't load \(pr.ref.description)", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(e)
                } actions: {
                    Button("Retry") { Task { await pr.load() } }
                }
            case .loading(let msg) where pr.diff == nil:
                VStack(spacing: 10) {
                    ProgressView()
                    Text(msg).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            default:
                VStack(spacing: 0) {
                    PRHeaderView(store: pr)
                    Divider()
                    DiffView(store: pr)
                }
                .background(Color.ghCanvas)
            }
        } else {
            ContentUnavailableView {
                Label("No pull request selected", systemImage: "arrow.triangle.pull")
            } description: {
                Text("Pick a PR from the inbox, or press ⌘L to open one by URL.")
            } actions: {
                Button("Open by URL…") { app.showOpenURL = true }
            }
        }
    }

    /// Each toolbar item is its own view so a change to one store property only
    /// re-evaluates that item, not the whole window (matters for split toggles).
    @ToolbarContentBuilder
    var toolbar: some ToolbarContent {
        if let pr = app.current, pr.diff != nil {
            ToolbarItemGroup(placement: .principal) {
                ModePicker(store: pr)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                SummarizeButton(store: pr)
                PreReviewButton(store: pr)
                LocalCommitsButton(app: app, store: pr)
                ReviewButton(app: app, store: pr)
                Button { app.showDiscussPanel.toggle() } label: { Label("Discuss", systemImage: "sidebar.right") }
                    .help("Toggle Discuss panel (⌘\\)")
            }
        }
    }

}

struct ModePicker: View {
    let store: PRStore
    var body: some View {
        Picker("Diff mode", selection: Binding(get: { store.viewMode }, set: { store.setMode($0) })) {
            Label("Unified", systemImage: "rectangle").tag(DiffViewMode.unified)
            Label("Split", systemImage: "rectangle.split.2x1").tag(DiffViewMode.split)
        }
        .pickerStyle(.segmented)
        .help("Toggle split / unified (S)")
    }
}

struct SummarizeButton: View {
    let store: PRStore
    var body: some View {
        Button { Task { await store.summarize() } } label: {
            if store.summaryState.isRunning { ProgressView().controlSize(.small) }
            else { Label("Summarize", systemImage: "text.alignleft") }
        }
        .help("AI summary and reading order")
        .disabled(store.summaryState.isRunning)
    }
}

struct PreReviewButton: View {
    let store: PRStore
    var body: some View {
        Button { Task { await store.preReview() } } label: {
            if store.preReviewState.isRunning { ProgressView().controlSize(.small) }
            else {
                Label(store.openFindings.isEmpty ? "AI pre-review" : "AI pre-review (\(store.openFindings.count))",
                      systemImage: "sparkles")
            }
        }
        .help(help)
        .disabled(store.preReviewState.isRunning)
    }

    var help: String {
        if case .failed(let e) = store.preReviewState { return "Pre-review failed: \(e)" }
        if case .running(let m) = store.preReviewState { return m }
        return "Ask Claude to review the whole PR and leave private suggested comments"
    }
}

struct LocalCommitsButton: View {
    @Bindable var app: AppModel
    let store: PRStore
    var body: some View {
        if !store.localCommits.isEmpty {
            Button { app.showPushSheet = true } label: {
                Label("\(store.localCommits.count) local", systemImage: "arrow.up.circle")
            }
            .help("Push fix commits to the PR branch")
        }
    }
}

struct ReviewButton: View {
    @Bindable var app: AppModel
    let store: PRStore

    var body: some View {
        Button { app.showSubmitReview.toggle() } label: {
            HStack(spacing: 5) {
                Text("Review")
                if store.pendingCommentCount > 0 {
                    Text("\(store.pendingCommentCount)")
                        .font(.system(size: 10, weight: .bold))
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Color.ghAttention.opacity(0.25), in: Capsule())
                }
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold))
            }
        }
        .popover(isPresented: $app.showSubmitReview, arrowEdge: .bottom) {
            ReviewSubmitView(store: store, isPresented: $app.showSubmitReview)
        }
        .help("Finish your review")
    }
}

struct OpenURLSheet: View {
    let app: AppModel
    @State private var text = ""
    @State private var error: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Open pull request").font(.system(size: 15, weight: .semibold))
            TextField("https://github.com/owner/repo/pull/123  or  owner/repo#123", text: $text)
                .textFieldStyle(.roundedBorder)
                .onSubmit(open)
            if let error { Text(error).foregroundStyle(Color.ghDel).font(.system(size: 12)) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Open", action: open).keyboardShortcut(.defaultAction).disabled(text.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 480)
        .onAppear {
            if let s = NSPasteboard.general.string(forType: .string), PRRef(string: s) != nil { text = s }
        }
    }

    func open() {
        guard let ref = PRRef(string: text) else { error = "That doesn't look like a pull request URL."; return }
        dismiss()
        Task { await app.open(ref) }
    }
}

/// ⌘K palette over actions, files and inbox PRs.
struct CommandPalette: View {
    let app: AppModel
    @State private var query = ""
    @State private var selected = 0
    @Environment(\.dismiss) private var dismiss

    struct Item: Identifiable {
        var id: String
        var title: String
        var subtitle: String?
        var icon: String
        var shortcut: String?
        var run: @MainActor () -> Void
    }

    var items: [Item] {
        var out: [Item] = []
        let pr = app.current
        for a in ReviewAction.allCases where a != .commandPalette && a != .cancel {
            out.append(Item(id: "a:\(a.rawValue)", title: a.title, icon: "command", shortcut: a.shortcut) {
                perform(a)
            })
        }
        if let pr {
            for (i, f) in pr.files.enumerated() {
                out.append(Item(id: "f:\(i)", title: (f.path as NSString).lastPathComponent, subtitle: f.path, icon: "doc") {
                    pr.scrollToFile(i)
                })
            }
        }
        for s in app.inbox?.sections ?? [] {
            for p in s.items {
                out.append(Item(id: "p:\(s.id):\(p.id)", title: p.title, subtitle: p.ref.description, icon: "arrow.triangle.pull") {
                    Task { await app.open(p.ref) }
                })
            }
        }
        return out
    }

    var filtered: [Item] {
        if query.isEmpty { return Array(items.prefix(60)) }
        return items.compactMap { item -> (Int, Item)? in
            let s1 = KeyBindings.fuzzyScore(query, item.title)
            let s2 = item.subtitle.flatMap { KeyBindings.fuzzyScore(query, $0) }.map { $0 - 50 }
            guard let s = [s1, s2].compactMap({ $0 }).max() else { return nil }
            return (s, item)
        }
        .sorted { $0.0 > $1.0 }
        .prefix(60)
        .map(\.1)
    }

    var body: some View {
        let list = filtered
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Type a command, file or PR…", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 15))
                    .onSubmit { run(list) }
                    .onKeyPress(.downArrow) { selected = min(list.count - 1, selected + 1); return .handled }
                    .onKeyPress(.upArrow) { selected = max(0, selected - 1); return .handled }
            }
            .padding(12)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(list.enumerated()), id: \.element.id) { i, item in
                            HStack(spacing: 10) {
                                Image(systemName: item.icon).frame(width: 18).foregroundStyle(.secondary)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(item.title).lineLimit(1)
                                    if let s = item.subtitle { Text(s).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1) }
                                }
                                Spacer()
                                if let k = item.shortcut {
                                    Text(k).font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary)
                                }
                            }
                            .padding(.horizontal, 12).padding(.vertical, 6)
                            .background(i == selected ? Color.ghAccent.opacity(0.15) : .clear)
                            .contentShape(Rectangle())
                            .onTapGesture { selected = i; run(list) }
                            .id(i)
                        }
                    }
                }
                .onChange(of: selected) { proxy.scrollTo(selected) }
            }
            .frame(height: 340)
        }
        .frame(width: 560)
        .onChange(of: query) { selected = 0 }
        .onExitCommand { dismiss() }
    }

    func run(_ list: [Item]) {
        guard selected < list.count else { return }
        let item = list[selected]
        dismiss()
        DispatchQueue.main.async { item.run() }
    }

    func perform(_ a: ReviewAction) {
        let pr = app.current
        switch a {
        case .openByURL: app.showOpenURL = true
        case .toggleDiscussPanel: app.showDiscussPanel.toggle()
        case .submit: app.showSubmitReview = true
        case .summarize: if let pr { Task { await pr.summarize() } }
        case .preReview: if let pr { Task { await pr.preReview() } }
        case .refresh: Task { await app.reloadCurrent() }
        case .pushFixes: app.showPushSheet = true
        case .toggleSplit: pr?.toggleMode()
        case .toggleViewed: if let pr { pr.toggleViewed(pr.currentFile) }
        case .collapseFile: if let pr { pr.toggleCollapsed(pr.currentFile) }
        case .comment: pr?.startComment()
        case .discuss: pr?.startDiscuss()
        default: pr?.navigate(a)
        }
    }
}
