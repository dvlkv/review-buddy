import Foundation
import os

/// Signposts + simple in-process timers. View with Instruments ("Points of Interest")
/// or read `Perf.shared.report()` from the debug HUD.
public final class Perf: @unchecked Sendable {
    public static let shared = Perf()
    public let log = OSLog(subsystem: "dev.reviewbuddy", category: .pointsOfInterest)
    public let signposter = OSSignposter(subsystem: "dev.reviewbuddy", category: .pointsOfInterest)
    private let lock = NSLock()
    private var samples: [String: [Double]] = [:]
    private var counters: [String: Int] = [:]

    public func count(_ name: String, _ n: Int = 1) {
        lock.lock(); counters[name, default: 0] += n; lock.unlock()
    }

    public func counter(_ name: String) -> Int {
        lock.lock(); defer { lock.unlock() }
        return counters[name] ?? 0
    }

    public init() {}

    @discardableResult
    public func measure<T>(_ name: StaticString, _ body: () throws -> T) rethrows -> T {
        let state = signposter.beginInterval(name)
        let start = ContinuousClock.now
        defer {
            signposter.endInterval(name, state)
            record("\(name)", ms: (ContinuousClock.now - start).milliseconds)
        }
        return try body()
    }

    public func record(_ name: String, ms: Double) {
        lock.lock(); defer { lock.unlock() }
        samples[name, default: []].append(ms)
        if samples[name]!.count > 500 { samples[name]!.removeFirst(250) }
    }

    public func last(_ name: String) -> Double? {
        lock.lock(); defer { lock.unlock() }
        return samples[name]?.last
    }

    public func percentile(_ name: String, _ p: Double) -> Double? {
        lock.lock(); defer { lock.unlock() }
        guard let s = samples[name], !s.isEmpty else { return nil }
        let sorted = s.sorted()
        return sorted[min(sorted.count - 1, Int(Double(sorted.count) * p))]
    }

    public func report() -> String {
        lock.lock(); defer { lock.unlock() }
        let c = counters.keys.sorted().map { "\($0) count=\(counters[$0]!)" }
        return c.joined(separator: "\n") + (c.isEmpty ? "" : "\n") + samples.keys.sorted().map { k in
            let s = samples[k]!.sorted()
            let p50 = s[s.count / 2], p95 = s[min(s.count - 1, Int(Double(s.count) * 0.95))]
            return String(format: "%@ n=%d p50=%.2fms p95=%.2fms", k, s.count, p50, p95)
        }.joined(separator: "\n")
    }

    public func reset() {
        lock.lock(); samples.removeAll(); counters.removeAll(); lock.unlock()
    }
}

public extension Duration {
    var milliseconds: Double {
        let c = components
        return Double(c.seconds) * 1000 + Double(c.attoseconds) / 1e15
    }
}
