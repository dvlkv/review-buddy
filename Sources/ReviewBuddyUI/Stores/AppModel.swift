import AppKit
import Foundation
import Observation
import ReviewBuddyCore
import UserNotifications

@MainActor
@Observable
public final class InboxStore {
    public struct Section: Identifiable, Equatable {
        public var id: String
        public var title: String
        public var query: String
        public var items: [PRSummary] = []
        public var error: String?
        public var isLoading = false
    }

    @ObservationIgnored let services: Services
    public private(set) var sections: [Section] = []
    public private(set) var lastRefresh: Date?
    @ObservationIgnored private var knownReviewRequests: Set<String>?
    @ObservationIgnored private var timer: Timer?

    public init(services: Services) {
        self.services = services
        rebuildSections()
    }

    func rebuildSections() {
        var s = InboxSection.allCases.map { Section(id: $0.rawValue, title: $0.title, query: $0.query) }
        for repo in services.settings.pinnedRepos {
            s.append(Section(id: "repo:\(repo)", title: repo, query: "is:pr is:open repo:\(repo) sort:updated-desc"))
        }
        // Keep already-loaded items when pinned repos change.
        let old = Dictionary(uniqueKeysWithValues: sections.map { ($0.id, $0) })
        sections = s.map { var n = $0; if let o = old[$0.id] { n.items = o.items }; return n }
    }

    public func pinnedReposChanged() {
        rebuildSections()
        Task { await refresh() }
    }

    public func refresh() async {
        rebuildSections()
        for i in sections.indices { sections[i].isLoading = true }
        let api = services.github
        // One task per section, each applying its own result as soon as it arrives.
        // Not a task group: with Swift 6.3 in release builds, `withTaskGroup` here
        // finished without yielding any child results, so every section spun forever.
        let tasks = sections.map { section in
            Task { @MainActor [weak self] in
                let result: Result<[PRSummary], Error>
                do { result = .success(try await api.search(section.query)) } catch { result = .failure(error) }
                self?.apply(id: section.id, result: result)
            }
        }
        for t in tasks { await t.value }
        lastRefresh = Date()
        notifyNewReviewRequests()
    }

    private func apply(id: String, result: Result<[PRSummary], Error>) {
        guard let i = sections.firstIndex(where: { $0.id == id }) else { return }
        var section = sections[i]
        section.isLoading = false
        switch result {
        case .success(let items): section.items = items; section.error = nil
        case .failure(let e): section.error = e.localizedDescription
        }
        sections[i] = section
    }

    public func startAutoRefresh() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 300, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
    }

    /// Refresh when the app becomes active, at most once a minute.
    public func refreshIfStale() {
        if let last = lastRefresh, Date().timeIntervalSince(last) < 60 { return }
        Task { await refresh() }
    }

    private func notifyNewReviewRequests() {
        guard let section = sections.first(where: { $0.id == InboxSection.reviewRequested.rawValue }) else { return }
        let ids = Set(section.items.map(\.id))
        defer { knownReviewRequests = ids }
        guard let known = knownReviewRequests, services.settings.notifyOnReviewRequests else { return }
        let fresh = section.items.filter { !known.contains($0.id) }
        guard !fresh.isEmpty, Bundle.main.bundleIdentifier != nil else { return }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            for pr in fresh {
                let content = UNMutableNotificationContent()
                content.title = "Review requested: \(pr.ref)"
                content.body = "\(pr.title) — @\(pr.author)"
                content.userInfo = ["url": pr.ref.webURL.absoluteString]
                center.add(UNNotificationRequest(identifier: pr.id, content: content, trigger: nil))
            }
        }
    }
}

