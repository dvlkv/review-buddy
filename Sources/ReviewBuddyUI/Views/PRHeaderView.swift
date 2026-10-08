import SwiftUI
import ReviewBuddyCore

struct PRHeaderView: View {
    let store: PRStore
    /// Tallest the description + summary area may get before it scrolls, so the diff
    /// keeps most of the window.
    var maxBodyHeight: CGFloat = 400
    @State private var showDescription = false
    @State private var summaryCollapsed = false

    var body: some View {
        if let d = store.detail {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    stateBadge(d)
                    Avatar(url: d.authorAvatarURL, size: 18)
                    Text(d.author).fontWeight(.semibold).lineLimit(1).fixedSize()
                    Text("wants to merge into").foregroundStyle(Color.ghMuted).lineLimit(1)
                        .layoutPriority(-1)
                    BranchLabel(name: d.baseRefName)
                    Image(systemName: "arrow.left").font(.system(size: 10)).foregroundStyle(Color.ghMuted)
                    BranchLabel(name: d.headRefName)
                    DiffStat(additions: d.additions, deletions: d.deletions).fixedSize()
                    Text(d.changedFiles == 1 ? "1 file" : "\(d.changedFiles) files").foregroundStyle(Color.ghMuted).lineLimit(1).fixedSize()
                    Spacer(minLength: 4)
                    // These toggles stay in this fixed row, so the description and summary
                    // can be opened and closed from anywhere in the PR. No animation: growing
                    // the header resizes the diff below on every frame, which stuttered.
                    if !d.body.isEmpty {
                        Button {
                            showDescription.toggle()
                        } label: {
                            Label(showDescription ? "Hide description" : "Description",
                                  systemImage: showDescription ? "chevron.up" : "text.alignleft")
                                .lineLimit(1).fixedSize()
                        }
                        .buttonStyle(GHButtonStyle(kind: .invisible, compact: true))
                    }
                    // Always here: shows/hides the summary, or generates one if there is none.
                    let hasSummary = store.summary != nil || store.summaryState.isRunning
                    let shown = hasSummary && !summaryCollapsed
                    Button {
                        if hasSummary { summaryCollapsed.toggle() } else {
                            summaryCollapsed = false
                            store.startSummarize()
                        }
                    } label: {
                        Label(shown ? "Hide summary" : "AI summary", systemImage: shown ? "chevron.up" : "sparkles")
                            .lineLimit(1).fixedSize()
                    }
                    .buttonStyle(GHButtonStyle(kind: .invisible, compact: true))
                    .help(hasSummary ? "Show or hide the AI summary" : "Generate an AI summary and reading order")
                }
                .font(.system(size: 12))
                // A narrow column clips the row instead of growing the detail column past
                // the window edge (which pushed the diff's Viewed buttons off-screen).
                .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
                .clipped()
                CappedScroll(maxHeight: maxBodyHeight) {
                    VStack(alignment: .leading, spacing: 8) {
                        if showDescription {
                            MarkdownView(text: d.body)
                                .padding(12)
                                .background(Color.ghCanvas, in: RoundedRectangle(cornerRadius: 6))
                                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
                        }
                        SummaryCard(store: store, collapsed: $summaryCollapsed)
                    }
                }
                GitStatusBanner(store: store)
                if let banner = store.banner {
                    HStack {
                        Image(systemName: "info.circle")
                        Text(banner).lineLimit(2)
                        Spacer()
                        Button { store.banner = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                    }
                    .font(.system(size: 12))
                    .padding(8)
                    .background(Color.ghAccent.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
                    .task(id: banner) {
                        try? await Task.sleep(for: .seconds(6))
                        if store.banner == banner { store.banner = nil }
                    }
                }
            }
            .padding(.horizontal, 16)
            // The title lives in the window toolbar (navigationTitle), not here.
            .padding(.vertical, 12)
            .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
            .background(Color.ghCanvas)
        }
    }

