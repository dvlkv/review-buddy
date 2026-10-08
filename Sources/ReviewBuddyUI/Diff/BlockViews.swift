import SwiftUI
import ReviewBuddyCore

// MARK: - Inline blocks

struct BlockView: View {
    let store: PRStore
    let block: InlineBlockRef

    var body: some View {
        Group {
            switch block.kind {
            case .reviewThread: ThreadBlockView(store: store, threadId: block.id)
            case .composer: ComposerView(store: store, composerId: block.id)
            case .discuss: DiscussMarkerView(store: store, discussId: block.id)
            case .finding: FindingBlockView(store: store, findingId: block.id)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Color.ghCanvas)
    }
}

struct ThreadBlockView: View {
    let store: PRStore
    let threadId: String
    @State private var expandedResolved = false

    var body: some View {
        if let t = store.threads[threadId] {
            let collapsed = t.isResolved && !expandedResolved
            VStack(alignment: .leading, spacing: 0) {
                if t.isResolved || t.isOutdated {
                    HStack(spacing: 6) {
                        if t.isResolved {
                            Image(systemName: "checkmark.circle").foregroundStyle(Color.ghMuted)
                            Text("Resolved").fontWeight(.medium)
                        }
                        if t.isOutdated { Pill(text: "Outdated", color: .ghAttention) }
                        if let first = t.comments.first, collapsed {
                            Text("@\(first.author): \(first.body.replacingOccurrences(of: "\n", with: " "))")
                                .lineLimit(1).foregroundStyle(Color.ghMuted)
                        }
                        Spacer()
                        if t.isResolved {
                            Button(collapsed ? "Show" : "Hide") { expandedResolved.toggle() }.buttonStyle(.link)
                        }
                    }
                    .font(.system(size: 12))
                    .padding(.horizontal, 12).padding(.vertical, 7)
                    .background(Color.ghSubtle)
                    if !collapsed { Divider() }
                }
                if !collapsed {
                    ForEach(t.comments) { c in
                        CommentView(store: store, thread: t, comment: c)
                        Divider().opacity(0.6)
                    }
                    if let composer = store.composerFor(reply: threadId) {
                        InlineComposer(store: store, composer: composer, placeholder: "Reply…")
                            .padding(10)
                    } else {
                        HStack(spacing: 8) {
                            Button { store.startReply(threadId: threadId) } label: {
                                Text("Reply…").foregroundStyle(Color.ghMuted)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 10).padding(.vertical, 5)
                                    .background(Color.ghCanvas, in: RoundedRectangle(cornerRadius: 6))
                                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
                            }
                            .buttonStyle(.plain)
                            Button(t.isResolved ? "Unresolve conversation" : "Resolve conversation") {
                                Task { await store.setResolved(threadId: threadId, !t.isResolved) }
                            }
                            .buttonStyle(GHButtonStyle(compact: true))
                        }
                        .padding(10)
                        .background(Color.ghSubtle)
                    }
                }
            }
            .background(Color.ghCanvas)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            .frame(maxWidth: 880, alignment: .leading)
        }
    }
}

struct CommentView: View {
    let store: PRStore
    let thread: ReviewThread
    let comment: ReviewComment

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Avatar(url: comment.authorAvatarURL, size: 20)
                Text(comment.author).fontWeight(.semibold)
                Text(comment.createdAt.relative).foregroundStyle(Color.ghMuted)
                if comment.isPending { Pill(text: "Pending", color: .ghAttention) }
                Spacer()
                if comment.viewerDidAuthor {
                    Menu {
                        Button("Edit") { store.startEdit(threadId: thread.id, comment: comment) }
                        Button("Delete", role: .destructive) {
                            Task { await store.deleteComment(threadId: thread.id, commentId: comment.id) }
                        }
                        if let url = comment.url { Button("Copy link") { copy(url.absoluteString) } }
                    } label: { Image(systemName: "ellipsis") }
                        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                }
            }
            .font(.system(size: 12))
            if let editor = store.composerFor(edit: comment.id) {
                InlineComposer(store: store, composer: editor, placeholder: "Edit comment")
            } else {
                MarkdownView(text: comment.body, suggestionContext: suggestionContext)
            }
        }
        .padding(12)
    }

    var suggestionContext: [String] {
        guard let line = thread.line else { return [] }
        return store.lines(for: CodeAnchor(path: thread.path, side: thread.side, startLine: thread.startLine ?? line, endLine: line))
    }

    func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }
}

/// New-thread composer block (GitHub's "Add a comment" box).
struct ComposerView: View {
    let store: PRStore
    let composerId: String

