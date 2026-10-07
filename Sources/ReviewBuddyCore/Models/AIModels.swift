import Foundation

/// A private AI conversation anchored to lines of a PR (persisted per PR).
public struct DiscussThread: Codable, Identifiable, Hashable, Sendable {
    public enum Status: String, Codable, Sendable { case active, outdated }
    public var id: String
    public var ref: PRRef
    public var anchor: CodeAnchor
    /// Head commit the anchor's line numbers refer to.
    public var commitSHA: String
    /// Hash of the anchored lines' text, used to detect edits on remap.
    public var anchorTextHash: UInt64
    public var sessionId: String?
    public var messages: [DiscussMessage]
    public var createdAt: Date
    public var status: Status
    /// Commits produced by "Fix it" from this thread.
    public var fixCommits: [String]

    public init(id: String = UUID().uuidString, ref: PRRef, anchor: CodeAnchor, commitSHA: String,
                anchorTextHash: UInt64, sessionId: String? = nil, messages: [DiscussMessage] = [],
                createdAt: Date = Date(), status: Status = .active, fixCommits: [String] = []) {
        self.id = id; self.ref = ref; self.anchor = anchor; self.commitSHA = commitSHA
        self.anchorTextHash = anchorTextHash; self.sessionId = sessionId; self.messages = messages
        self.createdAt = createdAt; self.status = status; self.fixCommits = fixCommits
    }

    public var title: String {
        messages.first(where: { $0.role == .user })?.text.split(separator: "\n").first.map(String.init) ?? anchor.label
    }

    public static func hashLines(_ lines: [String]) -> UInt64 {
        HashedWordSet.hash(Array(lines.joined(separator: "\n").utf16)[...])
    }
}

/// A queued request for the Discuss agent to change code. Kept as its own type so a
/// batched "fix queue" can be added without reworking Discuss.
public struct FixRequest: Codable, Identifiable, Hashable, Sendable {
    public enum Status: String, Codable, Sendable { case queued, running, committed, failed }
    public var id: String
    public var threadId: String
    public var anchor: CodeAnchor
    public var instructions: String
    public var status: Status
    public var commits: [String]
    public var error: String?
    public var createdAt: Date

    public init(id: String = UUID().uuidString, threadId: String, anchor: CodeAnchor, instructions: String,
                status: Status = .queued, commits: [String] = [], error: String? = nil, createdAt: Date = Date()) {
        self.id = id; self.threadId = threadId; self.anchor = anchor; self.instructions = instructions
        self.status = status; self.commits = commits; self.error = error; self.createdAt = createdAt
    }
}

public struct AISummary: Codable, Hashable, Sendable {
    public struct ReadingItem: Codable, Hashable, Sendable {
        public var path: String
        public var reason: String
        public init(path: String, reason: String) { self.path = path; self.reason = reason }
    }
    public struct Risk: Codable, Hashable, Sendable {
        public var path: String?
        public var description: String
        public init(path: String?, description: String) { self.path = path; self.description = description }
    }
    public var summary: String
    public var readingOrder: [ReadingItem]
    public var risks: [Risk]
    public var headSHA: String?
    public var createdAt: Date?

    public init(summary: String, readingOrder: [ReadingItem], risks: [Risk], headSHA: String? = nil, createdAt: Date? = nil) {
        self.summary = summary; self.readingOrder = readingOrder; self.risks = risks
        self.headSHA = headSHA; self.createdAt = createdAt
    }
}

public struct AIFinding: Codable, Identifiable, Hashable, Sendable {
    public enum Severity: String, Codable, Sendable, CaseIterable, Comparable {
        case high, medium, low, nit
        public static func < (a: Self, b: Self) -> Bool { a.order < b.order }
        var order: Int { switch self { case .high: 0; case .medium: 1; case .low: 2; case .nit: 3 } }
    }
    public enum State: String, Codable, Sendable { case open, accepted, dismissed, discussed }
    public var id: String
    public var path: String
    public var side: DiffSide
    public var startLine: Int
    public var endLine: Int
    public var severity: Severity
    public var title: String
    public var body: String
    public var suggestion: String?
    public var state: State

    enum CodingKeys: String, CodingKey { case id, path, side, startLine, endLine, severity, title, body, suggestion, state }

    public init(id: String = UUID().uuidString, path: String, side: DiffSide, startLine: Int, endLine: Int,
                severity: Severity, title: String, body: String, suggestion: String? = nil, state: State = .open) {
        self.id = id; self.path = path; self.side = side; self.startLine = min(startLine, endLine)
        self.endLine = max(startLine, endLine); self.severity = severity; self.title = title; self.body = body
        self.suggestion = suggestion; self.state = state
    }

    /// Decodes both persisted findings and raw model output (no id/state).
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? UUID().uuidString
        path = try c.decode(String.self, forKey: .path)
        side = (try? c.decode(DiffSide.self, forKey: .side)) ?? .right
        let s = try c.decode(Int.self, forKey: .startLine)
        let e = try c.decodeIfPresent(Int.self, forKey: .endLine) ?? s
        startLine = min(s, e); endLine = max(s, e)
        severity = (try? c.decode(Severity.self, forKey: .severity)) ?? .low
        title = try c.decode(String.self, forKey: .title)
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        let sug = try c.decodeIfPresent(String.self, forKey: .suggestion)
        suggestion = (sug?.isEmpty ?? true) ? nil : sug
        state = try c.decodeIfPresent(State.self, forKey: .state) ?? .open
    }

    public var anchor: CodeAnchor { CodeAnchor(path: path, side: side, startLine: startLine, endLine: endLine) }

    /// The review comment body produced by "Accept".
    public var commentBody: String {
        var s = "**\(title)**\n\n\(body)"
        if let suggestion { s += "\n\n```suggestion\n\(suggestion)\n```" }
        return s
    }
}

public struct PreReviewResult: Codable, Hashable, Sendable {
    public var findings: [AIFinding]
    public var headSHA: String?
    public var createdAt: Date?
    public init(findings: [AIFinding], headSHA: String? = nil, createdAt: Date? = nil) {
        self.findings = findings; self.headSHA = headSHA; self.createdAt = createdAt
    }
}
