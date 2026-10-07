import Foundation

/// One entry in a Discuss conversation as shown in the side panel.
public struct DiscussMessage: Codable, Identifiable, Hashable, Sendable {
    public enum Role: String, Codable, Sendable { case user, assistant, tool, error, system }
    public var id: String
    public var role: Role
    public var text: String
    /// For tool calls: tool name, compact input, and result.
    public var toolName: String?
    public var toolInput: String?
    public var toolResult: String?
    public var toolFailed: Bool
    public var createdAt: Date
    /// True while text is still streaming.
    public var isStreaming: Bool

    public init(id: String = UUID().uuidString, role: Role, text: String, toolName: String? = nil,
                toolInput: String? = nil, toolResult: String? = nil, toolFailed: Bool = false,
                createdAt: Date = Date(), isStreaming: Bool = false) {
        self.id = id; self.role = role; self.text = text; self.toolName = toolName; self.toolInput = toolInput
        self.toolResult = toolResult; self.toolFailed = toolFailed; self.createdAt = createdAt; self.isStreaming = isStreaming
    }

    /// "Read src/a.ts", "Bash: npm test" — the folded tool row label.
    public var toolSummary: String {
        guard let name = toolName else { return text }
        guard let input = toolInput, let data = input.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return name }
        func short(_ p: String) -> String {
            let parts = p.split(separator: "/")
            return parts.count > 3 ? "…/" + parts.suffix(3).joined(separator: "/") : p
        }
        switch name {
        case "Read", "Edit", "Write", "MultiEdit", "NotebookEdit":
            return "\(name) \(short(obj["file_path"] as? String ?? obj["notebook_path"] as? String ?? ""))"
        case "Bash":
            let cmd = (obj["command"] as? String ?? "").split(separator: "\n").first.map(String.init) ?? ""
            return "Bash: \(cmd.prefix(80))"
        case "Grep": return "Grep \"\(obj["pattern"] as? String ?? "")\""
        case "Glob": return "Glob \(obj["pattern"] as? String ?? "")"
        case "WebFetch": return "Fetch \(obj["url"] as? String ?? "")"
        case "Task", "Agent": return "Agent: \(obj["description"] as? String ?? "")"
        default: return name
        }
    }
}

/// Folds `ClaudeEvent`s into a list of `DiscussMessage`s. Pure, so it is unit-tested
/// against recorded CLI output.
public struct TranscriptReducer: Sendable {
    public private(set) var messages: [DiscussMessage]
    public private(set) var sessionId: String?
    public private(set) var isRunning = false
    public private(set) var lastResult: ClaudeResult?
    /// Index of the assistant message receiving streamed text.
    private var streamingIndex: Int?
    private var toolIndexById: [String: Int] = [:]

    public init(messages: [DiscussMessage] = [], sessionId: String? = nil) {
        self.messages = messages
        self.sessionId = sessionId
        for (i, m) in messages.enumerated() where m.role == .tool { toolIndexById[m.id] = i }
    }

    public mutating func userSent(_ text: String, display: String? = nil) {
        messages.append(DiscussMessage(role: .user, text: display ?? text))
        isRunning = true
        streamingIndex = nil
    }

    public mutating func apply(_ event: ClaudeEvent) {
        switch event {
        case .initialized(let sid, _):
            sessionId = sid
        case .textDelta(_, let text):
            if let i = streamingIndex {
                messages[i].text += text
            } else {
                messages.append(DiscussMessage(role: .assistant, text: text, isStreaming: true))
                streamingIndex = messages.count - 1
            }
        case .assistant(_, let blocks):
            for b in blocks {
                switch b {
                case .text(let t):
                    if let i = streamingIndex {
                        // Replace streamed text with the authoritative full text.
                        messages[i].text = t
                        messages[i].isStreaming = false
                        streamingIndex = nil
                    } else if !t.isEmpty {
                        messages.append(DiscussMessage(role: .assistant, text: t))
                    }
                case .thinking:
                    break
                case .toolUse(let id, let name, let input):
                    finishStreaming()
                    messages.append(DiscussMessage(id: id, role: .tool, text: name, toolName: name, toolInput: input))
                    toolIndexById[id] = messages.count - 1
                }
            }
        case .toolResults(let results):
            for r in results {
                if let i = toolIndexById[r.toolUseId] {
                    messages[i].toolResult = String(r.content.prefix(20_000))
                    messages[i].toolFailed = r.isError
                }
            }
        case .result(let res):
            finishStreaming()
            isRunning = false
            lastResult = res
            if let sid = res.sessionId { sessionId = sid }
            if res.isError {
                messages.append(DiscussMessage(role: .error, text: res.text ?? "Claude stopped (\(res.subtype))."))
            }
        case .other:
            break
        }
    }

    public mutating func processExited(status: Int32, stderr: String) {
        finishStreaming()
        if isRunning {
            isRunning = false
            let msg = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            messages.append(DiscussMessage(role: .error, text: msg.isEmpty ? "Claude exited (\(status))." : msg))
        }
    }

    public mutating func interrupted() {
        finishStreaming()
        isRunning = false
        messages.append(DiscussMessage(role: .system, text: "Stopped."))
    }

    private mutating func finishStreaming() {
        if let i = streamingIndex { messages[i].isStreaming = false }
        streamingIndex = nil
    }
}
