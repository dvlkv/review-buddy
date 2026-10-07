import Foundation

public struct ClaudeConfig: Sendable {
    public var executable: String
    public var cwd: URL
    public var resumeSessionId: String?
    public var forkSession: Bool
    public var appendSystemPrompt: String?
    public var model: String?
    public var permissionMode: String
    public var jsonSchema: String?
    public var extraArgs: [String]

    public init(executable: String, cwd: URL, resumeSessionId: String? = nil, forkSession: Bool = false,
                appendSystemPrompt: String? = nil, model: String? = nil, permissionMode: String = "bypassPermissions",
                jsonSchema: String? = nil, extraArgs: [String] = []) {
        self.executable = executable; self.cwd = cwd; self.resumeSessionId = resumeSessionId
        self.forkSession = forkSession; self.appendSystemPrompt = appendSystemPrompt; self.model = model
        self.permissionMode = permissionMode; self.jsonSchema = jsonSchema; self.extraArgs = extraArgs
    }

    /// Arguments for a long-lived streaming session.
    public func streamingArgs() -> [String] {
        var a = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
                 "--include-partial-messages", "--permission-mode", permissionMode]
        a += commonArgs()
        return a
    }

    /// Arguments for a one-shot run that prints a single JSON result.
    public func oneShotArgs(prompt: String) -> [String] {
        var a = ["-p", "--output-format", "json", "--permission-mode", permissionMode]
        a += commonArgs()
        if let jsonSchema { a += ["--json-schema", jsonSchema] }
        a.append(prompt)
        return a
    }

    private func commonArgs() -> [String] {
        var a: [String] = []
        if let resumeSessionId { a += ["--resume", resumeSessionId] }
        if forkSession { a.append("--fork-session") }
        if let appendSystemPrompt, !appendSystemPrompt.isEmpty { a += ["--append-system-prompt", appendSystemPrompt] }
        if let model, !model.isEmpty { a += ["--model", model] }
        return a + extraArgs
    }
}

public enum ClaudeError: Error, LocalizedError, Equatable {
    case notInstalled
    case failed(String)
    case badOutput
    public var errorDescription: String? {
        switch self {
        case .notInstalled: "The `claude` CLI was not found. Install Claude Code and sign in with `claude`."
        case .failed(let s): s
        case .badOutput: "Claude returned output that could not be parsed."
        }
    }
}

public enum SessionOutput: Sendable {
    case event(ClaudeEvent)
    case exited(status: Int32, stderr: String)
}

/// A long-lived `claude` process speaking stream-json over stdin/stdout.
/// Multiple user turns are written to the same process; if it exits, the next
/// `send` restarts it with `--resume <session_id>`.
public final class ClaudeSession: @unchecked Sendable {
    public private(set) var config: ClaudeConfig
    private let lock = NSLock()
    private var process: Process?
    private var stdin: FileHandle?
    private var continuation: AsyncStream<SessionOutput>.Continuation?
    public let output: AsyncStream<SessionOutput>
    private var sessionId: String?

    public init(config: ClaudeConfig) {
        self.config = config
        self.sessionId = config.resumeSessionId
        var cont: AsyncStream<SessionOutput>.Continuation!
        self.output = AsyncStream(bufferingPolicy: .unbounded) { cont = $0 }
        self.continuation = cont
    }

    public var isProcessRunning: Bool { lock.withLock { process?.isRunning ?? false } }
    public var currentSessionId: String? { lock.withLock { sessionId } }

    public func send(_ text: String) throws {
        try ensureProcess()
        let data = StreamJSON.userMessage(text)
        try lock.withLock { try stdin?.write(contentsOf: data) }
    }

    /// Asks Claude to stop the current turn; falls back to SIGINT.
    public func interrupt() {
        lock.withLock {
            guard let p = process, p.isRunning else { return }
            try? stdin?.write(contentsOf: StreamJSON.interruptRequest())
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 2) { [weak self] in
            guard let self else { return }
            self.lock.withLock { if let p = self.process, p.isRunning, self._turnActive { p.interrupt() } }
        }
    }