/// Root application state.
@MainActor
@Observable
public final class AppModel {
    public private(set) var services: Services?
    public private(set) var inbox: InboxStore?
    public private(set) var current: PRStore?
    public var showOpenURL = false
    public var showPalette = false
    /// Left column (inbox). The file tree column stays; the user can drag it closed.
    public var showSidebar = true
    /// Closed until a discussion starts (or the user toggles it with ⌘\).
    public var showDiscussPanel = false
    public var showSubmitReview = false
    public var showPushSheet = false
    public var startupError: String?
    @ObservationIgnored private var cache: [PRRef: PRStore] = [:]
    @ObservationIgnored private var updateTimer: Timer?
    @ObservationIgnored private var cacheOrder: [PRRef] = []

    public init() {}

    /// Launch arguments:
    ///   --synthetic small|large|huge   open an in-memory PR of that size (perf loop)
    ///   --open <url>                   open a PR on launch
    public func bootstrap(arguments: [String] = CommandLine.arguments) async {
        if let i = arguments.firstIndex(of: "--synthetic") {
            let name = i + 1 < arguments.count ? arguments[i + 1] : "large"
            let shape: SyntheticDiff.Shape = name == "huge" ? .huge : name == "small" ? .small : .large
            let store = LocalStore(root: FileManager.default.temporaryDirectory.appendingPathComponent("rb-synthetic-\(UUID().uuidString)"))
            let s = Services(github: DemoGitHub.synthetic(shape), store: store, settings: AppSettings(),
                             worktrees: nil, claudePath: nil)
            install(services: s)
            _ = PerfRunner.launch
            if let j = arguments.firstIndex(of: "--perf-run"), j + 1 < arguments.count {
                let out = URL(fileURLWithPath: arguments[j + 1])
                Task { await PerfRunner.run(app: self, output: out) }
            }
            if let j = arguments.firstIndex(of: "--ui-script"), j + 1 < arguments.count {
                let dirIdx = arguments.firstIndex(of: "--snap-dir")
                let dir = URL(fileURLWithPath: dirIdx.map { arguments[$0 + 1] } ?? NSTemporaryDirectory())
                Task { await UIScript.run(app: self, script: arguments[j + 1], dir: dir) }
            }
            await open(PRRef(owner: "demo", repo: "synthetic", number: 1))
            return
        }
        let s = await Services.live()
        install(services: s)
        // Load the inbox alongside any PR opened at launch, not after it: a big PR
        // can take a long time to load and the sidebar would sit empty meanwhile.
        let inbox = self.inbox
        Task { await inbox?.refresh() }
        if let j = arguments.firstIndex(of: "--ui-script"), j + 1 < arguments.count {
            let dirIdx = arguments.firstIndex(of: "--snap-dir")
            let dir = URL(fileURLWithPath: dirIdx.map { arguments[$0 + 1] } ?? NSTemporaryDirectory())
            Task { await UIScript.run(app: self, script: arguments[j + 1], dir: dir) }
        }
        if let i = arguments.firstIndex(of: "--open"), i + 1 < arguments.count, let ref = PRRef(string: arguments[i + 1]) {
            await open(ref)
        }
    }

    public func install(services: Services) {
        self.services = services
        let inbox = InboxStore(services: services)
        self.inbox = inbox
        inbox.startAutoRefresh()
        updateTimer?.invalidate()
        updateTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.current?.checkForUpdates() }
        }
    }

    public func open(_ ref: PRRef) async {
        guard let services else { return }
        if let cached = cache[ref] {
            current = cached
            await cached.checkForUpdates()
            return
        }
        let store = PRStore(ref: ref, services: services)
        cache[ref] = store
        cacheOrder.append(ref)
        // Keep a handful of PRs warm; drop the oldest.
        if cacheOrder.count > 6 {
            let old = cacheOrder.removeFirst()
            if old != current?.ref { cache.removeValue(forKey: old) }
        }
        current = store
        await store.load()
    }

    public func open(urlString: String) async -> Bool {
        guard let ref = PRRef(string: urlString) else { return false }
        await open(ref)
        return true
    }

    public func reloadCurrent() async {
        await current?.refresh()
        await inbox?.refresh()
    }
}
