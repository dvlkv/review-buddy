import SwiftUI
import ReviewBuddyCore

/// The PR's Conversation and Commits (as on GitHub), plus the AI pre-review. Shown in
/// place of the diff when the "Conversation" row above the file tree is selected.
struct OverviewView: View {
    let store: PRStore
    @State private var tab: Tab = .conversation

    enum Tab: Hashable { case conversation, commits }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Picker("Section", selection: $tab) {
                    Text("Conversation").tag(Tab.conversation)
                    Text("Commits (\(commitCount))").tag(Tab.commits)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()

                OutdatedBar(store: store)
                PreReviewCard(store: store)

                switch tab {
                case .conversation:
                    ConversationTimeline(store: store)
                    // GitHub's order: timeline, merge box, then the comment box.
                    MergeStatusCard(store: store)
                    CommentComposer(store: store)
                case .commits: CommitList(store: store)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .frame(maxWidth: 900, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color.ghCanvas)
    }

    var commitCount: Int { (store.detail?.conversation?.commits.count ?? 0) + store.localCommits.count }
}

/// Run / stop the AI pre-review and list its open findings.
struct PreReviewCard: View {
    let store: PRStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "sparkles").foregroundStyle(Color.ghAI)
                Text("AI pre-review").font(.system(size: 13, weight: .semibold)).foregroundStyle(Color.ghAI)
                status
                Spacer()
                if store.preReviewState.isRunning {
                    Button { store.cancelPreReview() } label: { Label("Stop", systemImage: "stop.circle") }
                        .buttonStyle(GHButtonStyle(compact: true))
                } else {
                    Button { store.startPreReview() } label: {
                        Label(store.findings.isEmpty ? "Run pre-review" : "Run again", systemImage: "sparkles")
                    }
                    .buttonStyle(GHButtonStyle(kind: .ai, compact: true))
                    .help("Ask Claude to review the whole PR and leave private suggested comments")
                }
            }
            if case .failed(let e) = store.preReviewState {
                Text(e).font(.system(size: 12)).foregroundStyle(Color.ghDel).lineLimit(4)
            }
            let open = store.openFindings
            if !open.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(open) { f in
                        Button { store.scrollTo(anchor: f.anchor) } label: {
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Pill(text: f.severity.rawValue.capitalized, color: f.severity == .high ? .ghDel : .ghMuted)
                                Text(f.title).foregroundStyle(Color.ghText).lineLimit(1)
                                Spacer(minLength: 8)
                                Text("\((f.path as NSString).lastPathComponent):\(f.startLine)")
                                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(Color.ghAccent)
                            }
                            .font(.system(size: 12))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            } else if !store.preReviewState.isRunning, !store.findings.isEmpty {
                Text("All findings handled.").font(.system(size: 12)).foregroundStyle(Color.ghMuted)
            }
        }
        .padding(12)
        .background(Color.ghAIBg, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.ghAI.opacity(0.3)))
    }

    @ViewBuilder var status: some View {
        switch store.preReviewState {
        case .running(let m):
            ProgressView().controlSize(.mini)
            Text(m).font(.system(size: 11)).foregroundStyle(Color.ghMuted)
        default:
            if !store.openFindings.isEmpty {
                Text("\(store.openFindings.count) open").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
            }
        }
    }
}

/// Description, comments, reviews and inline threads in time order.
struct ConversationTimeline: View {
    let store: PRStore

    enum Item: Identifiable {
        case comment(PRConversation.Comment)
        case review(PRConversation.Review)
        case thread(ReviewThread)

        var id: String {
            switch self {
            case .comment(let c): "c:\(c.id)"
            case .review(let r): "r:\(r.id)"
            case .thread(let t): "t:\(t.id)"
            }
        }
        var date: Date {
            switch self {
            case .comment(let c): c.createdAt
            case .review(let r): r.submittedAt
            case .thread(let t): t.comments.first?.createdAt ?? .distantPast
            }
        }
    }