    @ViewBuilder
    func stateBadge(_ d: PRDetail) -> some View {
        if d.isDraft {
            Pill(text: "Draft", color: .ghMuted, filled: true)
        } else if d.state == "MERGED" {
            Pill(text: "Merged", color: .ghAI, filled: true)
        } else if d.state == "CLOSED" {
            Pill(text: "Closed", color: .ghDel, filled: true)
        } else {
            Pill(text: "Open", color: Color(nsColor: NSColor(hex: 0x1F883D)), filled: true)
        }
    }
}

struct BranchLabel: View {
    let name: String
    var body: some View {
        Text(name)
            .font(.system(size: 11.5, design: .monospaced))
            .lineLimit(1)
            .truncationMode(.middle)
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(Color.ghAccent.opacity(0.1), in: RoundedRectangle(cornerRadius: 5))
            .foregroundStyle(Color.ghAccent)
            .textSelection(.enabled)
    }
}

/// Sizes to its content up to `maxHeight`, then scrolls.
struct CappedScroll<Content: View>: View {
    let maxHeight: CGFloat
    @ViewBuilder var content: Content
    @State private var contentHeight: CGFloat = 0

    var body: some View {
        ScrollView(.vertical) {
            content.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { contentHeight = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: min(contentHeight, maxHeight))
    }
}

/// AI summary + reading order (§6.1).
struct SummaryCard: View {
    let store: PRStore
    @Binding var collapsed: Bool
    @State private var expanded = false

    var body: some View {
        // Hidden entirely when collapsed; the header's "AI summary" button brings it back.
        if !collapsed, store.summary != nil || store.summaryState != .idle {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "sparkles").foregroundStyle(Color.ghAI)
                    Text("AI summary").font(.system(size: 12, weight: .semibold)).foregroundStyle(Color.ghAI)
                    if case .running(let msg) = store.summaryState {
                        ProgressView().controlSize(.mini)
                        Text(msg).font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                        Button("Cancel") { store.cancelSummarize() }
                            .buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Color.ghAccent)
                    }
                    if let s = store.summary, s.headSHA != store.detail?.headOid {
                        Pill(text: "Outdated", color: .ghAttention)
                    }
                    Spacer()
                    if store.summary != nil {
                        Button { store.startSummarize() } label: { Image(systemName: "arrow.clockwise") }
                            .buttonStyle(.plain).foregroundStyle(Color.ghMuted).help("Regenerate")
                            .disabled(store.summaryState.isRunning)
                    }
                }
                if case .failed(let e) = store.summaryState {
                    Text(e).font(.system(size: 12)).foregroundStyle(Color.ghDel).lineLimit(4)
                }
                if let s = store.summary {
                    // No fixedSize here: in an early layout pass with a tiny width, a fixed-size
                    // long paragraph reports a huge minimum height that stretches the window.
                    Text(inlineMarkdown: s.summary).font(.system(size: 13)).textSelection(.enabled)
                        .lineLimit(expanded ? nil : 3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if s.summary.count > 240 {
                        Button(expanded ? "Show less" : "Show more") { expanded.toggle() }
                            .buttonStyle(.link).font(.system(size: 12))
                    }
                    if !s.readingOrder.isEmpty {
                        Text("Suggested reading order").font(.system(size: 11, weight: .semibold)).foregroundStyle(Color.ghMuted)
                        VStack(alignment: .leading, spacing: 3) {
                            ForEach(Array(s.readingOrder.prefix(expanded ? 50 : 5).enumerated()), id: \.offset) { i, item in
                                Button {
                                    if let f = store.diff?.fileIndex(path: item.path) { store.scrollToFile(f) }
                                } label: {
                                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                                        Text("\(i + 1).").foregroundStyle(Color.ghMuted).frame(width: 18, alignment: .trailing)
                                        Text(item.path).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.ghAccent)
                                        Text(inlineMarkdown: "— \(item.reason)").foregroundStyle(Color.ghMuted).lineLimit(1)
                                    }
                                    .font(.system(size: 12))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    if !s.risks.isEmpty {
                        Text("Risky areas").font(.system(size: 11, weight: .semibold)).foregroundStyle(Color.ghMuted)
                        ForEach(Array(s.risks.prefix(expanded ? 50 : 3).enumerated()), id: \.offset) { _, r in
                            // File on top, explanation underneath.
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Color.ghAttention).font(.system(size: 10))
                                    if let p = r.path {
                                        Button {
                                            if let f = store.diff?.fileIndex(path: p) { store.scrollToFile(f) }
                                        } label: {
                                            Text(p).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.ghAccent)
                                        }
                                        .buttonStyle(.plain)
                                    } else {
                                        Text("General").font(.system(size: 12, weight: .medium))
                                    }
                                }
                                Text(inlineMarkdown: r.description).foregroundStyle(Color.ghText)
                                    .lineLimit(expanded ? nil : 2)
                                    .padding(.leading, 18)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .font(.system(size: 12))
                        }
                    }
                }
            }
            .padding(12)
            .background(Color.ghAIBg, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.ghAI.opacity(0.3)))
        }
    }
}