    var body: some View {
        if let c = store.composer(composerId) {
            VStack(alignment: .leading, spacing: 0) {
                if let a = c.anchor {
                    HStack(spacing: 6) {
                        Text(a.isMultiline ? "Commenting on lines \(a.side == .left ? "L" : "R")\(a.startLine) to \(a.side == .left ? "L" : "R")\(a.endLine)"
                                           : "Commenting on line \(a.side == .left ? "L" : "R")\(a.startLine)")
                        if let origin = c.origin { Pill(text: origin, color: .ghAI) }
                        Spacer()
                    }
                    .font(.system(size: 12)).foregroundStyle(Color.ghMuted)
                    .padding(.horizontal, 12).padding(.top, 10)
                }
                InlineComposer(store: store, composer: c, placeholder: "Leave a comment")
                    .padding(10)
            }
            .background(Color.ghCanvas)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghBorder))
            .frame(maxWidth: 880, alignment: .leading)
        }
    }
}

/// Write/Preview editor with suggestion insertion; ⌘↩ submits, Esc cancels.
struct InlineComposer: View {
    let store: PRStore
    let composer: Composer
    let placeholder: String
    @State private var text: String = ""
    @State private var preview = false
    @FocusState private var focused: Bool

    func submit() {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, !composer.isSubmitting else { return }
        store.updateComposer(composer.id, text: text)
        Task { await store.submitComposer(composer.id) }
    }

    /// 80–300 pt, growing by one line per newline (long wrapped lines scroll inside).
    var editorHeight: CGFloat {
        let lines = text.reduce(1) { $0 + ($1 == "\n" ? 1 : 0) }
        return min(300, max(80, CGFloat(lines) * 17 + 24))
    }

    var submitTitle: String {
        switch composer.kind {
        case .newThread: store.pendingReviewId == nil ? "Start a review" : "Add review comment"
        case .reply: "Reply"
        case .edit: "Update comment"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 0) {
                tab("Write", !preview) { preview = false }
                tab("Preview", preview) { preview = true }
                Spacer()
                if composer.anchor != nil {
                    Button {
                        text += (text.isEmpty || text.hasSuffix("\n") ? "" : "\n") + MarkdownParser.suggestionTemplate(for: composer.anchorLines)
                    } label: {
                        Image(systemName: "plus.forwardslash.minus")
                    }
                    .buttonStyle(.plain).foregroundStyle(Color.ghMuted)
                    .help("Insert a suggestion")
                }
            }
            if preview {
                MarkdownView(text: text.isEmpty ? "_Nothing to preview_" : text, suggestionContext: composer.anchorLines)
                    .padding(8)
                    .frame(minHeight: 80, alignment: .topLeading)
            } else {
                ZStack(alignment: .topLeading) {
                    if text.isEmpty {
                        Text(placeholder).foregroundStyle(Color.ghMuted).padding(.horizontal, 9).padding(.vertical, 8)
                    }
                    // Explicit height from the line count: asking TextEditor for its ideal size
                    // (fixedSize) was half the cost of opening a composer.
                    TextEditor(text: $text)
                        .font(.system(size: 13))
                        .scrollContentBackground(.hidden)
                        .padding(4)
                        .focused($focused)
                        .frame(height: editorHeight)
                        // ⌘↩ submits, Esc cancels. Handled on the editor rather than with
                        // .keyboardShortcut, which re-registers key equivalents app-wide
                        // (~4 ms every time a composer opens).
                        .onKeyPress(keys: [.return, .escape], phases: .down) { press in
                            if press.key == .escape { store.cancelComposer(composer.id); return .handled }
                            if press.modifiers.contains(.command) { submit(); return .handled }
                            return .ignored
                        }
                }
                .background(Color.ghSubtle.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(focused ? Color.ghAccent : Color.ghBorder, lineWidth: focused ? 2 : 1))
            }
            if let err = composer.error {
                Text(err).font(.system(size: 12)).foregroundStyle(Color.ghDel)
            }
            HStack {
                Text("Markdown supported · ⌘↩ to submit").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                Spacer()
                Button("Cancel") { store.cancelComposer(composer.id) }
                    .buttonStyle(GHButtonStyle(compact: true))
                    
                Button { submit() } label: {
                    if composer.isSubmitting { ProgressView().controlSize(.small) } else { Text(submitTitle) }
                }
                .buttonStyle(GHButtonStyle(kind: .primary, compact: true))
                
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || composer.isSubmitting)
            }
        }
        .onAppear {
            text = composer.text
            focused = true
        }
        .onChange(of: composer.text) { _, new in if new != text { text = new } }
        // A reused composer view is handed a different composer: reset its local state.
        .onChange(of: composer.id) { _, _ in
            text = composer.text
            preview = false
            focused = true
        }
        .onChange(of: text) { _, new in store.updateComposer(composer.id, text: new) }
    }

    func tab(_ title: String, _ active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: active ? .semibold : .regular))
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(active ? Color.ghSubtle : Color.clear, in: RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }
}

struct DiscussMarkerView: View {
    let store: PRStore
    let discussId: String