    var items: [Item] {
        let conv = store.detail?.conversation
        let all = (conv?.comments ?? []).map(Item.comment) + (conv?.reviews ?? []).map(Item.review)
            + store.threads.values.filter { !$0.comments.isEmpty }.map(Item.thread)
        return all.sorted { $0.date < $1.date }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let d = store.detail {
                Entry(author: d.author, avatar: d.authorAvatarURL, caption: "opened this pull request") {
                    if d.body.isEmpty {
                        Text("No description provided.").italic().foregroundStyle(Color.ghMuted)
                    } else {
                        MarkdownView(text: d.body)
                    }
                }
            }
            ForEach(items) { item in
                switch item {
                case .comment(let c):
                    Entry(author: c.author, avatar: c.authorAvatarURL, caption: "commented \(c.createdAt.relative)") {
                        MarkdownView(text: c.body)
                    }
                case .review(let r):
                    ReviewEntry(review: r)
                case .thread(let t):
                    ThreadEntry(store: store, thread: t)
                }
            }
            if store.detail?.conversation == nil {
                Text("Comments, reviews and commits load with the pull request. Refresh (⌘R) to fetch them.")
                    .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
            }
        }
    }
}

/// One timeline card: avatar, author, caption, body.
struct Entry<Content: View>: View {
    let author: String
    let avatar: URL?
    let caption: String
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Avatar(url: avatar, size: 28)
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Text(author).fontWeight(.semibold)
                    Text(caption).foregroundStyle(Color.ghMuted)
                    Spacer()
                }
                .font(.system(size: 12))
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Color.ghSubtle)
                Divider()
                content
                    .font(.system(size: 13))
                    .padding(12)
            }
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
    }
}

struct ReviewEntry: View {
    let review: PRConversation.Review

    var body: some View {
        if review.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            // GitHub shows bodiless reviews as a single event line.
            HStack(spacing: 8) {
                icon.frame(width: 28)
                Avatar(url: review.authorAvatarURL, size: 18)
                Text(review.author).fontWeight(.semibold)
                Text("\(verb) \(review.submittedAt.relative)").foregroundStyle(Color.ghMuted)
            }
            .font(.system(size: 12))
            .padding(.vertical, 2)
        } else {
            Entry(author: review.author, avatar: review.authorAvatarURL, caption: "\(verb) \(review.submittedAt.relative)") {
                MarkdownView(text: review.body)
            }
        }
    }

    var verb: String {
        switch review.state {
        case "APPROVED": "approved these changes"
        case "CHANGES_REQUESTED": "requested changes"
        case "DISMISSED": "left a dismissed review"
        default: "reviewed"
        }
    }

    @ViewBuilder var icon: some View {
        switch review.state {
        case "APPROVED": Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.ghAdd)
        case "CHANGES_REQUESTED": Image(systemName: "exclamationmark.circle.fill").foregroundStyle(Color.ghDel)
        default: Image(systemName: "eye.circle").foregroundStyle(Color.ghMuted)
        }
    }
}

/// An inline review thread: location, first comment, reply count. Click to open it in the diff.
struct ThreadEntry: View {
    let store: PRStore
    let thread: ReviewThread

    var body: some View {
        let first = thread.comments[0]
        Entry(author: first.author, avatar: first.authorAvatarURL, caption: "commented on code \(first.createdAt.relative)") {
            VStack(alignment: .leading, spacing: 8) {
                Button {
                    store.scrollTo(anchor: CodeAnchor(path: thread.path, side: thread.side,
                                                      startLine: thread.startLine ?? thread.anchorLine, endLine: thread.anchorLine))
                } label: {
                    HStack(spacing: 6) {
                        Text("\(thread.path):\(thread.anchorLine)")
                            .font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.ghAccent)
                        if thread.isResolved { Pill(text: "Resolved", color: .ghMuted) }
                        if thread.isOutdated { Pill(text: "Outdated", color: .ghAttention) }
                        if first.isPending { Pill(text: "Pending", color: .ghAttention) }
                    }
                }
                .buttonStyle(.plain)
                .help("Show in the diff")
                MarkdownView(text: first.body)
                if thread.comments.count > 1 {
                    Text(thread.comments.count == 2 ? "1 reply" : "\(thread.comments.count - 1) replies")
                        .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                }
            }
        }
    }
}

struct CommitList: View {
    let store: PRStore

