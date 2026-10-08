import AppKit
import Foundation
import Observation
import ReviewBuddyCore

public struct DiffSelection: Equatable, Sendable {
    public var file: Int
    public var path: String
    public var side: DiffSide
    /// Line where the drag started; `end` follows the mouse.
    public var start: Int
    public var end: Int
    /// Character offsets (UTF-16) on the `start` / `end` lines for a text selection made
    /// by dragging in the code; nil when whole lines are selected (gutter, keyboard).
    public var startColumn: Int?
    public var endColumn: Int?
    /// Comments and discussions still anchor on whole lines: GitHub has no character ranges.
    public var anchor: CodeAnchor { CodeAnchor(path: path, side: side, startLine: start, endLine: end) }
    public func contains(side s: DiffSide, line: Int) -> Bool {
        s == side && line >= min(start, end) && line <= max(start, end)
    }

    public var isPartial: Bool { startColumn != nil && endColumn != nil }

    /// Selected characters on `line`, or nil for a whole-line selection. The upper bound
    /// is `Int.max` when the selection continues past the end of the line.
    public func columnRange(line: Int) -> Range<Int>? {
        guard let sc = startColumn, let ec = endColumn else { return nil }
        let (lo, loCol, hi, hiCol) = (start, sc) <= (end, ec) ? (start, sc, end, ec) : (end, ec, start, sc)
        guard line >= lo, line <= hi else { return nil }
        let a = line == lo ? loCol : 0
        let b = line == hi ? hiCol : Int.max
        return a..<max(a, b)
    }
}

public struct Composer: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case newThread(CodeAnchor)
        case reply(threadId: String)
        case edit(threadId: String, commentId: String)
    }
    public var id: String = UUID().uuidString
    public var kind: Kind
    public var text: String = ""
    public var isSubmitting = false
    public var error: String?
    /// Lines under the anchor, used for "Insert suggestion" and previews.
    public var anchorLines: [String] = []
    /// Set when the draft came from an AI finding or Discuss conversation.
    public var origin: String?

    public var anchor: CodeAnchor? { if case .newThread(let a) = kind { return a }; return nil }
}

public enum LoadState: Equatable, Sendable {
    case idle, loading(String), loaded, failed(String)
}

public enum TaskState: Equatable, Sendable {
    case idle, running(String), failed(String)
    public var isRunning: Bool { if case .running = self { return true }; return false }
}

public enum WorktreeState: Equatable, Sendable {
    case idle, preparing, ready(Worktree), failed(String)
    public var worktree: Worktree? { if case .ready(let w) = self { return w }; return nil }
}

/// What the store needs from the diff table. Lets the store drive scrolling and
/// reloads without owning AppKit views (and lets tests use a fake).
@MainActor
public protocol DiffViewDriving: AnyObject {
    func layoutDidChange(preserveScroll: Bool)
    func reloadFile(_ file: Int)
    func scrollToRow(_ row: Int, select: Bool)
    var topVisibleRow: Int { get }
    func focusDiff()
}

/// State and actions for one open pull request.
@MainActor
@Observable
public final class PRStore {
    public let ref: PRRef
    @ObservationIgnored let services: Services

    // Loaded data
    public internal(set) var detail: PRDetail?
    public private(set) var diff: DiffSet?
    /// Driven imperatively into the table; not observed by SwiftUI (rebuilt often).
    @ObservationIgnored public private(set) var layout: DiffLayout?
    @ObservationIgnored public private(set) var layoutVersion = 0
    public private(set) var loadState: LoadState = .idle
    public private(set) var diffSource: String = ""

    // View state
    @ObservationIgnored public var options = DiffLayoutOptions() {
        didSet { if options.mode != viewMode { viewMode = options.mode } }
    }
    /// Observable mirror of `options.mode` for the toolbar.
    public private(set) var viewMode: DiffViewMode = .unified
    public var selection: DiffSelection?
    /// Not observed by SwiftUI: it changes on every scroll. Interested views subscribe.
    @ObservationIgnored public private(set) var currentFile = 0
    @ObservationIgnored public var onCurrentFileChange: [String: (Int) -> Void] = [:]
    public var fileFilter = ""
    /// The Conversation & commits screen is showing instead of the diff. Every jump to
    /// code (`scrollToFile`, `scrollTo(anchor:)`, `scrollToBlock`) switches back.
    public var showOverview = false
    /// The user's "Review first" list; nil follows the AI summary's reading order.
    public private(set) var customReviewOrder: [String]?
    public var banner: String?

    // Review
    public private(set) var threads: [String: ReviewThread] = [:] {
        didSet { recomputeThreadCounts() }
    }
    /// Unresolved thread count per path, cached so per-row views don't scan all threads.
    public private(set) var openThreadCountByPath: [String: Int] = [:]
    /// Not observed as a whole: views observe per-composer / per-thread cells below, so
    /// opening one composer doesn't re-render every thread on screen.
    @ObservationIgnored public private(set) var composers: [String: Composer] = [:] {
        didSet { syncComposerCells(old: oldValue) }
    }
    @ObservationIgnored private var composerCells: [String: ValueCell<Composer?>] = [:]
    @ObservationIgnored private var replyCells: [String: ValueCell<String?>] = [:]   // threadId → composer id
    @ObservationIgnored private var editCells: [String: ValueCell<String?>] = [:]    // commentId → composer id
    public private(set) var pendingReviewId: String?
    public private(set) var viewed: Set<String> = []
    public var reviewBody = ""
    public private(set) var submitState: TaskState = .idle