/// "New commits were pushed" with a Refresh button (reloads the diff and updates the checkout).
struct OutdatedBar: View {
    let store: PRStore

    var body: some View {
        if store.isOutdated {
            HStack(spacing: 8) {
                Image(systemName: "arrow.triangle.2.circlepath").foregroundStyle(Color.ghAccent)
                Text("This pull request has new commits on GitHub.")
                Spacer()
                Button("Refresh") { Task { await store.refresh() } }
                    .buttonStyle(GHButtonStyle(kind: .primary, compact: true))
                    .help("Reload the diff and update the local checkout (⌘R)")
            }
            .font(.system(size: 12))
            .padding(8)
            .background(Color.ghAccent.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
        }
    }
}

/// Local fix commits and remote divergence (§5.1, §5.4).
struct GitStatusBanner: View {
    let store: PRStore

    var body: some View {
        OutdatedBar(store: store)
        if store.remoteAhead > 0 && !store.localCommits.isEmpty {
            HStack(spacing: 8) {
                Image(systemName: "arrow.triangle.branch").foregroundStyle(Color.ghAttention)
                Text("The PR was updated on GitHub (\(store.remoteAhead) new commit\(store.remoteAhead == 1 ? "" : "s")) · you have \(store.localCommits.count) local fix commit\(store.localCommits.count == 1 ? "" : "s")")
                Spacer()
                Button("Rebase local commits") { Task { await store.rebaseOntoRemote() } }
                    .buttonStyle(GHButtonStyle(compact: true))
            }
            .font(.system(size: 12))
            .padding(8)
            .background(Color.ghAttention.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
        } else if store.remoteAhead > 0 && !store.isOutdated {
            // Refresh fast-forwards a clean checkout; uncommitted edits there block that.
            HStack(spacing: 8) {
                Image(systemName: "arrow.triangle.branch").foregroundStyle(Color.ghAttention)
                Text("The local checkout is \(store.remoteAhead) commit\(store.remoteAhead == 1 ? "" : "s") behind the PR: it has uncommitted changes, so it wasn't updated.")
                Spacer()
            }
            .font(.system(size: 12))
            .padding(8)
            .background(Color.ghAttention.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
        }
        if case .failed(let e) = store.worktreeState {
            HStack {
                Image(systemName: "exclamationmark.triangle").foregroundStyle(Color.ghDel)
                Text("Checkout failed: \(e)").lineLimit(3)
                Spacer()
                Button("Retry") { Task { try? await store.ensureWorktree() } }.buttonStyle(GHButtonStyle(compact: true))
            }
            .font(.system(size: 12))
            .padding(8)
            .background(Color.ghDel.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
        }
    }
}

/// Review submit popover (§4).
struct ReviewSubmitView: View {
    let store: PRStore
    @Binding var isPresented: Bool
    @State private var event: ReviewEvent = .comment

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Finish your review").font(.system(size: 14, weight: .semibold))
            TextEditor(text: Bindable(store).reviewBody)
                .font(.system(size: 13))
                .frame(height: 110)
                .scrollContentBackground(.hidden)
                .padding(4)
                .background(Color.ghSubtle.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            VStack(alignment: .leading, spacing: 8) {
                radio(.comment, "Submit general feedback without explicit approval.")
                radio(.approve, "Give your approval to merge these changes.")
                    .disabled(store.detail?.author == store.detail?.viewerLogin)
                radio(.requestChanges, "Submit feedback that must be addressed before merging.")
                    .disabled(store.detail?.author == store.detail?.viewerLogin)
            }
            if case .failed(let e) = store.submitState {
                Text(e).foregroundStyle(Color.ghDel).font(.system(size: 12))
            }
            HStack {
                Text("\(store.pendingCommentCount) pending comment\(store.pendingCommentCount == 1 ? "" : "s")")
                    .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                if store.pendingReviewId != nil {
                    Button("Discard") { Task { await store.discardPendingReview(); isPresented = false } }
                        .buttonStyle(GHButtonStyle(kind: .danger, compact: true))
                }
                Spacer()
                Button {
                    Task { if await store.submitReview(event: event) { isPresented = false } }
                } label: {
                    if store.submitState.isRunning { ProgressView().controlSize(.small) } else { Text("Submit review") }
                }
                .buttonStyle(GHButtonStyle(kind: .primary))
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(store.submitState.isRunning || (event == .comment && store.reviewBody.isEmpty && store.pendingCommentCount == 0))
            }
        }
        .padding(16)
        .frame(width: 420)
    }

    func radio(_ e: ReviewEvent, _ detail: String) -> some View {
        Button { event = e } label: {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: event == e ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(event == e ? Color.ghAccent : Color.ghMuted)
                VStack(alignment: .leading, spacing: 1) {
                    Text(e.title).font(.system(size: 13, weight: .semibold))
                    Text(detail).font(.system(size: 11.5)).foregroundStyle(Color.ghMuted)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Push confirmation sheet (§5.4).
struct PushSheet: View {
    let store: PRStore
    @Binding var isPresented: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Push fix commits").font(.system(size: 15, weight: .semibold))
            if let d = store.detail {
                HStack(spacing: 4) {
                    Text("These commits will be pushed to")
                    BranchLabel(name: d.headRefName)
                    Text("on \(d.headRepoNameWithOwner ?? d.ref.nameWithOwner).")
                }
                .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
            }
            VStack(alignment: .leading, spacing: 0) {
                ForEach(store.localCommits) { c in
                    HStack(spacing: 8) {
                        Text(c.shortSHA).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.ghAccent)
                        Text(c.subject).lineLimit(1)
                        Spacer()
                        Text(c.author).foregroundStyle(Color.ghMuted)
                    }
                    .font(.system(size: 12))
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    Divider()
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            if let reason = store.pushDisabledReason {
                Label(reason, systemImage: "lock").font(.system(size: 12)).foregroundStyle(Color.ghDel)
            }
            if case .failed(let e) = store.pushState {
                Text(e).font(.system(size: 12)).foregroundStyle(Color.ghDel).lineLimit(5)
            }
            HStack {
                Text("Never force-pushes.").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                Spacer()
                Button("Cancel") { isPresented = false }.keyboardShortcut(.cancelAction)
                Button {
                    Task { if await store.push() { isPresented = false } }
                } label: {
                    if store.pushState.isRunning { ProgressView().controlSize(.small) }
                    else { Text("Push \(store.localCommits.count) commit\(store.localCommits.count == 1 ? "" : "s")") }
                }
                .buttonStyle(GHButtonStyle(kind: .primary))
                .disabled(!store.canPush || store.localCommits.isEmpty || store.pushState.isRunning)
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 520)
    }
}
