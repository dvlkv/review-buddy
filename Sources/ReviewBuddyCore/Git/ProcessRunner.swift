import Foundation

public struct ProcessResult: Sendable {
    public var status: Int32
    public var stdout: Data
    public var stderr: Data
    public var stdoutString: String { String(decoding: stdout, as: UTF8.self) }
    public var stderrString: String { String(decoding: stderr, as: UTF8.self) }
}

public struct ProcessError: Error, LocalizedError, Sendable {
    public var command: String
    public var status: Int32
    public var stderr: String
    public var errorDescription: String? {
        let msg = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        return "`\(command)` failed (\(status))" + (msg.isEmpty ? "" : ": \(msg)")
    }
}

/// Runs subprocesses off the main thread with cancellation support.
public protocol CommandRunning: Sendable {
    func run(_ executable: String, _ args: [String], cwd: URL?, env: [String: String]?, stdin: Data?) async throws -> ProcessResult
}

public extension CommandRunning {
    /// Runs and throws on non-zero exit.
    @discardableResult
    func check(_ executable: String, _ args: [String], cwd: URL? = nil, env: [String: String]? = nil, stdin: Data? = nil) async throws -> ProcessResult {
        let r = try await run(executable, args, cwd: cwd, env: env, stdin: stdin)
        guard r.status == 0 else {
            throw ProcessError(command: ([URL(fileURLWithPath: executable).lastPathComponent] + args).joined(separator: " "),
                               status: r.status, stderr: r.stderrString)
        }
        return r
    }
}

public struct ProcessRunner: CommandRunning {
    public init() {}

    public func run(_ executable: String, _ args: [String], cwd: URL? = nil, env: [String: String]? = nil, stdin: Data? = nil) async throws -> ProcessResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = args
        if let cwd { process.currentDirectoryURL = cwd }
        var environment = ProcessInfo.processInfo.environment
        environment["PATH"] = ToolLocator.searchPath
        environment["GIT_TERMINAL_PROMPT"] = "0"
        if let env { environment.merge(env) { _, new in new } }
        process.environment = environment

        let outPipe = Pipe(), errPipe = Pipe()
        process.standardOutput = outPipe
        process.standardError = errPipe
        let inPipe: Pipe? = stdin != nil ? Pipe() : nil
        process.standardInput = inPipe ?? FileHandle.nullDevice

        // Drain pipes concurrently so large outputs (e.g. a 50 MB diff) never deadlock.
        // Resume only once both pipes reached EOF and the process exited.
        let outBox = DataBox(), errBox = DataBox()

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<ProcessResult, Error>) in
                let gate = ExitGate { status in
                    cont.resume(returning: ProcessResult(status: status, stdout: outBox.data, stderr: errBox.data))
                }
                for (pipe, box) in [(outPipe, outBox), (errPipe, errBox)] {
                    pipe.fileHandleForReading.readabilityHandler = { h in
                        let chunk = h.availableData
                        if chunk.isEmpty { h.readabilityHandler = nil; gate.outputClosed() } else { box.append(chunk) }
                    }
                }
                process.terminationHandler = { p in gate.terminated(p.terminationStatus) }
                do {
                    try process.run()
                    if let inPipe, let stdin {
                        DispatchQueue.global().async {
                            try? inPipe.fileHandleForWriting.write(contentsOf: stdin)
                            try? inPipe.fileHandleForWriting.close()
                        }
                    }
                } catch {
                    process.terminationHandler = nil
                    outPipe.fileHandleForReading.readabilityHandler = nil
                    errPipe.fileHandleForReading.readabilityHandler = nil
                    gate.cancel()
                    cont.resume(throwing: error)
                }
            }
        } onCancel: {
            if process.isRunning { process.interrupt() }
        }
    }
}

final class DataBox: @unchecked Sendable {
    private let lock = NSLock()
    private var buffer = Data()
    func append(_ d: Data) { guard !d.isEmpty else { return }; lock.lock(); buffer.append(d); lock.unlock() }
    var data: Data { lock.lock(); defer { lock.unlock() }; return buffer }
}

/// Finds CLI tools when launched from Finder (where PATH is minimal).
public enum ToolLocator {
    static let home = FileManager.default.homeDirectoryForCurrentUser.path
    public static let searchDirs: [String] = [
        "/opt/homebrew/bin", "/usr/local/bin", "\(home)/.local/bin", "\(home)/.claude/local",
        "\(home)/.bun/bin", "\(home)/.npm-global/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
    ]
    public static var searchPath: String {
        let existing = ProcessInfo.processInfo.environment["PATH"]?.split(separator: ":").map(String.init) ?? []
        var seen = Set<String>()
        return (existing + searchDirs).filter { seen.insert($0).inserted }.joined(separator: ":")
    }

    private static let cache = LockedDictionary<String, String>()

    /// Absolute path to `tool`, or nil. Checks PATH dirs, then the user's login shell.
    public static func find(_ tool: String) -> String? {
        if let c = cache[tool] { return c }
        let fm = FileManager.default
        for dir in searchPath.split(separator: ":") {
            let p = "\(dir)/\(tool)"
            if fm.isExecutableFile(atPath: p) { cache[tool] = p; return p }
        }
        // nvm and friends: ask the login shell.
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/zsh")
        p.arguments = ["-lc", "command -v \(tool)"]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        do {
            try p.run()
            p.waitUntilExit()
            let s = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if p.terminationStatus == 0, s.hasPrefix("/") { cache[tool] = s; return s }
        } catch {}
        return nil
    }
}

public final class LockedDictionary<K: Hashable, V>: @unchecked Sendable {
    private let lock = NSLock()
    private var dict: [K: V] = [:]
    public init() {}
    public subscript(key: K) -> V? {
        get { lock.lock(); defer { lock.unlock() }; return dict[key] }
        set { lock.lock(); dict[key] = newValue; lock.unlock() }
    }
}

/// Fires once after two pipe EOFs and process termination have all been observed.
final class ExitGate: @unchecked Sendable {
    private let lock = NSLock()
    private var pendingPipes = 2
    private var status: Int32?
    private var fired = false
    private let onExit: @Sendable (Int32) -> Void

    init(onExit: @escaping @Sendable (Int32) -> Void) { self.onExit = onExit }

    func outputClosed() { lock.withLock { pendingPipes -= 1 }; check() }
    func terminated(_ s: Int32) { lock.withLock { status = s }; check() }
    func cancel() { lock.withLock { fired = true } }

    private func check() {
        let fire: Int32? = lock.withLock {
            guard !fired, pendingPipes <= 0, let s = status else { return nil }
            fired = true
            return s
        }
        if let fire { onExit(fire) }
    }
}