    // AI
    public private(set) var discussThreads: [String: DiscussThread] = [:] {
        didSet {
            let paths = Set(discussThreads.values.map(\.anchor.path))
            if paths != discussPaths { discussPaths = paths }
        }
    }
    public private(set) var discussPaths: Set<String> = []
    @ObservationIgnored public private(set) var controllers: [String: DiscussController] = [:]
    public var activeDiscussId: String?
    public private(set) var findings: [String: AIFinding] = [:]
    /// PR head the current findings were produced for (outdated when it differs).
    public private(set) var findingsHeadSHA: String?
    /// The review thread last reached with ]/[ — target of the `r` (reply) shortcut.
    public private(set) var focusedThreadId: String?
    public private(set) var summary: AISummary?
    public private(set) var summaryState: TaskState = .idle
    public private(set) var preReviewState: TaskState = .idle
    /// Running AI jobs, kept so the user can cancel them (cancelling interrupts `claude`).
    @ObservationIgnored var summaryTask: Task<Void, Never>?
    @ObservationIgnored var preReviewTask: Task<Void, Never>?
    public private(set) var fixRequests: [FixRequest] = []

    // Git
    public private(set) var worktreeState: WorktreeState = .idle
    public private(set) var localCommits: [CommitInfo] = []
    public private(set) var remoteAhead = 0
    /// PR head on GitHub as of the last update check; differs from `detail.headOid` once pushed to.
    public internal(set) var remoteHeadOid: String?
    @ObservationIgnored private var lastUpdateCheck: Date?
    public private(set) var pushState: TaskState = .idle

    @ObservationIgnored public weak var view: DiffViewDriving?
    @ObservationIgnored let highlights: HighlightCacheRef = HighlightCacheRef()
    /// Side-by-side pairing for every file, computed off the main thread after load so the
    /// first switch to split view only lays out rows.
    @ObservationIgnored var splitPairs: [ContiguousArray<SplitPair>]?
    @ObservationIgnored private var worktreeTask: Task<Worktree, Error>?
    @ObservationIgnored var fileContentCache: [String: [String]] = [:]

    public init(ref: PRRef, services: Services) {
        self.ref = ref
        self.services = services
        options.mode = services.settings.viewMode
        options.largeFileThreshold = services.settings.largeFileThreshold
        options.showExpandFooter = true
    }

    // MARK: - Loading

    public func load() async {
        loadState = .loading("Loading pull request…")
        let start = ContinuousClock.now
        let local = await services.store.load(ref)
        apply(local: local)
        customReviewOrder = local.reviewOrder
        do {
            async let detailTask = services.github.pullRequest(ref)
            async let diffTask: Data? = { [services, ref] in
                do { return try await services.github.diff(ref) } catch GitHubError.diffTooLarge { return nil }
            }()
            let detail = try await detailTask
            self.detail = detail
            self.remoteHeadOid = nil
            self.pendingReviewId = detail.pendingReviewId
            self.threads = Dictionary(uniqueKeysWithValues: detail.threads.map { ($0.id, $0) })
            self.viewed = Set(detail.files.filter { $0.viewed == .viewed }.map(\.path))
            var data = try await diffTask
            diffSource = "GitHub API"
            if data == nil {
                loadState = .loading("Large diff — preparing local checkout…")
                let wt = try await ensureWorktree()
                data = try await services.worktrees?.diff(repoDir: wt.repoDir, base: "refs/review-buddy/base-\(ref.number)",
                                                          head: wt.remoteHeadRef)
                diffSource = "local git"
            }
            let bytes = data ?? Data()
            let parsed = await Task.detached(priority: .userInitiated) {
                Perf.shared.measure("diff.parse") { UnifiedDiffParser.parse(data: bytes) }
            }.value
            install(diff: parsed)
            applyFileOrder()
            Perf.shared.record("pr.load.total", ms: (ContinuousClock.now - start).milliseconds)
            loadState = .loaded
            remapDiscussThreads()
            await cleanUpWorktreeIfClosed()
            autoRunAI()
        } catch {
            loadState = .failed(error.localizedDescription)
        }
    }

    /// Installs a diff directly (fixtures, synthetic perf mode, tests).
    public func install(diff: DiffSet, detail: PRDetail? = nil) {
        if let detail {
            self.detail = detail
            self.threads = Dictionary(uniqueKeysWithValues: detail.threads.map { ($0.id, $0) })
            self.viewed = Set(detail.files.filter { $0.viewed == .viewed }.map(\.path))
            self.pendingReviewId = detail.pendingReviewId
        }
        self.diff = diff
        splitPairs = nil
        highlights.cache = HighlightCache(diff: diff)
        Task { [weak self] in
            let pairs = await Task.detached(priority: .utility) { diff.files.map { SplitAligner.align(file: $0) } }.value
            guard let self, self.diff === diff, self.splitPairs == nil else { return }
            self.splitPairs = pairs
        }
        // Viewed files start collapsed, like GitHub.
        options.collapsed = Set(diff.files.indices.filter { viewed.contains(diff.files[$0].path) })
        rebuildLayout(preserveScroll: false)
        loadState = .loaded
    }

