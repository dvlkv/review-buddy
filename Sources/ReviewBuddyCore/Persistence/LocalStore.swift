import Foundation

public struct AppSettings: Codable, Equatable, Sendable {
    public var pinnedRepos: [String] = []
    /// `owner/repo` → path of the user's local clone.
    public var repoClones: [String: String] = [:]
    public var viewMode: DiffViewMode = .unified
    public var autoSummarize: Bool = false
    /// Run the AI pre-review automatically when a PR has at most this many changed lines (0 = never).
    public var autoPreReviewMaxLines: Int = 0
    public var claudeModel: String = ""
    public var notifyOnReviewRequests: Bool = true
    public var largeFileThreshold: Int = 2000
    public var fontSize: Double = 12
    public var hideWhitespace: Bool = false

    public init() {}

    public init(from decoder: Decoder) throws {
        // Tolerant decoding so new settings never wipe old ones.
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = AppSettings()
        pinnedRepos = (try? c.decode([String].self, forKey: .pinnedRepos)) ?? d.pinnedRepos
        repoClones = (try? c.decode([String: String].self, forKey: .repoClones)) ?? d.repoClones
        viewMode = (try? c.decode(DiffViewMode.self, forKey: .viewMode)) ?? d.viewMode
        autoSummarize = (try? c.decode(Bool.self, forKey: .autoSummarize)) ?? d.autoSummarize
        autoPreReviewMaxLines = (try? c.decode(Int.self, forKey: .autoPreReviewMaxLines)) ?? d.autoPreReviewMaxLines
        claudeModel = (try? c.decode(String.self, forKey: .claudeModel)) ?? d.claudeModel
        notifyOnReviewRequests = (try? c.decode(Bool.self, forKey: .notifyOnReviewRequests)) ?? d.notifyOnReviewRequests
        largeFileThreshold = (try? c.decode(Int.self, forKey: .largeFileThreshold)) ?? d.largeFileThreshold
        fontSize = (try? c.decode(Double.self, forKey: .fontSize)) ?? d.fontSize
        hideWhitespace = (try? c.decode(Bool.self, forKey: .hideWhitespace)) ?? d.hideWhitespace
    }

    enum CodingKeys: String, CodingKey {
        case pinnedRepos, repoClones, viewMode, autoSummarize, autoPreReviewMaxLines, claudeModel,
             notifyOnReviewRequests, largeFileThreshold, fontSize, hideWhitespace
    }

    public func localClone(for nameWithOwner: String) -> URL? {
        repoClones[nameWithOwner].map { URL(fileURLWithPath: $0) }
    }
}

/// Everything Review Buddy remembers about one PR.
public struct PRLocalState: Codable, Equatable, Sendable {
    public var discussThreads: [DiscussThread] = []
    public var fixRequests: [FixRequest] = []
    public var summary: AISummary?
    public var preReview: PreReviewResult?
    public var lastSeenHead: String?
    public init() {}
}

/// JSON-file persistence under Application Support. Writes are atomic and coalesced.
public actor LocalStore {
    public nonisolated let root: URL
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private var prCache: [String: PRLocalState] = [:]
    private var pendingWrites: [String: Task<Void, Never>] = [:]

    public static var defaultRoot: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("ReviewBuddy", isDirectory: true)
    }

    public init(root: URL = LocalStore.defaultRoot) {
        self.root = root
        encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys]
        decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        try? FileManager.default.createDirectory(at: root.appendingPathComponent("prs"), withIntermediateDirectories: true)
    }

    private var settingsURL: URL { root.appendingPathComponent("settings.json") }
    private func prURL(_ ref: PRRef) -> URL { root.appendingPathComponent("prs/\(ref.storageKey).json") }

    public func loadSettings() -> AppSettings {
        guard let d = try? Data(contentsOf: settingsURL), let s = try? decoder.decode(AppSettings.self, from: d) else {
            return AppSettings()
        }
        return s
    }

    public func saveSettings(_ s: AppSettings) throws {
        try encoder.encode(s).write(to: settingsURL, options: .atomic)
    }

    public func load(_ ref: PRRef) -> PRLocalState {
        if let c = prCache[ref.storageKey] { return c }
        let state = (try? Data(contentsOf: prURL(ref))).flatMap { try? decoder.decode(PRLocalState.self, from: $0) } ?? PRLocalState()
        prCache[ref.storageKey] = state
        return state
    }

    /// Saves after a short delay, coalescing bursts (e.g. streaming transcript updates).
    public func save(_ ref: PRRef, _ state: PRLocalState, debounce: Duration = .milliseconds(400)) {
        prCache[ref.storageKey] = state
        pendingWrites[ref.storageKey]?.cancel()
        pendingWrites[ref.storageKey] = Task { [weak self] in
            try? await Task.sleep(for: debounce)
            guard !Task.isCancelled else { return }
            await self?.writeNow(ref)
        }
    }

    public func update(_ ref: PRRef, _ body: (inout PRLocalState) -> Void) {
        var s = load(ref)
        body(&s)
        save(ref, s)
    }

    public func flush() {
        for key in pendingWrites.keys {
            pendingWrites[key]?.cancel()
            if let state = prCache[key], let data = try? encoder.encode(state) {
                try? data.write(to: root.appendingPathComponent("prs/\(key).json"), options: .atomic)
            }
        }
        pendingWrites.removeAll()
    }

    private func writeNow(_ ref: PRRef) {
        guard let state = prCache[ref.storageKey], let data = try? encoder.encode(state) else { return }
        try? data.write(to: prURL(ref), options: .atomic)
        pendingWrites[ref.storageKey] = nil
    }
}