    var body: some View {
        let commits = store.detail?.conversation?.commits ?? []
        VStack(alignment: .leading, spacing: 0) {
            ForEach(commits) { c in
                row(sha: c.oid, headline: c.headline, author: c.author, avatar: c.authorAvatarURL, date: c.date, checks: c.checks)
                Divider()
            }
            ForEach(store.localCommits) { c in
                row(sha: c.sha, headline: c.subject, author: c.author, avatar: nil, date: c.date, checks: .none, local: true)
                Divider()
            }
            if commits.isEmpty && store.localCommits.isEmpty {
                Text("No commits loaded.").font(.system(size: 12)).foregroundStyle(Color.ghMuted).padding(12)
            }
        }
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }

    func row(sha: String, headline: String, author: String, avatar: URL?, date: Date?, checks: CheckState, local: Bool = false) -> some View {
        HStack(spacing: 8) {
            Avatar(url: avatar, size: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(headline).font(.system(size: 13, weight: .medium)).lineLimit(1)
                HStack(spacing: 4) {
                    Text(author).fontWeight(.semibold)
                    if let date { Text("committed \(date.relative)") }
                }
                .font(.system(size: 11)).foregroundStyle(Color.ghMuted)
            }
            Spacer(minLength: 8)
            if local { Pill(text: "Local", color: .ghAttention) }
            switch checks {
            case .success: Image(systemName: "checkmark").foregroundStyle(Color.ghAdd).help("Checks passed")
            case .failure, .error: Image(systemName: "xmark").foregroundStyle(Color.ghDel).help("Checks failing")
            case .pending, .expected: Image(systemName: "circle.fill").font(.system(size: 6)).foregroundStyle(Color.ghAttention).help("Checks running")
            case .none: EmptyView()
            }
            Text(String(sha.prefix(7)))
                .font(.system(size: 11, design: .monospaced)).foregroundStyle(Color.ghMuted)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }
}

/// Review decision, checks and whether the PR can merge, like GitHub's merge box.
struct MergeStatusCard: View {
    let store: PRStore
    @State private var showChecks = true