    func apply(local: PRLocalState) {
        discussThreads = Dictionary(uniqueKeysWithValues: local.discussThreads.map { ($0.id, $0) })
        summary = local.summary
        findings = Dictionary(uniqueKeysWithValues: (local.preReview?.findings ?? []).map { ($0.id, $0) })
        findingsHeadSHA = local.preReview?.headSHA
        fixRequests = local.fixRequests.map {
            var f = $0
            if f.status == .running || f.status == .queued { f.status = .failed; f.error = "Interrupted" }
            return f
        }
    }

    /// Reloads the PR from GitHub and brings the local checkout up to date with it.
    public func refresh() async {
        let scrollRow = view?.topVisibleRow
        await load()
        if let scrollRow, let layout, scrollRow < layout.rowCount { view?.scrollToRow(scrollRow, select: false) }
        await refreshGitState(updateCheckout: true)
    }

    /// New commits on GitHub that this view (diff, checkout) doesn't have yet.
    public var isOutdated: Bool {
        guard let remoteHeadOid, let detail else { return false }
        return remoteHeadOid != detail.headOid
    }

    /// Polls GitHub's PR head (cheap) so the UI can offer a refresh; at most every 30s unless forced.
    public func checkForUpdates(force: Bool = false) async {
        guard loadState == .loaded, detail != nil else { return }
        if !force, let last = lastUpdateCheck, Date().timeIntervalSince(last) < 30 { return }
        lastUpdateCheck = Date()
        guard let oid = try? await services.github.headOid(ref), loadState == .loaded else { return }
        if oid != remoteHeadOid { remoteHeadOid = oid }
    }

    private func recomputeThreadCounts() {
        var counts: [String: Int] = [:]
        for t in threads.values where !t.isResolved { counts[t.path, default: 0] += 1 }
        if counts != openThreadCountByPath { openThreadCountByPath = counts }
    }

    // MARK: - Layout

    public var files: [FileDiff] { diff?.files ?? [] }

    func blocksByFile() -> [Int: [InlineBlockRef]] {
        guard let diff else { return [:] }
        var out: [Int: [InlineBlockRef]] = [:]
        func add(_ path: String, _ b: InlineBlockRef) {
            guard let f = diff.fileIndex(path: path) else { return }
            out[f, default: []].append(b)
        }
        for t in threads.values.sorted(by: { ($0.comments.first?.createdAt ?? .distantPast) < ($1.comments.first?.createdAt ?? .distantPast) }) {
            // Outdated threads have no current line; -1 sends them to the end of the file.
            add(t.path, InlineBlockRef(id: t.id, kind: .reviewThread, side: t.side, line: t.line ?? -1))
        }
        for f in findings.values where f.state == .open {
            add(f.path, InlineBlockRef(id: f.id, kind: .finding, side: f.side, line: f.endLine))
        }
        for d in discussThreads.values.sorted(by: { $0.createdAt < $1.createdAt }) {
            add(d.anchor.path, InlineBlockRef(id: d.id, kind: .discuss, side: d.anchor.side,
                                              line: d.status == .outdated ? -1 : d.anchor.endLine))
        }
        for c in composers.values {
            if let a = c.anchor { add(a.path, InlineBlockRef(id: c.id, kind: .composer, side: a.side, line: a.endLine)) }
        }
        return out
    }

    public func rebuildLayout(preserveScroll: Bool = true) {
        guard let diff else { return }
        if layout?.options.mode == .split, let pairs = layout?.splitPairs, !pairs.isEmpty { splitPairs = pairs }
        let reuse = splitPairs?.count == diff.files.count ? splitPairs : nil
        layout = Perf.shared.measure("layout.build") {
            DiffLayout(diff: diff, options: options, blocks: blocksByFile(), splitCache: reuse)
        }
        layoutVersion += 1
        Perf.shared.measure("layout.notifyView") { view?.layoutDidChange(preserveScroll: preserveScroll) }
    }

    public func setMode(_ mode: DiffViewMode) {
        guard options.mode != mode else { return }
        let start = ContinuousClock.now
        defer { Perf.shared.record("setMode.sync", ms: (ContinuousClock.now - start).milliseconds) }
        options.mode = mode
        services.settings.viewMode = mode
        services.saveSettings()
        rebuildLayout()
    }

    public func applySettings() {
        options.largeFileThreshold = services.settings.largeFileThreshold
        options.mode = services.settings.viewMode
        rebuildLayout()
    }

    public func toggleMode() { setMode(options.mode == .split ? .unified : .split) }

    public func toggleCollapsed(_ file: Int) {
        if options.collapsed.contains(file) { options.collapsed.remove(file) } else { options.collapsed.insert(file) }
        rebuildLayout()
    }

    public func forceLoad(_ file: Int) {
        options.forceLoaded.insert(file)
        options.collapsed.remove(file)
        rebuildLayout()
    }

    public func reportTopRow(_ row: Int) {
        guard let layout, row >= 0, row < layout.rowCount else { return }
        let f = layout.fileIndex(forRow: row)
        if f != currentFile {
            currentFile = f
            for cb in onCurrentFileChange.values { cb(f) }
        }
    }

    // MARK: - Viewed

    public func isViewed(_ file: Int) -> Bool {
        guard file < files.count else { return false }
        return viewed.contains(files[file].path)
    }

