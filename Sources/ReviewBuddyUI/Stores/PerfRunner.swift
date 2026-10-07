import AppKit
import QuartzCore
import Foundation
import ReviewBuddyCore

/// Scripted performance run against the real window (`--perf-run <out.json>`).
/// Every step forces a synchronous display so timings include layout + drawing.
@MainActor
public enum PerfRunner {
    public struct Report: Codable {
        public var rows: Int = 0
        public var files: Int = 0
        public var lines: Int = 0
        public var loadMs: Double = 0
        public var firstPaintMs: Double = 0
        public var scrollUnified: Stats = .init()
        public var scrollSplit: Stats = .init()
        public var flingUnified: Stats = .init()
        public var jumpToFileMs: Stats = .init()
        public var toggleSplitMs: Double = 0
        public var toggleUnifiedMs: Double = 0
        public var addComposerMs: Double = 0
        public var collapseFileMs: Double = 0
        public var memoryMB: Double = 0
        public var counters: String = ""
    }

    public struct Stats: Codable {
        public var n = 0
        public var p50 = 0.0
        public var p95 = 0.0
        public var max = 0.0
        public var mean = 0.0
        init() {}
        init(_ samples: [Double]) {
            let s = samples.sorted()
            n = s.count
            guard !s.isEmpty else { return }
            p50 = s[s.count / 2]
            p95 = s[Swift.min(s.count - 1, Int(Double(s.count) * 0.95))]
            max = s.last!
            mean = s.reduce(0, +) / Double(s.count)
        }
    }

    static let launch = ContinuousClock.now

