import AppKit
import SwiftUI
import Foundation
import ReviewBuddyCore

/// Scripted UI driver for visual checks without screen-recording permission:
/// `--ui-script "wait:1;select:0:12:14;snap:select;comment;snap:composer;quit" --snap-dir <dir>`
/// Snapshots render the key window into PNGs via `cacheDisplay`.
@MainActor
public enum UIScript {
    public static func run(app: AppModel, script: String, dir: URL) async {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        // `nopr;…` runs inbox/sidebar-only commands without waiting for a PR to render.
        if script.hasPrefix("nopr;") {
            while app.inbox == nil { try? await Task.sleep(for: .milliseconds(50)) }
            for raw in script.split(separator: ";").dropFirst() {
                let parts = raw.split(separator: ":").map(String.init)
                switch parts.first {
                case "wait": try? await Task.sleep(for: .milliseconds(Int((Double(parts[safe: 1] ?? "1") ?? 1) * 1000)))
                case "inbox": dumpInbox(app: app, dir: dir)
                case "sidebar": app.showSidebar.toggle()
                case "tree": dumpTree(depth: Int(parts[safe: 1] ?? "7") ?? 7, dir: dir)
                case "quit": NSApp.terminate(nil)
                default: break
                }
                try? await Task.sleep(for: .milliseconds(350))
            }
            return
        }
        while app.current?.layout == nil || (app.current?.view as? DiffTableCoordinator)?.tableView?.window == nil {
            try? await Task.sleep(for: .milliseconds(50))
        }
        if let w = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 400 }) {
            if w.styleMask.contains(.fullScreen) { w.toggleFullScreen(nil) }
            w.setFrame(NSRect(x: 40, y: 40, width: 1600, height: 1000), display: true)
        }
        try? await Task.sleep(for: .milliseconds(500))
        for raw in script.split(separator: ";") {
            let parts = raw.split(separator: ":").map(String.init)
            guard let cmd = parts.first, let store = app.current else { continue }
            let arg = { (i: Int) -> Int in i < parts.count ? Int(parts[i]) ?? 0 : 0 }
            switch cmd {
            case "wait": try? await Task.sleep(for: .milliseconds(Int((Double(parts[safe: 1] ?? "1") ?? 1) * 1000)))
            case "select":
                // select:file:startLine:endLine (right side)
                store.select(file: arg(1), side: .right, line: arg(2), extend: false)
                store.select(file: arg(1), side: .right, line: arg(3) == 0 ? arg(2) : arg(3), extend: true)
                store.scrollTo(anchor: CodeAnchor(path: store.files[arg(1)].path, side: .right, startLine: arg(2), endLine: arg(2)))
                (store.view as? DiffTableCoordinator)?.redrawVisible()
                (store.view as? DiffTableCoordinator)?.updateSelectionBar()
            case "textsel":
                // textsel:file:startLine:startCol:endLine:endCol (right side) — a character selection.
                store.select(file: arg(1), side: .right, line: arg(2), extend: false, column: arg(3))
                store.select(file: arg(1), side: .right, line: arg(4), extend: true, column: arg(5))
                store.scrollTo(anchor: CodeAnchor(path: store.files[arg(1)].path, side: .right, startLine: arg(2), endLine: arg(2)))
                (store.view as? DiffTableCoordinator)?.redrawVisible()
                (store.view as? DiffTableCoordinator)?.updateSelectionBar()
            case "addfile":
                // Simulates a PR update that adds a file: reinstall the diff plus one new file.
                // `addfile:swap` drops the first file too, keeping the file count the same.
                let extra = "diff --git a/zz_new.go b/zz_new.go\nnew file mode 100644\n--- /dev/null\n+++ b/zz_new.go\n@@ -0,0 +1,2 @@\n+package main\n+// new\n"
                var text = SyntheticDiff.make(.small)
                if parts[safe: 1] == "swap", let r = text.range(of: "diff --git", range: text.index(after: text.startIndex)..<text.endIndex) {
                    text = String(text[r.lowerBound...])
                }
                store.install(diff: UnifiedDiffParser.parse(text + "\n" + extra))
                try? "files=\(store.files.count) last=\(store.files.last?.path ?? "-")".write(to: dir.appendingPathComponent("files.txt"), atomically: true, encoding: .utf8)
            case "order":
                let top = (store.view as? DiffTableCoordinator)?.topVisibleRow ?? -1
                let line = "first=\(store.files.prefix(3).map { ($0.path as NSString).lastPathComponent }) reviewFirst=\(store.reviewFirstCount) topRow=\(top) currentFile=\(store.currentFile)\n"
                let url = dir.appendingPathComponent("order.txt")
                let old = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
                try? (old + line).write(to: url, atomically: true, encoding: .utf8)
            case "admin": store.detail?.status?.viewerCanMergeAsAdmin = true
            case "outdated": store.remoteHeadOid = String(repeating: "c", count: 40)
            case "noconversation": store.detail?.conversation = nil
            case "mergestate":
                // Pretend GitHub reports another merge state, e.g. `mergestate:CLEAN`.
                store.detail?.status?.mergeStateStatus = parts[safe: 1] ?? "CLEAN"
            case "overview":
                store.showOverview = parts[safe: 1] != "off"
            case "selectfirst":
                // First added line of the first rendered file.
                if let (fi, line) = store.files.enumerated().lazy.compactMap({ (i, f) -> (Int, Int)? in
                    guard let l = f.lines.first(where: { $0.kind == .addition }) else { return nil }
                    return (i, Int(l.newLine))
                }).first {
                    store.select(file: fi, side: .right, line: line, extend: false)
                    store.scrollTo(anchor: CodeAnchor(path: store.files[fi].path, side: .right, startLine: line, endLine: line))
                    (store.view as? DiffTableCoordinator)?.redrawVisible()
                    (store.view as? DiffTableCoordinator)?.updateSelectionBar()
                }
            case "fakesummary":
                store.setSummary(AISummary(summary: String(repeating: "This PR reworks the processing pipeline and changes how scores are computed. ", count: Int(parts[safe: 1] ?? "4") ?? 4),
                                           readingOrder: [.init(path: store.files[store.files.count - 1].path, reason: "Core change"),
                                                          .init(path: store.files[store.files.count / 2].path, reason: "Call sites")],
                                           risks: [.init(path: store.files[0].path, description: "Edge case for `empty` input")]), state: .idle)
            case "tree": dumpTree(depth: Int(parts[safe: 1] ?? "7") ?? 7, dir: dir)
            case "inbox": dumpInbox(app: app, dir: dir)
            case "resize":
                if let w = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 400 }) {
                    w.setFrame(NSRect(x: 40, y: 40, width: CGFloat(arg(1)), height: CGFloat(parts.count > 2 ? arg(2) : 1000)), display: true)
                }
            case "sidebar": app.showSidebar = parts[safe: 1] != "off"
            case "settext":
                if let c = store.composers.values.first { store.updateComposer(c.id, text: parts.dropFirst().joined(separator: ":")) }
            case "cancelall":
                for id in store.composers.keys { store.cancelComposer(id) }
            case "selectline":
                // selectline:<file>:<n>th addition line of that file
                let f = arg(1)
                let adds = store.files[f].lines.filter { $0.kind == .addition }
                if !adds.isEmpty {
                    let l = Int(adds[min(adds.count - 1, arg(2))].newLine)
                    store.select(file: f, side: .right, line: l, extend: false)
                    store.scrollTo(anchor: CodeAnchor(path: store.files[f].path, side: .right, startLine: l, endLine: l))
                    (store.view as? DiffTableCoordinator)?.redrawVisible()
                }
            case "typecomment":
                // typecomment:<text> — fills the open new-thread composer and submits it.
                let text = parts.dropFirst().joined(separator: ":")
                if let c = store.composers.values.first(where: { $0.anchor != nil }) {
                    store.updateComposer(c.id, text: text)
                    await store.submitComposer(c.id)
                }
            case "replyfirst":
                if let t = store.threads.values.first(where: { !$0.isResolved }) {
                    store.startReply(threadId: t.id)
                    if let c = store.composerFor(reply: t.id) {
                        store.updateComposer(c.id, text: parts.dropFirst().joined(separator: ":"))
                        await store.submitComposer(c.id)
                    }
                }
            case "resolvefirst":
                if let t = store.threads.values.first(where: { !$0.isResolved }) {
                    await store.setResolved(threadId: t.id, true)
                }
            case "submitreview":
                // submitreview:<EVENT>:<body>
                store.reviewBody = parts.dropFirst(2).joined(separator: ":")
                _ = await store.submitReview(event: ReviewEvent(rawValue: parts[safe: 1] ?? "COMMENT") ?? .comment)
            case "discard": await store.discardPendingReview()
            case "ensurewt": _ = try? await store.ensureWorktree()
            case "localcommit":
                // Makes a fix commit in the PR worktree (stands in for "Fix it" without a Claude run).
                if let wt = store.worktreeState.worktree {
                    let r = ProcessRunner()
                    let file = wt.path.appendingPathComponent("calc.go")
                    if var src = try? String(contentsOf: file, encoding: .utf8) {
                        src += "\n// touched by Review Buddy live test \(Date().timeIntervalSince1970)\n"
                        try? src.write(to: file, atomically: true, encoding: .utf8)
                    }
                    _ = try? await r.check("/usr/bin/git", ["-C", wt.path.path, "-c", "user.name=Review Buddy", "-c", "user.email=noreply@example.com",
                                                            "commit", "-qam", "docs: document Div precondition"])
                    await store.refreshGitState()
                }
            case "push": _ = await store.push()
            case "expandabove": store.expandAbove(file: arg(1), hunk: arg(2))
            case "expandbelow": store.expandBelow(file: arg(1))
            case "summarize": await store.summarize()
            case "prereview": await store.preReview()
            case "convert":
                if let id = store.activeDiscussId { await store.controller(for: id).convertToComment() }
            case "fix":
                if let id = store.activeDiscussId { await store.controller(for: id).fixIt() }
            case "reopen":
                // Re-activates the most recent Discuss thread (persisted from an earlier run).
                store.activeDiscussId = store.sortedDiscussThreads.first?.id
            case "dump":
                // Writes a JSON summary of store state for verification.
                var out: [String: Any] = [
                    "composers": store.composers.values.map { $0.text },
                    "localCommits": store.localCommits.map { "\($0.shortSHA) \($0.subject)" },
                    "findings": store.openFindings.map { "\($0.severity.rawValue) \($0.path):\($0.startLine) \($0.title)" },
                    "summary": store.summary?.summary ?? "",
                    "readingOrder": store.summary?.readingOrder.map(\.path) ?? [],
                    "fixRequests": store.fixRequests.map { "\($0.status.rawValue) \($0.error ?? "")" },
                    "threads": store.threads.values.map { "\($0.path):\($0.line ?? -1) resolved=\($0.isResolved) " + $0.comments.map { "[\($0.isPending ? "pending" : "posted")] \($0.body)" }.joined(separator: " | ") },
                    "pendingReviewId": store.pendingReviewId ?? "",
                    "viewed": Array(store.viewed),
                    "pushState": "\(store.pushState)", "submitState": "\(store.submitState)", "banner": store.banner ?? "",
                    "summaryState": "\(store.summaryState)", "preReviewState": "\(store.preReviewState)",
                ]
                if let id = store.activeDiscussId { out["draftState"] = "\(store.controller(for: id).draftState)" }
                if let d = try? JSONSerialization.data(withJSONObject: out, options: [.prettyPrinted, .sortedKeys]) {
                    try? d.write(to: dir.appendingPathComponent("\(parts[safe: 1] ?? "state").json"))
                }
            case "comment": store.startComment()
            case "discuss": store.startDiscuss()
            case "split": store.setMode(.split)
            case "unified": store.setMode(.unified)
            case "file": store.scrollToFile(arg(1))
            case "viewed": store.toggleViewed(arg(1))
            case "collapse": store.toggleCollapsed(arg(1))
            case "panel": app.showDiscussPanel = parts[safe: 1] != "off"
            case "palette": app.showPalette = parts[safe: 1] != "off"
            case "submit": app.showSubmitReview = parts[safe: 1] != "off"
            case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
            case "light": NSApp.appearance = NSAppearance(named: .aqua)
            case "next": store.navigate(.nextThread)
            case "snap": snapshot(name: parts[safe: 1] ?? "snap", dir: dir)
            case "snappanel":
                // The inspector is Liquid Glass and can't be cached; render it offscreen.
                let r = ImageRenderer(content: DiscussPanel(store: store).frame(width: 400, height: 760)
                    .environment(\.colorScheme, NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? .dark : .light))
                r.scale = 2
                if let img = r.nsImage, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                   let png = rep.representation(using: .png, properties: [:]) {
                    try? png.write(to: dir.appendingPathComponent("\(parts[safe: 1] ?? "panel").png"))
                }
            case "ask":
                // ask:<text> — sends a question in the active Discuss thread.
                if let id = store.activeDiscussId {
                    let q = parts.dropFirst().joined(separator: ":")
                    Task { await store.controller(for: id).send(q) }
                }
            case "waitai":
                // Waits (up to N seconds) for the active Discuss turn to finish.
                let limit = Date().addingTimeInterval(Double(parts[safe: 1] ?? "120") ?? 120)
                while Date() < limit, let id = store.activeDiscussId, store.controller(for: id).isRunning {
                    try? await Task.sleep(for: .milliseconds(500))
                }
            case "quit": NSApp.terminate(nil)
            default: break
            }
            try? await Task.sleep(for: .milliseconds(350))
        }
    }

    /// Inbox sections as text (the sidebar is Liquid Glass and can't be snapped).
    static func dumpInbox(app: AppModel, dir: URL) {
        let lines = (app.inbox?.sections ?? []).map {
            "\($0.id) loading=\($0.isLoading) items=\($0.items.count) error=\($0.error ?? "-")"
        }
        try? lines.joined(separator: "\n").write(to: dir.appendingPathComponent("inbox.txt"), atomically: true, encoding: .utf8)
    }

    static func dumpTree(depth: Int, dir: URL) {
        func dump(_ v: NSView, _ d: Int) -> String {
            guard d < depth else { return "" }
            var out = String(repeating: "  ", count: d) + "\(type(of: v)) \(v.frame) hidden=\(v.isHidden) alpha=\(v.alphaValue)\n"
            for sub in v.subviews { out += dump(sub, d + 1) }
            return out
        }
        if let w = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 400 }), let cv = w.contentView {
            let items = (w.toolbar?.items ?? []).map { "toolbar: \($0.itemIdentifier.rawValue) \($0.label)" }.joined(separator: "\n")
            try? (items + "\n" + dump(cv, 0)).write(to: dir.appendingPathComponent("tree.txt"), atomically: true, encoding: .utf8)
        }
    }

    public static func snapshot(name: String, dir: URL) {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil && $0.frame.width > 400 }),
              let view = window.contentView else { return }
        print("snapshot \(name): window \(window.frame) content \(view.bounds)")
        view.displayIfNeeded()
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        if let data = rep.representation(using: .png, properties: [:]) {
            try? data.write(to: dir.appendingPathComponent("\(name).png"))
        }
    }
}