    public func toggleViewed(_ file: Int) {
        guard file < files.count else { return }
        let path = files[file].path
        let nowViewed = !viewed.contains(path)
        if nowViewed { viewed.insert(path); options.collapsed.insert(file) }
        else { viewed.remove(path); options.collapsed.remove(file) }
        rebuildLayout()
        if nowViewed, let start = layout?.fileStartRow[file] { view?.scrollToRow(start, select: false) }
        guard let prId = detail?.nodeId else { return }
        Task {
            do { try await services.github.setViewed(pullRequestId: prId, path: path, viewed: nowViewed) }
            catch {
                if nowViewed { viewed.remove(path) } else { viewed.insert(path) }
                banner = "Couldn't update viewed state: \(error.localizedDescription)"
            }
        }
    }

    public var viewedCount: Int { files.filter { viewed.contains($0.path) }.count }

    // MARK: - Expand context

    public func expandAbove(file: Int, hunk: Int, by n: Int = 20) {
        Task {
            guard await ensureFileContent(file: file) else { return }
            let key = DiffLayoutOptions.key(file: file, hunk: hunk)
            options.expandedAbove[key, default: 0] += n
            rebuildLayout()
        }
    }

    public func expandBelow(file: Int, by n: Int = 20) {
        Task {
            guard await ensureFileContent(file: file), let lines = fileContentCache[files[file].path],
                  let last = files[file].hunks.last else { return }
            let shownEnd = last.newStart + last.newCount - 1 + options.expandedBelow[file, default: 0]
            let more = min(n, lines.count - shownEnd)
            guard more > 0 else { return }
            options.expandedBelow[file, default: 0] += more
            rebuildLayout()
        }
    }

    /// Whether there are hidden lines above hunk `h` (shows the expand arrow).
    public func canExpandAbove(file: Int, hunk: Int) -> Bool {
        guard file < files.count, hunk < files[file].hunks.count else { return false }
        let f = files[file]
        let h = f.hunks[hunk]
        let prevEnd = hunk > 0 ? f.hunks[hunk - 1].newStart + f.hunks[hunk - 1].newCount : 1
        let expanded = options.expandedAbove[DiffLayoutOptions.key(file: file, hunk: hunk)] ?? 0
        return h.newStart - expanded > prevEnd
    }

    public func canExpandBelow(file: Int) -> Bool {
        guard file < files.count, let last = files[file].hunks.last, files[file].status != .deleted else { return false }
        if let lines = fileContentCache[files[file].path] {
            return last.newStart + last.newCount - 1 + (options.expandedBelow[file] ?? 0) < lines.count
        }
        return true
    }

    @ObservationIgnored var expandedLinesProvider: ((Int, [String]) -> Void)?

    func ensureFileContent(file: Int) async -> Bool {
        let f = files[file]
        if fileContentCache[f.path] != nil { return true }
        guard let detail else { return false }
        do {
            let data: Data
            if let wt = worktreeState.worktree, let w = services.worktrees {
                data = try await w.fileContents(repoDir: wt.repoDir, sha: detail.headOid, path: f.newPath)
            } else {
                let owner = detail.headRepoNameWithOwner?.split(separator: "/").map(String.init) ?? [ref.owner, ref.repo]
                data = try await services.github.fileContents(owner: owner[0], repo: owner[1], path: f.newPath, ref: detail.headOid)
            }
            let lines = String(decoding: data, as: UTF8.self).split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            fileContentCache[f.path] = lines
            expandedLinesProvider?(file, lines)
            return true
        } catch {
            banner = "Couldn't load \(f.path): \(error.localizedDescription)"
            return false
        }
    }

    // MARK: - Selection

    /// `column` makes it a character selection (drag in the code); nil selects whole lines.
    public func select(file: Int, side: DiffSide, line: Int, extend: Bool, column: Int? = nil) {
        guard file < files.count, line > 0 else { return }
        if extend, var s = selection, s.file == file, s.side == side {
            s.end = line
            if s.startColumn != nil { s.endColumn = column ?? Int.max }
            selection = s
        } else {
            selection = DiffSelection(file: file, path: files[file].path, side: side, start: line, end: line,
                                      startColumn: column, endColumn: column)
        }
    }

    /// A click in the code that didn't drag: nothing is selected. Lines are selected from
    /// the gutter (line numbers, +) only.
    public var isEmptyTextSelection: Bool {
        guard let s = selection else { return false }
        return s.isPartial && s.start == s.end && s.startColumn == s.endColumn
    }

    /// The selected text: whole lines, or just the dragged-over characters.
    public func selectedText() -> String? {
        guard let sel = selection else { return nil }
        let a = sel.anchor
        let texts = lines(for: a)
        guard sel.isPartial, texts.count == a.endLine - a.startLine + 1 else { return texts.joined(separator: "\n") }
        return zip(a.startLine...a.endLine, texts).map { n, t -> String in
            guard let r = sel.columnRange(line: n) else { return t }
            let u = Array(t.utf16)
            let lo = min(r.lowerBound, u.count), hi = min(r.upperBound, u.count)
            return String(decoding: u[lo..<hi], as: UTF16.self)
        }.joined(separator: "\n")
    }

    public func clearSelection() { selection = nil }

    /// Text of lines in an anchor, from the diff (and expanded context).
    public func lines(for anchor: CodeAnchor) -> [String] {
        guard let diff, let fi = diff.fileIndex(path: anchor.path) else { return [] }
        let f = diff.files[fi]
        var byLine: [Int: String] = [:]
        for l in f.lines where l.kind != .noNewline {
            let n = l.lineNumber(on: anchor.side)
            if n >= anchor.startLine && n <= anchor.endLine && (anchor.side == .right ? l.kind != .deletion : l.kind != .addition) {
                byLine[n] = diff.text(of: l)
            }
        }
        if anchor.side == .right, let content = fileContentCache[anchor.path] {
            for n in anchor.startLine...anchor.endLine where byLine[n] == nil && n >= 1 && n <= content.count {
                byLine[n] = content[n - 1]
            }
        }
        return (anchor.startLine...anchor.endLine).compactMap { byLine[$0] }
    }

