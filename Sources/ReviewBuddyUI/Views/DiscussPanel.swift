import SwiftUI
import ReviewBuddyCore

/// Right-hand inspector: the list of Discuss threads and the active conversation.
struct DiscussPanel: View {
    let store: PRStore

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let id = store.activeDiscussId, store.discussThreads[id] != nil {
                ConversationView(store: store, controller: store.controller(for: id))
                    .id(id)
            } else {
                ThreadListView(store: store)
            }
        }
        .background(Color.ghCanvas)
    }

    var header: some View {
        HStack(spacing: 8) {
            if store.activeDiscussId != nil {
                Button { store.activeDiscussId = nil } label: { Image(systemName: "chevron.left") }
                    .buttonStyle(.plain).foregroundStyle(Color.ghMuted).help("All conversations")
            }
            Image(systemName: "sparkle").foregroundStyle(Color.ghAI)
            Text("Discuss").font(.system(size: 13, weight: .semibold))
            Spacer()
            if !store.localCommits.isEmpty {
                Pill(text: "\(store.localCommits.count) local commit\(store.localCommits.count == 1 ? "" : "s")", color: .ghAttention)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 38)
    }
}

struct ThreadListView: View {
    let store: PRStore

    var body: some View {
        if store.discussThreads.isEmpty {
            VStack(spacing: 10) {
                Image(systemName: "sparkle").font(.system(size: 28)).foregroundStyle(Color.ghAI.opacity(0.6))
                Text("Select lines in the diff and press **Discuss** (or D) to ask Claude about them.")
                    .multilineTextAlignment(.center).font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                Text("Claude works in a private checkout of this PR. It can read the whole repo, run commands, and fix what you discuss.")
                    .multilineTextAlignment(.center).font(.system(size: 11)).foregroundStyle(Color.ghMuted)
            }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List {
                ForEach(store.sortedDiscussThreads) { t in
                    Button { store.activeDiscussId = t.id; store.scrollToBlock(id: t.id) } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            HStack {
                                Text(t.anchor.label).font(.system(size: 11, design: .monospaced)).foregroundStyle(Color.ghAccent)
                                if t.status == .outdated { Pill(text: "Outdated", color: .ghAttention) }
                                if let fix = store.fixStatus(threadId: t.id) { FixBadge(fix: fix) }
                                Spacer()
                                Text(t.createdAt.relative).font(.system(size: 10)).foregroundStyle(Color.ghMuted)
                            }
                            Text(t.messages.isEmpty ? "New conversation" : t.title).font(.system(size: 12)).lineLimit(2)
                        }
                        .padding(.vertical, 4)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button("Delete conversation", role: .destructive) { store.deleteDiscuss(t.id) }
                    }
                }
            }
            .listStyle(.plain)
        }
    }
}

struct ConversationView: View {
    let store: PRStore
    let controller: DiscussController
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            if let t = controller.thread {
                anchorHeader(t)
                Divider()
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        if controller.messages.isEmpty {
                            Text("Ask Claude anything about the selected lines — why they're written this way, whether there's a bug, how to test them…")
                                .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                                .padding(.top, 8)
                        }
                        ForEach(controller.messages) { m in
                            MessageView(message: m).id(m.id)
                        }
                        if let status = controller.status {
                            HStack { ProgressView().controlSize(.small); Text(status) }
                                .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                        } else if controller.reducer.isRunning {
                            TypingIndicator().id("typing")
                        }
                        Color.clear.frame(height: 1).id("bottom")
                    }
                    .padding(12)
                }
                .onChange(of: controller.messages.last?.text) { proxy.scrollTo("bottom", anchor: .bottom) }
                .onChange(of: controller.messages.count) { proxy.scrollTo("bottom", anchor: .bottom) }
                .onAppear { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            Divider()
            actions
            inputBox
        }
        .onAppear { inputFocused = true }
    }

    func anchorHeader(_ t: DiscussThread) -> some View {
        Button { store.scrollTo(anchor: t.anchor) } label: {
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Image(systemName: "doc.text").foregroundStyle(Color.ghMuted)
                    Text(t.anchor.label).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.ghAccent)
                    if t.status == .outdated { Pill(text: "Outdated", color: .ghAttention) }
                    Spacer()
                }
                let lines = store.lines(for: t.anchor)
                if !lines.isEmpty {
                    Text(lines.prefix(6).joined(separator: "\n") + (lines.count > 6 ? "\n…" : ""))
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(Color.ghMuted)
                        .lineLimit(7)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(6)
                        .background(Color.ghSubtle, in: RoundedRectangle(cornerRadius: 5))
                }
            }
            .padding(10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    var actions: some View {
        let hasAnswer = controller.messages.contains { $0.role == .assistant }
        return HStack(spacing: 6) {
            Button {
                Task { await controller.convertToComment() }
            } label: {
                if controller.draftState.isRunning { ProgressView().controlSize(.mini) }
                Label("Comment", systemImage: "arrow.turn.down.right")
            }
            .buttonStyle(GHButtonStyle(compact: true))
            .disabled(!hasAnswer || controller.isRunning || controller.draftState.isRunning)
            .help("Draft a review comment from this conversation")

            Button {
                Task { await controller.fixIt() }
            } label: {
                Label(controller.isFixing ? "Fixing…" : "Fix it", systemImage: "hammer")
            }
            .buttonStyle(GHButtonStyle(kind: .ai, compact: true))
            .disabled(!hasAnswer || controller.isRunning)
            .help("Ask Claude to implement the fix and commit it to the PR branch (not pushed)")

            if !store.localCommits.isEmpty {
                Button { NotificationCenter.default.post(name: .rbShowPush, object: nil) } label: {
                    Label("Push…", systemImage: "arrow.up.circle")
                }
                .buttonStyle(GHButtonStyle(compact: true))
            }
            Spacer()
            if case .failed(let e) = controller.draftState {
                Image(systemName: "exclamationmark.triangle").foregroundStyle(Color.ghDel).help(e)
            }
            if let fix = store.fixStatus(threadId: controller.threadId) { FixBadge(fix: fix) }
        }
        .padding(.horizontal, 10).padding(.top, 8)
    }

    var inputBox: some View {
        VStack(spacing: 6) {
            ZStack(alignment: .topLeading) {
                if controller.input.isEmpty {
                    Text(controller.messages.isEmpty ? "Ask about these lines…" : "Reply to Claude…")
                        .foregroundStyle(Color.ghMuted).padding(.horizontal, 9).padding(.vertical, 8)
                }
                TextEditor(text: Bindable(controller).input)
                    .font(.system(size: 13))
                    .scrollContentBackground(.hidden)
                    .padding(4)
                    .frame(minHeight: 56, maxHeight: 160)
                    .fixedSize(horizontal: false, vertical: true)
                    .focused($inputFocused)
            }
            .background(Color.ghSubtle.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(inputFocused ? Color.ghAI : Color.ghBorder, lineWidth: inputFocused ? 1.5 : 1))
            HStack {
                Text("⌘↩ send").font(.system(size: 10.5)).foregroundStyle(Color.ghMuted)
                Spacer()
                if controller.reducer.isRunning {
                    Button { controller.stop() } label: { Label("Stop", systemImage: "stop.fill") }
                        .buttonStyle(GHButtonStyle(kind: .danger, compact: true))
                        .keyboardShortcut(".", modifiers: .command)
                }
                Button {
                    let text = controller.input
                    controller.input = ""
                    Task { await controller.send(text) }
                } label: { Label("Send", systemImage: "arrow.up") }
                    .buttonStyle(GHButtonStyle(kind: .ai, compact: true))
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(controller.input.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || controller.isRunning)
            }
        }
        .padding(10)
    }
}