    var body: some View {
        if let d = store.detail, let st = d.status {
            VStack(alignment: .leading, spacing: 0) {
                review(st)
                if !st.checks.isEmpty {
                    Divider()
                    checks(st.checks)
                }
                Divider()
                merge(st, detail: d)
                if d.state == "OPEN", d.viewerCanPush, st.mergeStateStatus != "DRAFT" {
                    MergeButton(store: store, status: st)
                        .padding(.horizontal, 12).padding(.bottom, 12)
                }
            }
            .font(.system(size: 13))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
    }

    func row(_ icon: String, _ color: Color, _ title: String, _ detail: String? = nil) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).foregroundStyle(color).font(.system(size: 16)).frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).fontWeight(.semibold)
                if let detail { Text(detail).font(.system(size: 12)).foregroundStyle(Color.ghMuted) }
            }
            Spacer(minLength: 0)
        }
        .padding(12)
    }

    @ViewBuilder func review(_ st: PRMergeStatus) -> some View {
        switch st.reviewDecision {
        case "APPROVED": row("checkmark.circle.fill", .ghAdd, "Changes approved")
        case "CHANGES_REQUESTED": row("xmark.circle.fill", .ghDel, "Changes requested", "A reviewer asked for changes before merging.")
        case "REVIEW_REQUIRED": row("exclamationmark.circle.fill", .ghAttention, "Review required", "At least one approving review is required.")
        default: row("person.crop.circle", .ghMuted, "No review required")
        }
    }

    func checks(_ checks: [PRMergeStatus.Check]) -> some View {
        let failed = checks.filter(\.isFailure).count, pending = checks.filter(\.isPending).count
        let ok = checks.count - failed - pending
        let (icon, color, title): (String, Color, String) =
            failed > 0 ? ("xmark.circle.fill", .ghDel, "Some checks were not successful")
            : pending > 0 ? ("clock.fill", .ghAttention, "Some checks haven't completed yet")
            : ("checkmark.circle.fill", .ghAdd, "All checks have passed")
        var parts: [String] = []
        if failed > 0 { parts.append("\(failed) failing") }
        if pending > 0 { parts.append("\(pending) in progress") }
        if ok > 0 { parts.append("\(ok) successful") }
        return VStack(alignment: .leading, spacing: 0) {
            Button { showChecks.toggle() } label: {
                HStack {
                    row(icon, color, title, parts.joined(separator: ", ") + (checks.count == 1 ? " check" : " checks"))
                    Image(systemName: showChecks ? "chevron.up" : "chevron.down").foregroundStyle(Color.ghMuted).padding(.trailing, 12)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if showChecks {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(checks.sorted { ($0.isFailure ? 0 : $0.isPending ? 1 : 2, $0.name) < ($1.isFailure ? 0 : $1.isPending ? 1 : 2, $1.name) }) { c in
                        HStack(spacing: 8) {
                            checkIcon(c).frame(width: 20)
                            Text(c.name).fontWeight(.medium)
                            if let app = c.app { Text(app).foregroundStyle(Color.ghMuted) }
                            Spacer()
                            if let url = c.url {
                                Button("Details") { NSWorkspace.shared.open(url) }.buttonStyle(.link)
                            }
                        }
                        .font(.system(size: 12))
                        .padding(.horizontal, 12).padding(.vertical, 5)
                    }
                }
                .padding(.bottom, 8)
                .background(Color.ghSubtle.opacity(0.5))
            }
        }
    }

    @ViewBuilder func checkIcon(_ c: PRMergeStatus.Check) -> some View {
        if c.isSuccess { Image(systemName: "checkmark").foregroundStyle(Color.ghAdd) }
        else if c.isPending { Image(systemName: "circle.fill").font(.system(size: 7)).foregroundStyle(Color.ghAttention) }
        else { Image(systemName: "xmark").foregroundStyle(Color.ghDel) }
    }

    @ViewBuilder func merge(_ st: PRMergeStatus, detail d: PRDetail) -> some View {
        if d.state == "MERGED" {
            row("arrow.triangle.merge", .ghAI, "Pull request merged")
        } else if d.state == "CLOSED" {
            row("xmark.circle", .ghDel, "Pull request closed")
        } else if st.mergeable == "CONFLICTING" || st.mergeStateStatus == "DIRTY" {
            row("exclamationmark.triangle.fill", .ghDel, "This branch has conflicts that must be resolved")
        } else {
            switch st.mergeStateStatus {
            case "CLEAN", "HAS_HOOKS": row("checkmark.circle.fill", .ghAdd, "Ready to merge", "No conflicts with the base branch.")
            case "UNSTABLE": row("exclamationmark.circle.fill", .ghAttention, "Can merge, but some checks are failing")
            case "BEHIND": row("arrow.down.circle.fill", .ghAttention, "This branch is out-of-date with the base branch")
            case "DRAFT": row("circle.dashed", .ghMuted, "This pull request is still a draft")
            case "BLOCKED": row("nosign", .ghDel, "Merging is blocked", "Required reviews or checks haven't passed yet.")
            default: row("questionmark.circle", .ghMuted, "Checking mergeability…", "GitHub is still computing this. Refresh (⌘R) to update.")
            }
        }
    }
}

/// GitHub's merge split button: pick a method, press, then confirm (merging can't be undone).
struct MergeButton: View {
    let store: PRStore
    let status: PRMergeStatus
    @AppStorage("mergeMethod") private var savedMethod = ""
    @State private var confirming = false
    @State private var merging = false
    @State private var error: String?
    @State private var bypass = false