    // MARK: - Comments

    public func startComment(_ anchor: CodeAnchor? = nil, text: String = "", origin: String? = nil) {
        guard let anchor = anchor ?? selection?.anchor else { return }
        // Reuse an open composer on the same anchor.
        if let existing = composers.values.first(where: { $0.anchor == anchor }) {
            if !text.isEmpty { composers[existing.id]?.text = text }
            return
        }
        var c = Composer(kind: .newThread(anchor), text: text)
        c.anchorLines = lines(for: anchor)
        c.origin = origin
        composers[c.id] = c
        selection = nil
        rebuildLayout()
    }

    /// `r`: reply to the focused thread (reached with ]/[), else the first thread on screen.
    public func replyToFocusedThread() {
        if let id = focusedThreadId, threads[id] != nil { startReply(threadId: id); return }
        guard let layout else { return }
        let top = max(0, view?.topVisibleRow ?? 0)
        for r in layout.rows.indices.dropFirst(top) where layout.rows[r].kind == .block {
            let b = layout.blocks[Int(layout.rows[r].a)]
            if b.kind == .reviewThread { startReply(threadId: b.id); return }
        }
    }

    public func startReply(threadId: String) {
        guard composers.values.first(where: { $0.kind == .reply(threadId: threadId) }) == nil else { return }
        let c = Composer(kind: .reply(threadId: threadId))
        composers[c.id] = c
        // Reply composers render inside the thread block; no layout change needed beyond height.
        layoutVersion += 1
        rebuildLayout()
    }

    public func startEdit(threadId: String, comment: ReviewComment) {
        let c = Composer(kind: .edit(threadId: threadId, commentId: comment.id), text: comment.body)
        composers[c.id] = c
        rebuildLayout()
    }

    public func composerFor(reply threadId: String) -> Composer? {
        let id = cell(threadId, in: &replyCells, initial: composers.values.first {
            if case .reply(let t) = $0.kind { return t == threadId }; return false }?.id).value
        return id.flatMap { composer($0) }
    }

    public func composerFor(edit commentId: String) -> Composer? {
        let id = cell(commentId, in: &editCells, initial: composers.values.first {
            if case .edit(_, let c) = $0.kind { return c == commentId }; return false }?.id).value
        return id.flatMap { composer($0) }
    }

    public func composer(_ id: String) -> Composer? { cell(id, in: &composerCells, initial: composers[id]).value }

    private func cell<T>(_ key: String, in dict: inout [String: ValueCell<T>], initial: T) -> ValueCell<T> {
        if let c = dict[key] { return c }
        let c = ValueCell(initial)
        dict[key] = c
        return c
    }

    /// Lets the canvas render an offscreen composer to warm SwiftUI (not part of review state).
    func setPrewarmComposer(_ c: Composer?) {
        cell("prewarm", in: &composerCells, initial: nil).value = c
    }

    private func syncComposerCells(old: [String: Composer]) {
        for id in Set(old.keys).union(composers.keys) where old[id] != composers[id] {
            if let c = composerCells[id] { c.value = composers[id] }
        }
        var replies: [String: String] = [:], edits: [String: String] = [:]
        for c in composers.values {
            switch c.kind {
            case .reply(let t): replies[t] = c.id
            case .edit(_, let comment): edits[comment] = c.id
            case .newThread: break
            }
        }
        for (k, cell) in replyCells where cell.value != replies[k] { cell.value = replies[k] }
        for (k, cell) in editCells where cell.value != edits[k] { cell.value = edits[k] }
        for (k, v) in replies where replyCells[k] == nil { replyCells[k] = ValueCell(v) }
        for (k, v) in edits where editCells[k] == nil { editCells[k] = ValueCell(v) }
    }

    public func updateComposer(_ id: String, text: String) {
        composers[id]?.text = text
    }

    public func cancelComposer(_ id: String) {
        composers.removeValue(forKey: id)
        rebuildLayout()
    }

    func ensurePendingReview() async throws -> String {
        if let id = pendingReviewId { return id }
        guard let prId = detail?.nodeId else { throw GitHubError.notFound }
        let id = try await services.github.startReview(pullRequestId: prId)
        pendingReviewId = id
        return id
    }