    var body: some View {
        if let t = store.discussThreads[discussId] {
            let active = store.activeDiscussId == discussId
            let controller = store.controllers[discussId]
            let running = controller?.isRunning ?? false
            Button {
                // Clicking the open conversation again shows or hides the panel.
                if active { NotificationCenter.default.post(name: .rbToggleDiscussPanel, object: nil) }
                else { store.activeDiscussId = discussId }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "sparkle").foregroundStyle(Color.ghAI)
                    Text("Discuss").fontWeight(.semibold).foregroundStyle(Color.ghAI)
                    Text(t.messages.isEmpty ? "New conversation — ask Claude about \(t.anchor.label)" : t.title)
                        .lineLimit(1).foregroundStyle(Color.ghText)
                    if t.status == .outdated { Pill(text: "Outdated", color: .ghAttention) }
                    if let fix = store.fixStatus(threadId: discussId) { FixBadge(fix: fix) }
                    Spacer()
                    if running { ProgressView().controlSize(.mini) }
                    let n = t.messages.filter { $0.role == .user || $0.role == .assistant }.count
                    if n > 0 { Text("\(n) messages").foregroundStyle(Color.ghMuted) }
                    Image(systemName: "sidebar.right").foregroundStyle(Color.ghMuted)
                }
                .font(.system(size: 12))
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Color.ghAIBg, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.ghAI.opacity(active ? 0.9 : 0.35), lineWidth: active ? 1.5 : 1))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(maxWidth: 880, alignment: .leading)
        }
    }
}

struct FixBadge: View {
    let fix: FixRequest
    var body: some View {
        switch fix.status {
        case .queued: Pill(text: "Fix queued", color: .ghMuted)
        case .running: Pill(text: "Fixing…", color: .ghAI)
        case .committed: Pill(text: "Fixed in \(fix.commits.first.map { String($0.prefix(7)) } ?? "")", color: .ghAdd)
        case .failed: Pill(text: "Fix failed", color: .ghDel).help(fix.error ?? "")
        }
    }
}

struct FindingBlockView: View {
    let store: PRStore
    let findingId: String

    var body: some View {
        if let f = store.findings[findingId] {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "sparkles").foregroundStyle(Color.ghAI)
                    Text("AI pre-review").font(.system(size: 11, weight: .semibold)).foregroundStyle(Color.ghAI)
                    SeverityPill(severity: f.severity)
                    if store.findingsAreOutdated { Pill(text: "Outdated", color: .ghAttention).help("The PR changed since this pre-review ran") }
                    Spacer()
                }
                Text(f.title).fontWeight(.semibold).font(.system(size: 13))
                MarkdownView(text: f.body)
                if let s = f.suggestion {
                    SuggestionPreview(old: store.lines(for: f.anchor), new: s.components(separatedBy: "\n"))
                }
                // Full labels when there's room (unified); compact ones in a split-view half.
                ViewThatFits(in: .horizontal) {
                    actions(compact: false)
                    actions(compact: true)
                }
            }
            .padding(12)
            .background(Color.ghAIBg, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.ghAI.opacity(0.45), style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            .frame(maxWidth: 880, alignment: .leading)
        }
    }
}

extension FindingBlockView {
    func actions(compact: Bool) -> some View {
        HStack(spacing: 8) {
            Button(compact ? "Accept" : "Accept as comment") { store.acceptFinding(findingId) }
                .buttonStyle(GHButtonStyle(kind: .primary, compact: true)).fixedSize()
            Button { store.discussFinding(findingId) } label: { Label("Discuss", systemImage: "sparkle") }
                .buttonStyle(GHButtonStyle(compact: true)).fixedSize()
            Button("Dismiss") { store.dismissFinding(findingId) }
                .buttonStyle(GHButtonStyle(kind: .invisible, compact: true)).fixedSize()
            Spacer(minLength: 0)
            if !compact {
                Text("Private — only you can see this").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                    .lineLimit(1).fixedSize()
            }
        }
    }
}

struct SeverityPill: View {
    let severity: AIFinding.Severity
    var body: some View {
        switch severity {
        case .high: Pill(text: "High", color: .ghDel, filled: true)
        case .medium: Pill(text: "Medium", color: .ghAttention, filled: true)
        case .low: Pill(text: "Low", color: .ghAccent)
        case .nit: Pill(text: "Nit", color: .ghMuted)
        }
    }
}

/// Floating bar shown under a line selection.
struct SelectionBar: View {
    let store: PRStore
    let count: Int

    var body: some View {
        HStack(spacing: 6) {
            Button { store.startComment() } label: {
                Label("Comment", systemImage: "text.bubble")
            }
            .buttonStyle(GHButtonStyle(kind: .primary, compact: true))
            .help("Comment (C)")
            Button { store.startDiscuss() } label: {
                Label("Discuss", systemImage: "sparkle")
            }
            .buttonStyle(GHButtonStyle(kind: .ai, compact: true))
            .help("Ask Claude about this (D)")
            Text(count == 1 ? "1 line" : "\(count) lines")
                .font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                .padding(.trailing, 4)
        }
        .padding(5)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.ghBorder))
        .shadow(color: .black.opacity(0.15), radius: 8, y: 3)
        .fixedSize()
    }
}