    var methods: [MergeMethod] { status.allowedMergeMethods.flatMap { $0.isEmpty ? nil : $0 } ?? MergeMethod.allCases }
    var method: MergeMethod {
        if let m = MergeMethod(rawValue: savedMethod), methods.contains(m) { return m }
        if let m = status.defaultMergeMethod, methods.contains(m) { return m }
        return methods[0]
    }
    var enabled: Bool { status.canMerge || (status.canBypass && bypass) }
    var tint: Color { status.canMerge ? .ghAdd : .ghDel }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if confirming {
                HStack(spacing: 8) {
                    Button {
                        Task {
                            merging = true
                            error = await store.mergePullRequest(method: method)
                            merging = false
                            confirming = false
                        }
                    } label: {
                        if merging { ProgressView().controlSize(.small) } else { Text(confirmTitle) }
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(tint)
                    .disabled(merging)
                    Button("Cancel") { confirming = false }
                        .buttonStyle(GHButtonStyle(kind: .normal, compact: true))
                        .disabled(merging)
                    Text("Merges #\(store.ref.number) into \(store.detail?.baseRefName ?? "base") on GitHub.")
                        .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                }
            } else {
                HStack(spacing: 8) {
                    Group {
                        // One allowed method (repo setting): a plain button, as on GitHub.
                        if methods.count > 1 {
                            Menu {
                                ForEach(methods, id: \.self) { m in
                                    Button { savedMethod = m.rawValue } label: {
                                        if m == method { Label(m.title, systemImage: "checkmark") } else { Text(m.title) }
                                    }
                                }
                            } label: {
                                Text(method.buttonTitle)
                            } primaryAction: {
                                error = nil
                                confirming = true
                            }
                            .menuStyle(.button)
                        } else {
                            Button(method.buttonTitle) {
                                error = nil
                                confirming = true
                            }
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(tint)
                    .fixedSize()
                    .disabled(!enabled)
                    if !status.canMerge && !status.canBypass {
                        Text(status.mergeable == "CONFLICTING" ? "Resolve conflicts first." : "Not mergeable yet.")
                            .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                    }
                }
            }
            if status.canBypass && !confirming {
                Toggle("Merge without waiting for requirements to be met (bypass rules)", isOn: $bypass)
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12))
            }
            if let error {
                Text(error + " Refresh (⌘R) and try again.").font(.system(size: 12)).foregroundStyle(Color.ghDel)
            }
        }
    }

    var confirmTitle: String {
        switch method {
        case .merge: "Confirm merge"
        case .squash: "Confirm squash and merge"
        case .rebase: "Confirm rebase and merge"
        }
    }
}

/// Leave a PR comment, like the comment box at the bottom of GitHub's Conversation tab.
struct CommentComposer: View {
    let store: PRStore
    @State private var text = ""
    @State private var posting = false
    @State private var closing = false
    @State private var error: String?

    /// GitHub lets the author and anyone with write access close or reopen.
    var canClose: Bool {
        guard let d = store.detail, d.state != "MERGED" else { return false }
        return d.viewerCanPush || d.author == d.viewerLogin
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Add a comment").font(.system(size: 13, weight: .semibold))
            TextEditor(text: $text)
                .font(.system(size: 13))
                .scrollContentBackground(.hidden)
                .padding(6)
                .frame(minHeight: 90, maxHeight: 220)
                .background(Color.ghCanvas, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
                .overlay(alignment: .topLeading) {
                    if text.isEmpty {
                        Text("Leave a comment (Markdown)").foregroundStyle(Color.ghMuted).font(.system(size: 13))
                            .padding(.horizontal, 11).padding(.vertical, 6).allowsHitTesting(false)
                    }
                }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(Color.ghDel) }
            HStack {
                Text("Posted to GitHub right away, outside your review.").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                Spacer()
                if canClose, let d = store.detail {
                    let closed = d.state == "CLOSED"
                    Button {
                        Task {
                            closing = true
                            // Like GitHub, a typed comment is posted before closing.
                            if !closed, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                error = await store.addConversationComment(text)
                                if error == nil { text = "" }
                            }
                            if error == nil { error = await store.setClosed(!closed) }
                            closing = false
                        }
                    } label: {
                        if closing { ProgressView().controlSize(.small) }
                        else { Label(closed ? "Reopen pull request" : (text.isEmpty ? "Close pull request" : "Close with comment"),
                                     systemImage: closed ? "arrow.uturn.backward.circle" : "xmark.circle") }
                    }
                    .buttonStyle(GHButtonStyle(kind: closed ? .normal : .danger, compact: true))
                    .disabled(closing || posting)
                }
                Button {
                    Task {
                        posting = true
                        error = await store.addConversationComment(text)
                        if error == nil { text = "" }
                        posting = false
                    }
                } label: {
                    if posting { ProgressView().controlSize(.small) } else { Text("Comment") }
                }
                .buttonStyle(GHButtonStyle(kind: .primary, compact: true))
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(posting || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }
}