    /// Posts the composer. `single` publishes it right away instead of adding it to the
    /// pending review (only offered while there is no pending review, as on GitHub).
    public func submitComposer(_ id: String, single: Bool = false) async {
        guard var c = composers[id], !c.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        c.isSubmitting = true
        c.error = nil
        composers[id] = c
        do {
            switch c.kind {
            case .newThread(let anchor) where single:
                guard let d = detail else { return }
                try await services.github.addSingleComment(ref: ref, commitId: d.headOid, anchor: anchor, body: c.text)
                try await reloadThreads()
            case .reply(let threadId) where single:
                if let dbId = threads[threadId]?.comments.first?.databaseId {
                    try await services.github.replySingle(ref: ref, threadId: threadId, commentDatabaseId: dbId, body: c.text)
                    try await reloadThreads()
                } else {
                    // No REST id (shouldn't happen for GitHub data): a reply outside a review.
                    let comment = try await services.github.reply(threadId: threadId, reviewId: nil, body: c.text)
                    threads[threadId]?.comments.append(comment)
                }
            case .newThread(let anchor):
                let review = try await ensurePendingReview()
                guard let prId = detail?.nodeId else { return }
                let t = try await services.github.addThread(pullRequestId: prId, reviewId: review, anchor: anchor, body: c.text)
                threads[t.id] = t
            case .reply(let threadId):
                let review = try await ensurePendingReview()
                let comment = try await services.github.reply(threadId: threadId, reviewId: review, body: c.text)
                threads[threadId]?.comments.append(comment)
            case .edit(let threadId, let commentId):
                let updated = try await services.github.updateComment(id: commentId, body: c.text)
                if let i = threads[threadId]?.comments.firstIndex(where: { $0.id == commentId }) {
                    threads[threadId]?.comments[i] = updated
                }
            }
            composers.removeValue(forKey: id)
            rebuildLayout()
        } catch {
            composers[id]?.isSubmitting = false
            composers[id]?.error = error.localizedDescription
        }
    }

    /// Re-reads threads from GitHub (the REST single-comment endpoints don't return the thread).
    private func reloadThreads() async throws {
        let d = try await services.github.pullRequest(ref)
        threads = Dictionary(uniqueKeysWithValues: d.threads.map { ($0.id, $0) })
        detail?.threads = d.threads
    }

    public func deleteComment(threadId: String, commentId: String) async {
        do {
            try await services.github.deleteComment(id: commentId)
            threads[threadId]?.comments.removeAll { $0.id == commentId }
            if threads[threadId]?.comments.isEmpty == true { threads.removeValue(forKey: threadId) }
            rebuildLayout()
        } catch {
            banner = "Couldn't delete comment: \(error.localizedDescription)"
        }
    }

    public func setResolved(threadId: String, _ resolved: Bool) async {
        let before = threads[threadId]?.isResolved
        threads[threadId]?.isResolved = resolved
        layoutVersion += 1
        do {
            let r = try await services.github.setResolved(threadId: threadId, resolved: resolved)
            threads[threadId]?.isResolved = r
        } catch {
            threads[threadId]?.isResolved = before ?? !resolved
            banner = "Couldn't update thread: \(error.localizedDescription)"
        }
        layoutVersion += 1
    }

    public var pendingCommentCount: Int {
        threads.values.reduce(0) { $0 + $1.comments.filter(\.isPending).count }
    }

    public func submitReview(event: ReviewEvent) async -> Bool {
        submitState = .running("Submitting…")
        do {
            let id = try await ensurePendingReview()
            try await services.github.submitReview(reviewId: id, event: event, body: reviewBody)
            pendingReviewId = nil
            reviewBody = ""
            for (k, t) in threads {
                threads[k]?.comments = t.comments.map { var c = $0; c.isPending = false; return c }
            }
            submitState = .idle
            banner = event == .approve ? "Approved ✓" : "Review submitted"
            return true
        } catch {
            submitState = .failed(error.localizedDescription)
            return false
        }
    }

    public func discardPendingReview() async {
        guard let id = pendingReviewId else { return }
        do {
            try await services.github.deleteReview(reviewId: id)
            pendingReviewId = nil
            submitState = .idle
            threads = threads.filter { !$0.value.isPending }
            for (k, t) in threads { threads[k]?.comments = t.comments.filter { !$0.isPending } }
            rebuildLayout()
        } catch {
            banner = "Couldn't discard review: \(error.localizedDescription)"
        }
    }

    // MARK: - Navigation

    public func navigate(_ action: ReviewAction) {
        guard let layout else { return }
        let top = max(0, view?.topVisibleRow ?? 0)
        func scroll(_ r: Int?) { if let r { view?.scrollToRow(r, select: false) } }
        switch action {
        case .nextFile:
            let f = layout.fileIndex(forRow: top)
            scroll(f + 1 < layout.fileStartRow.count ? layout.fileStartRow[f + 1] : nil)
        case .previousFile:
            let f = layout.fileIndex(forRow: top)
            let start = layout.fileStartRow[f]
            scroll(top > start ? start : (f > 0 ? layout.fileStartRow[f - 1] : nil))
        case .nextHunk:
            scroll(layout.rows.indices.dropFirst(top + 1).first { layout.rows[$0].kind == .hunkHeader })
        case .previousHunk:
            scroll(layout.rows.indices.prefix(top).last { layout.rows[$0].kind == .hunkHeader })
        case .nextThread, .previousThread:
            let target = action == .nextThread
                ? layout.rows.indices.dropFirst(top + 4).first { layout.rows[$0].kind == .block }
                : layout.rows.indices.prefix(max(0, top + 3)).last { layout.rows[$0].kind == .block }
            if let r = target {
                let b = layout.blocks[Int(layout.rows[r].a)]
                focusedThreadId = b.kind == .reviewThread ? b.id : nil
            }
            scroll(target.map { max(0, $0 - 3) })
        default:
            break
        }
    }

    // MARK: - Review order

    /// Files shown first, in order: the user's list, else the AI reading order. Only paths in the diff.
    public var reviewOrder: [String] {
        let wanted = customReviewOrder ?? summary?.readingOrder.map(\.path) ?? []
        var seen = Set<String>()
        return wanted.filter { diff?.fileIndex(path: $0) != nil && seen.insert($0).inserted }
    }

