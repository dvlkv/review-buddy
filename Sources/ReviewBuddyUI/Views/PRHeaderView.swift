import SwiftUI
import ReviewBuddyCore

struct PRHeaderView: View {
    let store: PRStore
    @State private var showDescription = false

    var body: some View {
        if let d = store.detail {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(d.title).font(.system(size: 18, weight: .semibold)).textSelection(.enabled).lineLimit(2)
                    Text("#\(d.ref.number)").font(.system(size: 18)).foregroundStyle(Color.ghMuted)
                    Spacer()
                }
                HStack(spacing: 6) {
                    stateBadge(d)
                    Avatar(url: d.authorAvatarURL, size: 18)
                    Text(d.author).fontWeight(.semibold).lineLimit(1).fixedSize()
                    Text("wants to merge into").foregroundStyle(Color.ghMuted).lineLimit(1).fixedSize()
                    BranchLabel(name: d.baseRefName)
                    Image(systemName: "arrow.left").font(.system(size: 10)).foregroundStyle(Color.ghMuted)
                    BranchLabel(name: d.headRefName)
                    DiffStat(additions: d.additions, deletions: d.deletions).fixedSize()
                    Text(d.changedFiles == 1 ? "1 file" : "\(d.changedFiles) files").foregroundStyle(Color.ghMuted).lineLimit(1).fixedSize()
                    Spacer(minLength: 4)
                    if !d.body.isEmpty {
                        Button {
                            withAnimation(.snappy(duration: 0.2)) { showDescription.toggle() }
                        } label: {
                            Label(showDescription ? "Hide description" : "Description",
                                  systemImage: showDescription ? "chevron.up" : "text.alignleft")
                                .lineLimit(1).fixedSize()
                        }
                        .buttonStyle(GHButtonStyle(kind: .invisible, compact: true))
                    }
                }
                .font(.system(size: 12))
                if showDescription {
                    MarkdownView(text: d.body)
                        .padding(12)
                        .background(Color.ghCanvas, in: RoundedRectangle(cornerRadius: 6))
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
                        .frame(maxHeight: 260)
                }
                SummaryCard(store: store)
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
            .padding(.vertical, 12)
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

/// AI summary + reading order (§6.1).
struct SummaryCard: View {
    let store: PRStore
    @State private var collapsed = false
    @State private var expanded = false

    var body: some View {
        if store.summary != nil || store.summaryState != .idle {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "sparkles").foregroundStyle(Color.ghAI)
                    Text("AI summary").font(.system(size: 12, weight: .semibold)).foregroundStyle(Color.ghAI)
                    if case .running(let msg) = store.summaryState {
                        ProgressView().controlSize(.mini)
                        Text(msg).font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                    }
                    if let s = store.summary, s.headSHA != store.detail?.headOid {
                        Pill(text: "Outdated", color: .ghAttention)
                    }
                    Spacer()
                    if store.summary != nil {
                        Button { Task { await store.summarize() } } label: { Image(systemName: "arrow.clockwise") }
                            .buttonStyle(.plain).foregroundStyle(Color.ghMuted).help("Regenerate")
                            .disabled(store.summaryState.isRunning)
                        Button { collapsed.toggle() } label: { Image(systemName: collapsed ? "chevron.down" : "chevron.up") }
                            .buttonStyle(.plain).foregroundStyle(Color.ghMuted)
                    }
                }
                if case .failed(let e) = store.summaryState {
                    Text(e).font(.system(size: 12)).foregroundStyle(Color.ghDel).lineLimit(4)
                }
                if let s = store.summary, !collapsed {
                    // No fixedSize here: in an early layout pass with a tiny width, a fixed-size
                    // long paragraph reports a huge minimum height that stretches the window.
                    Text(s.summary).font(.system(size: 13)).textSelection(.enabled)
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
                                        Text("— \(item.reason)").foregroundStyle(Color.ghMuted).lineLimit(1)
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
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Color.ghAttention).font(.system(size: 10))
                                if let p = r.path { Text(p).font(.system(size: 12, design: .monospaced)) }
                                Text(r.description).foregroundStyle(Color.ghText).lineLimit(expanded ? nil : 2)
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

/// Local fix commits and remote divergence (§5.1, §5.4).
struct GitStatusBanner: View {
    let store: PRStore

    var body: some View {
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