    public static func run(app: AppModel, output: URL) async {
        var report = Report()
        // Wait for the PR and the table.
        while app.current?.layout == nil || (app.current?.view as? DiffTableCoordinator)?.tableView?.window == nil {
            try? await Task.sleep(for: .milliseconds(20))
        }
        guard let store = app.current, let c = store.view as? DiffTableCoordinator,
              let tv = c.tableView, let sv = c.scrollView, let layout = store.layout else { return }
        if let w = tv.window { w.setFrame(NSRect(x: 40, y: 40, width: 1600, height: 1000), display: true) }
        try? await Task.sleep(for: .milliseconds(300)) // let the window settle
        sv.displayIfNeeded()
        report.firstPaintMs = (ContinuousClock.now - launch).milliseconds
        report.loadMs = Perf.shared.last("pr.load.total") ?? 0
        report.rows = layout.rowCount
        report.files = store.files.count
        report.lines = store.diff?.totalLines ?? 0
        Perf.shared.reset()

        func timed(_ label: String = "", _ body: () -> Void) -> Double {
            let tilesBefore = Perf.shared.counter("tile.render")
            defer { if !label.isEmpty { Perf.shared.record("\(label).tiles", ms: Double(Perf.shared.counter("tile.render") - tilesBefore)) } }
            let s = ContinuousClock.now
            body()
            if !label.isEmpty { Perf.shared.record("\(label).body", ms: (ContinuousClock.now - s).milliseconds) }
            let s2 = ContinuousClock.now
            defer { if !label.isEmpty { Perf.shared.record("\(label).flush", ms: (ContinuousClock.now - s2).milliseconds) } }
            // Layout, then commit the CA transaction so layer-backed views draw now.
            var t = ContinuousClock.now
            func phase(_ name: String) {
                if !label.isEmpty { Perf.shared.record("\(label).\(name)", ms: (ContinuousClock.now - t).milliseconds) }
                t = ContinuousClock.now
            }
            sv.window?.layoutIfNeeded(); phase("layout")
            sv.window?.displayIfNeeded(); phase("display")
            CATransaction.flush(); phase("commit")
            return (ContinuousClock.now - s).milliseconds
        }

        /// Like real scrolling, each step is one display frame (~8 ms at 120 Hz) and the
        /// run loop gets to process idle work between frames.
        func scrollSamples(steps: Int, by dy: CGFloat) async -> [Double] {
            var out: [Double] = []
            for _ in 0..<steps {
                try? await Task.sleep(for: .milliseconds(8))
                out.append(timed {
                    let y = sv.contentView.bounds.minY + dy
                    sv.contentView.scroll(to: CGPoint(x: 0, y: min(max(0, tv.bounds.height - sv.contentView.bounds.height), y)))
                    sv.reflectScrolledClipView(sv.contentView)
                })
            }
            return out
        }

        let only: String? = CommandLine.arguments.firstIndex(of: "--perf-only").map { CommandLine.arguments[$0 + 1] }
        if only == "jump" {
            var js: [Double] = []
            for _ in 0..<400 {
                try? await Task.sleep(for: .milliseconds(16))
                js.append(timed { store.scrollToFile(Int.random(in: 0..<store.files.count)) })
            }
            report.jumpToFileMs = Stats(js)
            if let data = try? JSONEncoder.pretty.encode(report) { try? data.write(to: output) }
            NSApp.terminate(nil)
            return
        }
        if only == "composer" {
            var xs: [Double] = []
            for i in 0..<40 {
                let f = min(store.files.count - 1, 10 + i * 3)
                store.scrollToFile(f)
                try? await Task.sleep(for: .milliseconds(60))
                guard let line = store.files[f].lines.first(where: { $0.kind == .addition }) else { continue }
                store.select(file: f, side: .right, line: Int(line.newLine), extend: false)
                xs.append(timed { store.startComment() })
                try? await Task.sleep(for: .milliseconds(60))
                if let c = store.composers.keys.first { store.cancelComposer(c) }
            }
            report.addComposerMs = xs.sorted()[xs.count / 2]
            report.counters = Perf.shared.report()
            if let data = try? JSONEncoder.pretty.encode(report) { try? data.write(to: output) }
            NSApp.terminate(nil)
            return
        }
        if only == "scroll" {
            report.scrollUnified = Stats(await scrollSamples(steps: 1500, by: 60))
            report.counters = Perf.shared.report()
            if let data = try? JSONEncoder.pretty.encode(report) { try? data.write(to: output) }
            NSApp.terminate(nil)
            return
        }
        if only == "toggle" {
            var js: [Double] = []
            for i in 0..<30 { js.append(timed { store.setMode(i % 2 == 0 ? .split : .unified) }) }
            report.jumpToFileMs = Stats(js)
            if let data = try? JSONEncoder.pretty.encode(report) { try? data.write(to: output) }
            NSApp.terminate(nil)
            return
        }
        // Structural edits: add a comment composer (on a visible line), collapse a file.
        // Each is the median of 9, a few frames apart, after the previous step settles.
        var composers: [Double] = [], collapses: [Double] = []
        for i in 0..<9 {
            let f = min(store.files.count - 1, 10 + i * 7)
            store.scrollToFile(f)
            try? await Task.sleep(for: .milliseconds(80))
            if let line = store.files[f].lines.first(where: { $0.kind == .addition }) {
                store.select(file: f, side: .right, line: Int(line.newLine), extend: false)
                composers.append(timed("composer") { store.startComment() })
            }
            try? await Task.sleep(for: .milliseconds(80))
            for id in store.composers.keys { store.cancelComposer(id) }
            try? await Task.sleep(for: .milliseconds(80))
            collapses.append(timed("collapse") { store.toggleCollapsed(f) })
            try? await Task.sleep(for: .milliseconds(80))
            store.toggleCollapsed(f)
        }
        report.addComposerMs = composers.sorted()[composers.count / 2]
        report.collapseFileMs = collapses.sorted()[collapses.count / 2]
        store.scrollToFile(0)
        try? await Task.sleep(for: .milliseconds(200))

        // Smooth trackpad-like scroll: 60px per frame.
        report.scrollUnified = Stats(await scrollSamples(steps: 400, by: 60))
        // Fast fling: a full page per frame (every frame shows all-new rows).
        report.flingUnified = Stats(await scrollSamples(steps: 150, by: sv.contentView.bounds.height))

        // Random jumps between files.
        var jumps: [Double] = []
        var rng = SystemRandomNumberGenerator()
        for _ in 0..<40 {
            try? await Task.sleep(for: .milliseconds(16))
            let f = Int.random(in: 0..<max(1, store.files.count), using: &rng)
            jumps.append(timed { store.scrollToFile(f) })
        }
        report.jumpToFileMs = Stats(jumps)

        store.scrollToFile(min(store.files.count - 1, 5))
        // Median of several toggles, a frame apart (single samples are noisy).
        func median(_ xs: [Double]) -> Double { xs.sorted()[xs.count / 2] }
        var splits: [Double] = [], unifieds: [Double] = []
        for _ in 0..<5 {
            try? await Task.sleep(for: .milliseconds(50))
            splits.append(timed { store.setMode(.split) })
            try? await Task.sleep(for: .milliseconds(50))
            unifieds.append(timed { store.setMode(.unified) })
        }
        report.toggleSplitMs = median(splits)
        report.toggleUnifiedMs = median(unifieds)
        store.setMode(.split)
        report.scrollSplit = Stats(await scrollSamples(steps: 300, by: 60))
        store.setMode(.unified)



        report.memoryMB = residentMemoryMB()
        report.counters = Perf.shared.report()
        if let data = try? JSONEncoder.pretty.encode(report) {
            try? data.write(to: output)
        }
        if CommandLine.arguments.contains("--perf-quit") { NSApp.terminate(nil) }
    }

    static func residentMemoryMB() -> Double {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? Double(info.resident_size) / 1_048_576 : 0
    }
}

extension JSONEncoder {
    static var pretty: JSONEncoder {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        return e
    }
}