    /// The diff lists `reviewOrder` first, then everything else in file-tree order, so the
    /// tree and the diff always scroll in the same order.
    public var reviewFirstCount: Int { reviewOrder.count }

    public func addToReviewOrder(_ path: String) { setReviewOrder(reviewOrder.filter { $0 != path } + [path]) }
    public func removeFromReviewOrder(_ path: String) { setReviewOrder(reviewOrder.filter { $0 != path }) }

    public func moveInReviewOrder(_ path: String, by delta: Int) {
        var o = reviewOrder
        guard let i = o.firstIndex(of: path) else { return }
        let j = min(max(0, i + delta), o.count - 1)
        guard i != j else { return }
        o.remove(at: i); o.insert(path, at: j)
        setReviewOrder(o)
    }

    /// Back to the AI summary's reading order.
    public func resetReviewOrder() {
        customReviewOrder = nil
        persist()
        applyFileOrder()
    }

    private func setReviewOrder(_ paths: [String]) {
        customReviewOrder = paths
        persist()
        applyFileOrder()
    }

    /// Reorders the diff's files to `reviewOrder` + tree order. Index-keyed view state
    /// (collapsed, expanded context, split pairs) moves with its file.
    func applyFileOrder() {
        guard let old = diff else { return }
        let paths = old.files.map(\.path)
        let priority = reviewOrder.compactMap { old.fileIndex(path: $0) }
        let order = FileTreeBuilder.displayOrder(paths: paths, priority: priority)
        guard order != Array(paths.indices) else { return }
        var newIndex = [Int](repeating: 0, count: order.count)
        for (n, o) in order.enumerated() { newIndex[o] = n }
        // Stay on the file being read; from the very top, show the new first files.
        let topPath = currentFile > 0 ? files[safe: currentFile]?.path : nil

        options.collapsed = Set(options.collapsed.map { newIndex[$0] })
        options.forceLoaded = Set(options.forceLoaded.map { newIndex[$0] })
        options.expandedBelow = Dictionary(uniqueKeysWithValues: options.expandedBelow.map { (newIndex[$0.key], $0.value) })
        options.expandedAbove = Dictionary(uniqueKeysWithValues: options.expandedAbove.map {
            (DiffLayoutOptions.key(file: newIndex[Int($0.key >> 32)], hunk: Int($0.key & 0xFFFF_FFFF)), $0.value)
        })
        splitPairs = splitPairs.flatMap { p in p.count == order.count ? order.map { p[$0] } : nil }
        selection = nil
        let new = DiffSet(buffer: old.buffer, files: order.map { old.files[$0] })
        diff = new
        highlights.cache = HighlightCache(diff: new)
        layout = nil // its split pairs are in the old order
        rebuildLayout(preserveScroll: false)
        let row = topPath.flatMap { new.fileIndex(path: $0) }.flatMap { layout?.fileStartRow[safe: $0] } ?? 0
        view?.scrollToRow(row, select: false)
    }