struct MessageView: View {
    let message: DiscussMessage
    @State private var expanded = false

    var body: some View {
        switch message.role {
        case .user:
            HStack {
                Spacer(minLength: 30)
                Text(message.text)
                    .font(.system(size: 13))
                    .textSelection(.enabled)
                    .padding(.horizontal, 10).padding(.vertical, 7)
                    .background(Color.ghAccent.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
            }
        case .assistant:
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "sparkle").foregroundStyle(Color.ghAI).font(.system(size: 12)).padding(.top, 2)
                MarkdownView(text: message.text)
                    .opacity(message.isStreaming ? 0.92 : 1)
            }
        case .tool:
            VStack(alignment: .leading, spacing: 4) {
                Button { expanded.toggle() } label: {
                    HStack(spacing: 6) {
                        Image(systemName: expanded ? "chevron.down" : "chevron.right").font(.system(size: 9, weight: .bold))
                        Image(systemName: toolIcon).font(.system(size: 11))
                        Text(message.toolSummary).lineLimit(1).truncationMode(.middle)
                        if message.toolResult == nil { ProgressView().controlSize(.mini) }
                        if message.toolFailed { Image(systemName: "xmark.circle.fill").foregroundStyle(Color.ghDel) }
                        Spacer()
                    }
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundStyle(Color.ghMuted)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                if expanded {
                    ScrollView([.horizontal, .vertical]) {
                        Text((message.toolInput ?? "") + "\n\n" + (message.toolResult ?? "…"))
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                            .padding(8)
                    }
                    .frame(maxHeight: 220)
                    .background(Color.ghSubtle, in: RoundedRectangle(cornerRadius: 6))
                }
            }
            .padding(.leading, 20)
        case .error:
            Label(message.text, systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 12)).foregroundStyle(Color.ghDel)
                .textSelection(.enabled)
        case .system:
            Text(message.text).font(.system(size: 11)).foregroundStyle(Color.ghMuted).frame(maxWidth: .infinity)
        }
    }

    var toolIcon: String {
        switch message.toolName {
        case "Read": "doc.text"
        case "Edit", "Write", "MultiEdit": "pencil"
        case "Bash": "terminal"
        case "Grep", "Glob": "magnifyingglass"
        default: "wrench.and.screwdriver"
        }
    }
}

struct TypingIndicator: View {
    @State private var phase = 0.0
    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<3) { i in
                Circle().fill(Color.ghAI).frame(width: 5, height: 5)
                    .opacity(0.3 + 0.7 * max(0, sin(phase + Double(i) * 0.8)))
            }
        }
        .padding(.leading, 22)
        .onAppear { withAnimation(.linear(duration: 1.2).repeatForever(autoreverses: false)) { phase = .pi * 2 } }
    }
}

extension Notification.Name {
    static let rbShowPush = Notification.Name("rbShowPush")
}
