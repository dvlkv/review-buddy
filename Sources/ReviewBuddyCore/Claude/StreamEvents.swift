import Foundation

/// A decoded line of `claude --output-format stream-json`.
public enum ClaudeEvent: Sendable, Equatable {
    case initialized(sessionId: String, model: String?)
    /// Streaming text chunk (`--include-partial-messages`).
    case textDelta(messageId: String?, text: String)
    /// A complete assistant message.
    case assistant(messageId: String, blocks: [ContentBlock])
    /// Tool results reported back to the model.
    case toolResults([ToolResult])
    case result(ClaudeResult)
    case other(type: String)

    public enum ContentBlock: Sendable, Equatable {
        case text(String)
        case thinking(String)
        case toolUse(id: String, name: String, input: String)
    }

    public struct ToolResult: Sendable, Equatable {
        public var toolUseId: String
        public var content: String
        public var isError: Bool
    }
}

public struct ClaudeResult: Sendable, Equatable {
    public var sessionId: String?
    public var isError: Bool
    public var subtype: String
    public var text: String?
    public var costUSD: Double?
    /// Raw JSON of `structured_output` when `--json-schema` was used.
    public var structuredOutput: Data?

    public init(sessionId: String?, isError: Bool, subtype: String, text: String?, costUSD: Double?, structuredOutput: Data?) {
        self.sessionId = sessionId; self.isError = isError; self.subtype = subtype; self.text = text
        self.costUSD = costUSD; self.structuredOutput = structuredOutput
    }
}

public enum StreamJSON {
    /// Decodes a single NDJSON line. Unknown shapes become `.other` — the CLI adds
    /// event types over time and we must never crash on them.
    public static func decode(line: Data) -> ClaudeEvent? {
        guard !line.isEmpty,
              let obj = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { return nil }
        return decode(object: obj)
    }

    public static func decode(object obj: [String: Any]) -> ClaudeEvent {
        let type = obj["type"] as? String ?? ""
        switch type {
        case "system":
            if obj["subtype"] as? String == "init", let sid = obj["session_id"] as? String {
                return .initialized(sessionId: sid, model: obj["model"] as? String)
            }
            return .other(type: "system/\(obj["subtype"] as? String ?? "")")
        case "stream_event":
            guard let ev = obj["event"] as? [String: Any] else { return .other(type: type) }
            if ev["type"] as? String == "content_block_delta",
               let delta = ev["delta"] as? [String: Any], delta["type"] as? String == "text_delta",
               let text = delta["text"] as? String {
                return .textDelta(messageId: nil, text: text)
            }
            return .other(type: "stream_event/\(ev["type"] as? String ?? "")")
        case "assistant":
            guard let msg = obj["message"] as? [String: Any] else { return .other(type: type) }
            let id = msg["id"] as? String ?? UUID().uuidString
            let blocks: [ClaudeEvent.ContentBlock] = (msg["content"] as? [[String: Any]] ?? []).compactMap { b in
                switch b["type"] as? String {
                case "text": return .text(b["text"] as? String ?? "")
                case "thinking": return .thinking(b["thinking"] as? String ?? "")
                case "tool_use":
                    let input = (b["input"]).flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.sortedKeys]) }
                    return .toolUse(id: b["id"] as? String ?? "", name: b["name"] as? String ?? "tool",
                                    input: input.map { String(decoding: $0, as: UTF8.self) } ?? "{}")
                default: return nil
                }
            }
            return .assistant(messageId: id, blocks: blocks)
        case "user":
            guard let msg = obj["message"] as? [String: Any],
                  let content = msg["content"] as? [[String: Any]] else { return .other(type: type) }
            let results: [ClaudeEvent.ToolResult] = content.compactMap { b in
                guard b["type"] as? String == "tool_result" else { return nil }
                return .init(toolUseId: b["tool_use_id"] as? String ?? "",
                             content: flattenContent(b["content"]), isError: b["is_error"] as? Bool ?? false)
            }
            return results.isEmpty ? .other(type: type) : .toolResults(results)
        case "result":
            let structured = obj["structured_output"].flatMap { v -> Data? in
                guard !(v is NSNull) else { return nil }
                return try? JSONSerialization.data(withJSONObject: v, options: [.fragmentsAllowed])
            }
            return .result(ClaudeResult(sessionId: obj["session_id"] as? String,
                                        isError: obj["is_error"] as? Bool ?? false,
                                        subtype: obj["subtype"] as? String ?? "",
                                        text: obj["result"] as? String,
                                        costUSD: obj["total_cost_usd"] as? Double,
                                        structuredOutput: structured))
        default:
            return .other(type: type)
        }
    }

    static func flattenContent(_ c: Any?) -> String {
        if let s = c as? String { return s }
        if let arr = c as? [[String: Any]] {
            return arr.compactMap { $0["text"] as? String }.joined(separator: "\n")
        }
        return ""
    }

    /// The NDJSON line for a user turn.
    public static func userMessage(_ text: String) -> Data {
        let obj: [String: Any] = ["type": "user", "message": ["role": "user", "content": text]]
        var d = (try? JSONSerialization.data(withJSONObject: obj)) ?? Data()
        d.append(0x0A)
        return d
    }

    public static func interruptRequest(id: String = UUID().uuidString) -> Data {
        let obj: [String: Any] = ["type": "control_request", "request_id": id, "request": ["subtype": "interrupt"]]
        var d = (try? JSONSerialization.data(withJSONObject: obj)) ?? Data()
        d.append(0x0A)
        return d
    }
}

/// Splits a byte stream into lines without losing partial lines between reads.
public struct LineSplitter: Sendable {
    private var pending = Data()
    public init() {}
    public mutating func feed(_ chunk: Data) -> [Data] {
        pending.append(chunk)
        var lines: [Data] = []
        while let nl = pending.firstIndex(of: 0x0A) {
            let line = pending[pending.startIndex..<nl]
            if !line.isEmpty { lines.append(Data(line)) }
            pending.removeSubrange(pending.startIndex...nl)
        }
        return lines
    }
    public mutating func flush() -> Data? {
        defer { pending = Data() }
        return pending.isEmpty ? nil : pending
    }
}