    /// Posts a PR comment (GitHub's Conversation tab). Returns an error message on failure.
    public func addConversationComment(_ body: String) async -> String? {
        let text = body.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let d = detail else { return nil }
        do {
            let c = try await services.github.addComment(subjectId: d.nodeId, body: text)
            var conv = detail?.conversation ?? PRConversation()
            conv.comments.append(c)
            detail?.conversation = conv
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// Merges the PR on GitHub, then reloads it. Returns an error message on failure.
    public func mergePullRequest(method: MergeMethod) async -> String? {
        guard let d = detail else { return nil }
        do {
            try await services.github.mergePullRequest(id: d.nodeId, method: method, expectedHeadOid: d.headOid)
        } catch {
            return error.localizedDescription
        }
        await refresh()
        return nil
    }

    /// Closes or reopens the PR on GitHub, then reloads it. Returns an error message on failure.
    public func setClosed(_ closed: Bool) async -> String? {
        guard let d = detail else { return nil }
        do {
            try await services.github.setPullRequestClosed(id: d.nodeId, closed: closed)
        } catch {
            return error.localizedDescription
        }
        await refresh()
        return nil
    }

    public func scrollToFile(_ file: Int) {
        showOverview = false
        guard let layout, file < layout.fileStartRow.count else { return }
        view?.scrollToRow(layout.fileStartRow[file], select: false)
    }

    public func scrollToBlock(id: String) {
        showOverview = false
        guard let layout, let bi = layout.blocks.firstIndex(where: { $0.id == id }),
              let r = layout.rows.firstIndex(where: { $0.kind == .block && $0.a == Int32(bi) }) else { return }
        view?.scrollToRow(max(0, r - 4), select: false)
    }

    public func scrollTo(anchor: CodeAnchor) {
        showOverview = false
        guard let layout, let diff, let f = diff.fileIndex(path: anchor.path) else { return }
        if options.collapsed.contains(f) { options.collapsed.remove(f); rebuildLayout() }
        if let r = self.layout?.row(file: f, side: anchor.side, line: anchor.startLine) ?? layout.fileStartRow[safe: f] {
            view?.scrollToRow(max(0, r - 4), select: false)
        }
    }

    // MARK: - Worktree

    @discardableResult
    public func ensureWorktree() async throws -> Worktree {
        if let wt = worktreeState.worktree { return wt }
        if let t = worktreeTask { return try await t.value }
        guard let worktrees = services.worktrees else { throw WorktreeError.gitNotFound }
        guard let detail else { throw GitHubError.notFound }
        worktreeState = .preparing
        let clone = services.settings.localClone(for: ref.nameWithOwner)
        let task = Task { try await worktrees.ensureWorktree(for: detail, localClone: clone) }
        worktreeTask = task
        do {
            let wt = try await task.value
            worktreeState = .ready(wt)
            worktreeTask = nil
            await refreshGitState()
            remapDiscussThreads()
            return wt
        } catch {
            worktreeState = .failed(error.localizedDescription)
            worktreeTask = nil
            throw error
        }
    }

    /// Spec §5.1: when the PR is merged or closed, its worktree is removed unless it
    /// holds unpushed fix commits.
    func cleanUpWorktreeIfClosed() async {
        guard let detail, detail.state == "MERGED" || detail.state == "CLOSED",
              let w = services.worktrees,
              // Never pull the checkout out from under a Claude run that is preparing/using it.
              worktreeTask == nil, worktreeState.worktree == nil else { return }
        guard let wt = try? await w.ensureExistingWorktree(for: detail, localClone: services.settings.localClone(for: ref.nameWithOwner)) else { return }
        if let commits = try? await w.localCommits(wt), !commits.isEmpty { return }
        try? await w.remove(wt)
        if worktreeState.worktree == wt { worktreeState = .idle }
    }

    /// Fetches the PR into the checkout. With `updateCheckout`, also fast-forwards it to the
    /// new head when that loses nothing (no local fix commits, no uncommitted changes).
    public func refreshGitState(updateCheckout: Bool = false) async {
        guard let wt = worktreeState.worktree, let w = services.worktrees else { return }
        if let detail {
            if updateCheckout {
                _ = try? await w.ensureWorktree(for: detail, localClone: services.settings.localClone(for: ref.nameWithOwner))
            } else {
                try? await w.fetchPR(repoDir: wt.repoDir, pr: detail)
            }
        }
        localCommits = (try? await w.localCommits(wt)) ?? []
        remoteAhead = (try? await w.remoteAhead(wt)) ?? 0
    }

    public func push() async -> Bool {
        guard let wt = worktreeState.worktree, let w = services.worktrees, let detail else { return false }
        pushState = .running("Pushing…")
        do {
            try await w.push(wt, pr: detail)
            pushState = .idle
            banner = "Pushed \(localCommits.count) commit\(localCommits.count == 1 ? "" : "s") to \(detail.headRefName)"
            await refreshGitState()
            return true
        } catch {
            pushState = .failed(error.localizedDescription)
            return false
        }
    }

    public func rebaseOntoRemote() async {
        guard let wt = worktreeState.worktree, let w = services.worktrees else { return }
        do {
            try await w.rebaseOntoRemote(wt)
            await refreshGitState()
        } catch {
            banner = "Rebase failed (aborted): \(error.localizedDescription)"
        }
    }

    public var canPush: Bool { detail?.canPushFixes ?? false }

    public var pushDisabledReason: String? {
        guard let detail else { return "Not loaded" }
        if detail.canPushFixes { return nil }
        return detail.isFromFork
            ? "This PR comes from a fork and the author doesn't allow edits from maintainers."
            : "You don't have push access to \(ref.nameWithOwner)."
    }

    // MARK: - Persistence

    func persist() {
        var state = PRLocalState()
        state.discussThreads = Array(discussThreads.values).sorted { $0.createdAt < $1.createdAt }
        state.fixRequests = fixRequests
        state.summary = summary
        state.preReview = PreReviewResult(findings: Array(findings.values), headSHA: findingsHeadSHA ?? detail?.headOid, createdAt: Date())
        state.lastSeenHead = detail?.headOid
        state.reviewOrder = customReviewOrder
        let store = services.store
        let ref = self.ref
        Task { await store.save(ref, state) }
    }

    // MARK: - Discuss (implemented in PRStore+AI.swift)

    func updateDiscussThread(_ t: DiscussThread) {
        discussThreads[t.id] = t
        persist()
    }

    func setControllers(_ id: String, _ c: DiscussController?) { controllers[id] = c }

    func setSummary(_ s: AISummary?, state: TaskState) {
        let orderChanged = s?.readingOrder != summary?.readingOrder
        summary = s; summaryState = state
        // A new reading order moves its files to the top (unless the user set their own).
        if orderChanged, customReviewOrder == nil { applyFileOrder() }
    }
    func setPreReviewState(_ s: TaskState) { preReviewState = s }
    func setFindings(_ f: [AIFinding]) {
        findings = Dictionary(uniqueKeysWithValues: f.map { ($0.id, $0) })
        findingsHeadSHA = detail?.headOid
    }

    public var findingsAreOutdated: Bool {
        guard let h = findingsHeadSHA, let d = detail else { return false }
        return h != d.headOid
    }
    func updateFinding(_ f: AIFinding) { findings[f.id] = f }
    func setFixRequests(_ f: [FixRequest]) { fixRequests = f }
    func removeDiscuss(_ id: String) { discussThreads.removeValue(forKey: id) }
    func setDetailForTesting(_ d: PRDetail) { detail = d }
    func setWorktreeState(_ s: WorktreeState) { worktreeState = s }
}

/// A tiny observable box: SwiftUI views that read `value` re-render only when this
/// particular value changes.
@MainActor
@Observable
public final class ValueCell<T> {
    public var value: T
    public init(_ value: T) { self.value = value }
}

/// Indirection so the highlight cache can be swapped when a new diff is installed.
final class HighlightCacheRef {
    var cache: HighlightCache?
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