    private var _turnActive = false
    /// Set by the owner from the transcript; used to decide whether SIGINT is needed.
    public var turnActive: Bool {
        get { lock.withLock { _turnActive } }
        set { lock.withLock { _turnActive = newValue } }
    }

    public func terminate() {
        lock.withLock {
            try? stdin?.close()
            if let p = process, p.isRunning { p.terminate() }
            process = nil
            stdin = nil
        }
    }

    public func finish() {
        terminate()
        continuation?.finish()
    }

    private func ensureProcess() throws {
        try lock.withLock {
            if let p = process, p.isRunning { return }
            var cfg = config
            cfg.resumeSessionId = sessionId ?? config.resumeSessionId
            let p = Process()
            p.executableURL = URL(fileURLWithPath: cfg.executable)
            p.arguments = cfg.streamingArgs()
            p.currentDirectoryURL = cfg.cwd
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = ToolLocator.searchPath
            p.environment = env
            let inPipe = Pipe(), outPipe = Pipe(), errPipe = Pipe()
            p.standardInput = inPipe
            p.standardOutput = outPipe
            p.standardError = errPipe

            let splitter = SplitterBox()
            let errBox = DataBox()
            let cont = continuation
            // Report exit only after stdout and stderr hit EOF *and* the process
            // terminated, so no trailing events are lost to handler races.
            let gate = ExitGate { status in
                if let tail = splitter.flush(), let ev = StreamJSON.decode(line: tail) { cont?.yield(.event(ev)) }
                cont?.yield(.exited(status: status, stderr: String(decoding: errBox.data, as: UTF8.self)))
            }
            outPipe.fileHandleForReading.readabilityHandler = { [weak self] h in
                let chunk = h.availableData
                guard !chunk.isEmpty else {
                    h.readabilityHandler = nil
                    gate.outputClosed()
                    return
                }
                for line in splitter.feed(chunk) {
                    guard let ev = StreamJSON.decode(line: line) else { continue }
                    if case .initialized(let sid, _) = ev { self?.lock.withLock { self?.sessionId = sid } }
                    if case .result(let r) = ev, let sid = r.sessionId { self?.lock.withLock { self?.sessionId = sid } }
                    cont?.yield(.event(ev))
                }
            }
            errPipe.fileHandleForReading.readabilityHandler = { h in
                let chunk = h.availableData
                if chunk.isEmpty { h.readabilityHandler = nil; gate.outputClosed() } else { errBox.append(chunk) }
            }
            p.terminationHandler = { proc in gate.terminated(proc.terminationStatus) }
            try p.run()
            process = p
            stdin = inPipe.fileHandleForWriting
        }
    }
}

/// One-shot `claude -p --output-format json` runs (summary, pre-review, comment drafts).
public protocol ClaudeOneShotRunning: Sendable {
    func run(config: ClaudeConfig, prompt: String) async throws -> ClaudeResult
}

public struct ClaudeOneShot: ClaudeOneShotRunning {
    let runner: CommandRunning
    public init(runner: CommandRunning = ProcessRunner()) { self.runner = runner }

    public func run(config: ClaudeConfig, prompt: String) async throws -> ClaudeResult {
        let r = try await runner.run(config.executable, config.oneShotArgs(prompt: prompt), cwd: config.cwd, env: nil, stdin: nil)
        // The CLI prints the result object even on error exits.
        let trimmed = r.stdout.split(separator: 0x0A).last.map { Data($0) } ?? r.stdout
        guard let ev = StreamJSON.decode(line: trimmed), case .result(let res) = ev else {
            if r.status != 0 { throw ClaudeError.failed(r.stderrString.isEmpty ? "claude exited with \(r.status)" : r.stderrString) }
            throw ClaudeError.badOutput
        }
        if res.isError { throw ClaudeError.failed(res.text ?? "Claude reported an error (\(res.subtype)).") }
        return res
    }
}

final class SplitterBox: @unchecked Sendable {
    private let lock = NSLock()
    private var splitter = LineSplitter()
    func feed(_ d: Data) -> [Data] { lock.withLock { splitter.feed(d) } }
    func flush() -> Data? { lock.withLock { splitter.flush() } }
}
