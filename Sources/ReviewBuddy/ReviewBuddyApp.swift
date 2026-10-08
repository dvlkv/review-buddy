import AppKit
import SwiftUI
import ReviewBuddyCore
import ReviewBuddyUI

@main
struct ReviewBuddyApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @State private var app = AppModel()

    var body: some Scene {
        WindowGroup("Review Buddy", id: "main") {
            RootView(app: app)
                .frame(minWidth: 1000, minHeight: 640)
                .task { await app.bootstrap() }
        }
        .defaultSize(width: 1500, height: 940)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(after: .newItem) {
                Button("Open Pull Request…") { app.showOpenURL = true }
                    .keyboardShortcut("l", modifiers: .command)
            }
            CommandMenu("Review") {
                Button("Command Palette…") { app.showPalette = true }
                    .keyboardShortcut("k", modifiers: .command)
                Button("Refresh") { Task { await app.reloadCurrent() } }
                    .keyboardShortcut("r", modifiers: .command)
                Divider()
                Button("Toggle Inbox Sidebar") { app.showSidebar.toggle() }
                    .keyboardShortcut("s", modifiers: [.control, .command])
                Button("Toggle Discuss Panel") { app.showDiscussPanel.toggle() }
                    .keyboardShortcut("\\", modifiers: .command)
                Button("Toggle Split / Unified") { app.current?.toggleMode() }
                Button("Finish Review…") { app.showSubmitReview = true }
                    .keyboardShortcut(.return, modifiers: [.command, .shift])
                Divider()
                Button("Summarize Pull Request") { app.current?.startSummarize() }
                Button("Run AI Pre-review") { app.current?.startPreReview() }
                Button("Push Fix Commits…") { app.showPushSheet = true }
                    .disabled(app.current?.localCommits.isEmpty ?? true)
                Divider()
                Button("Copy Performance Report") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(Perf.shared.report(), forType: .string)
                }
            }
        }
        Settings {
            SettingsView(app: app)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Running from `swift run` (no bundle): still become a regular foreground app.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
