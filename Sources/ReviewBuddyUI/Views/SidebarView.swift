import SwiftUI
import ReviewBuddyCore

struct SidebarView: View {
    let app: AppModel
    let inbox: InboxStore

    var body: some View {
        List(selection: Binding(get: { app.current?.ref.description }, set: { _ in })) {
            ForEach(inbox.sections) { section in
                Section {
                    if let e = section.error {
                        Label(e, systemImage: "exclamationmark.triangle").font(.system(size: 11)).foregroundStyle(Color.ghDel)
                            .lineLimit(3)
                    } else if section.items.isEmpty && !section.isLoading {
                        Text("Nothing here").font(.system(size: 11)).foregroundStyle(Color.ghMuted)
                    }
                    ForEach(section.items) { pr in
                        InboxRow(pr: pr, showRepo: !section.id.hasPrefix("repo:"))
                            .tag(pr.id)
                            .contentShape(Rectangle())
                            .onTapGesture { Task { await app.open(pr.ref) } }
                            .contextMenu {
                                Button("Open on GitHub") { NSWorkspace.shared.open(pr.ref.webURL) }
                                Button("Copy URL") {
                                    NSPasteboard.general.clearContents()
                                    NSPasteboard.general.setString(pr.ref.webURL.absoluteString, forType: .string)
                                }
                            }
                    }
                } header: {
                    HStack {
                        Text(section.title)
                        if section.isLoading { ProgressView().controlSize(.mini) }
                        Spacer()
                        if !section.items.isEmpty { Text("\(section.items.count)").foregroundStyle(.secondary) }
                    }
                    // Section headers run 14pt wider than the row cells; line the count up with them.
                    .padding(.trailing, 14)
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            HStack {
                Button { app.showOpenURL = true } label: { Label("Open PR…", systemImage: "link") }
                    .buttonStyle(.borderless)
                Spacer()
                if let last = inbox.lastRefresh {
                    Text(last.relative).font(.system(size: 10)).foregroundStyle(.secondary)
                }
                Button { Task { await inbox.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless).help("Refresh inbox")
            }
            .padding(10)
            // Opaque bar so the list doesn't show through as it scrolls underneath.
            .background(.bar)
            .overlay(alignment: .top) { Divider() }
        }
    }
}

struct InboxRow: View {
    let pr: PRSummary
    let showRepo: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .top, spacing: 6) {
                Image(systemName: pr.isDraft ? "circle.dashed" : "arrow.triangle.pull")
                    .foregroundStyle(pr.isDraft ? Color.ghMuted : Color.ghAdd)
                    .font(.system(size: 11))
                    .padding(.top, 2)
                Text(pr.title).font(.system(size: 12, weight: .medium)).lineLimit(2)
            }
            HStack(spacing: 6) {
                if showRepo { Text(pr.ref.nameWithOwner).lineLimit(1) }
                Text("#\(pr.ref.number)")
                Text("·")
                Text(pr.author)
                Spacer(minLength: 0)
                checks
                review
            }
            .font(.system(size: 10.5))
            .foregroundStyle(.secondary)
            HStack(spacing: 6) {
                Text("+\(pr.additions)").foregroundStyle(Color.ghAdd)
                Text("−\(pr.deletions)").foregroundStyle(Color.ghDel)
                if pr.commentCount > 0 { Label("\(pr.commentCount)", systemImage: "bubble.left").labelStyle(.titleAndIcon) }
                Spacer(minLength: 0)
                Text(pr.updatedAt.relative)
            }
            .font(.system(size: 10, design: .monospaced))
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 3)
    }

    @ViewBuilder var checks: some View {
        switch pr.checks {
        case .success: Image(systemName: "checkmark").foregroundStyle(Color.ghAdd).help("Checks passed")
        case .failure, .error: Image(systemName: "xmark").foregroundStyle(Color.ghDel).help("Checks failing")
        case .pending, .expected: Image(systemName: "circle.fill").foregroundStyle(Color.ghAttention).font(.system(size: 6)).help("Checks running")
        case .none: EmptyView()
        }
    }

    @ViewBuilder var review: some View {
        switch pr.reviewDecision {
        case .approved: Image(systemName: "checkmark.seal.fill").foregroundStyle(Color.ghAdd).help("Approved")
        case .changesRequested: Image(systemName: "exclamationmark.bubble.fill").foregroundStyle(Color.ghDel).help("Changes requested")
        default: EmptyView()
        }
    }
}
