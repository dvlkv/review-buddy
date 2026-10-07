import SwiftUI
import ReviewBuddyCore

public struct SettingsView: View {
    let app: AppModel

    public init(app: AppModel) { self.app = app }

    public var body: some View {
        if let services = app.services {
            TabView {
                AccountSettings(services: services).tabItem { Label("Accounts", systemImage: "person.crop.circle") }
                RepoSettings(app: app, services: services).tabItem { Label("Repositories", systemImage: "folder") }
                ReviewSettings(app: app, services: services).tabItem { Label("Review", systemImage: "text.bubble") }
            }
            .frame(width: 560, height: 420)
        } else {
            ProgressView().frame(width: 300, height: 200)
        }
    }
}

struct AccountSettings: View {
    let services: Services
    @State private var ghStatus = "Checking…"
    @State private var pat = ""
    @State private var model = ""

    var body: some View {
        Form {
            Section("GitHub") {
                LabeledContent("GitHub CLI", value: ghStatus)
                SecureField("Personal access token (fallback)", text: $pat)
                    .onSubmit { Keychain.write(account: "github-pat", value: pat) }
                Text("Review Buddy uses `gh auth token`. Add a token only if the GitHub CLI isn't installed. It's stored in your Keychain.")
                    .font(.caption).foregroundStyle(.secondary)
                Button("Save token") { Keychain.write(account: "github-pat", value: pat) }
            }
            Section("Claude") {
                LabeledContent("claude CLI", value: services.claudePath ?? "Not found — install Claude Code")
                TextField("Model (blank = CLI default)", text: $model)
                    .onSubmit { services.settings.claudeModel = model; services.saveSettings() }
                    .onChange(of: model) { services.settings.claudeModel = model; services.saveSettings() }
                Text("Discuss runs with full permissions inside a separate worktree for each PR. It never pushes on its own.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .task {
            model = services.settings.claudeModel
            pat = Keychain.read(account: "github-pat") ?? ""
            if let gh = ToolLocator.find("gh"),
               let r = try? await ProcessRunner().run(gh, ["auth", "status"], cwd: nil, env: nil, stdin: nil) {
                let out = (r.stdoutString + r.stderrString)
                ghStatus = out.contains("Logged in") ? (out.components(separatedBy: "\n").first { $0.contains("Logged in") }?
                    .trimmingCharacters(in: .whitespaces) ?? "Logged in") : "Not logged in — run `gh auth login`"
            } else {
                ghStatus = "Not installed"
            }
        }
    }
}

struct RepoSettings: View {
    let app: AppModel
    let services: Services
    @State private var newRepo = ""
    @State private var pinned: [String] = []
    @State private var clones: [String: String] = [:]

    var body: some View {
        Form {
            Section("Pinned repositories") {
                ForEach(pinned, id: \.self) { repo in
                    HStack {
                        Text(repo).font(.system(.body, design: .monospaced))
                        Spacer()
                        Button { pinned.removeAll { $0 == repo }; save() } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                    }
                }
                HStack {
                    TextField("owner/repo", text: $newRepo).onSubmit(add)
                    Button("Add", action: add).disabled(!newRepo.contains("/"))
                }
            }
            Section {
                ForEach(Array(Set(pinned + Array(clones.keys))).sorted(), id: \.self) { repo in
                    HStack {
                        Text(repo).font(.system(.body, design: .monospaced))
                        Spacer()
                        Text(clones[repo] ?? "Managed clone").foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                        Button("Choose…") { choose(repo) }
                        if clones[repo] != nil {
                            Button { clones[repo] = nil; save() } label: { Image(systemName: "xmark.circle") }.buttonStyle(.borderless)
                        }
                    }
                }
            } header: {
                Text("Local clones")
            } footer: {
                Text("Map a repository to your own clone to reuse its objects and build caches. Review Buddy always works in a separate worktree, so your checkout is never touched.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear {
            pinned = services.settings.pinnedRepos
            clones = services.settings.repoClones
        }
    }

    func add() {
        let r = newRepo.trimmingCharacters(in: .whitespaces)
        guard r.split(separator: "/").count == 2, !pinned.contains(r) else { return }
        pinned.append(r)
        newRepo = ""
        save()
    }

    func choose(_ repo: String) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose your local clone of \(repo)"
        if panel.runModal() == .OK, let url = panel.url {
            clones[repo] = url.path
            save()
        }
    }

    func save() {
        services.settings.pinnedRepos = pinned
        services.settings.repoClones = clones
        services.saveSettings()
        app.inbox?.pinnedReposChanged()
    }
}

struct ReviewSettings: View {
    let app: AppModel
    let services: Services
    @State private var s = AppSettings()

    var body: some View {
        Form {
            Section("AI") {
                Toggle("Summarize pull requests automatically when opened", isOn: $s.autoSummarize)
                Stepper(value: $s.autoPreReviewMaxLines, in: 0...5000, step: 100) {
                    Text(s.autoPreReviewMaxLines == 0 ? "Auto pre-review: off" : "Auto pre-review PRs up to \(s.autoPreReviewMaxLines) changed lines")
                }
            }
            Section("Diff") {
                Picker("Default view", selection: $s.viewMode) {
                    Text("Unified").tag(DiffViewMode.unified)
                    Text("Split").tag(DiffViewMode.split)
                }
                Stepper(value: $s.largeFileThreshold, in: 200...20000, step: 200) {
                    Text("Collapse files with more than \(s.largeFileThreshold) changed lines")
                }
                Stepper(value: $s.fontSize, in: 10...18, step: 1) { Text("Code font size: \(Int(s.fontSize)) pt") }
            }
            Section("Notifications") {
                Toggle("Notify me about new review requests", isOn: $s.notifyOnReviewRequests)
            }
        }
        .formStyle(.grouped)
        .onAppear { s = services.settings }
        .onChange(of: s) {
            services.settings = s
            services.saveSettings()
            DiffMetrics.shared.setFontSize(CGFloat(s.fontSize))
            app.current?.applySettings()
        }
    }
}
