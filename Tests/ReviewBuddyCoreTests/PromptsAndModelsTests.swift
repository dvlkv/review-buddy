import Foundation
import Testing
@testable import ReviewBuddyCore

@Suite struct PromptsTests {
    let pr = PRDetail(ref: PRRef(owner: "o", repo: "r", number: 42), nodeId: "N", title: "Add cache",
                      body: "Speeds up lookups.", author: "alice", baseRefName: "main", headRefName: "cache",
                      baseOid: "0123456789abcdef", headOid: "fedcba9876543210")

    func thread(_ path: String, _ line: Int, _ body: String, resolved: Bool = false) -> ReviewThread {
        ReviewThread(id: UUID().uuidString, path: path, line: line, isResolved: resolved,
                     comments: [ReviewComment(id: "c", body: body, author: "bob", createdAt: Date())])
    }

    @Test func discussContextContainsEverything() {
        let anchor = CodeAnchor(path: "src/cache.swift", side: .right, startLine: 10, endLine: 12)
        let s = Prompts.discussContext(pr: pr, anchor: anchor, selectedLines: ["let a = 1", "let b = 2"],
                                       surrounding: "// around", threadsOnFile: [thread("src/cache.swift", 11, "why\nthis?", resolved: true)],
                                       otherThreads: [thread("other.swift", 3, "nit")], question: "Is this thread-safe?")
        #expect(s.contains("o/r#42") && s.contains("Add cache"))
        #expect(s.contains("Speeds up lookups."))
        #expect(s.contains("src/cache.swift lines 10-12 of the new (head) version"))
        #expect(s.contains("let a = 1\nlet b = 2"))
        #expect(s.contains("// around"))
        #expect(s.contains("L11 [resolved]: @bob: why this?"))
        #expect(s.contains("other.swift:3 @bob: nit"))
        #expect(s.hasSuffix("Is this thread-safe?"))
        #expect(s.contains("0123456789") && !s.contains("0123456789a"))
        let left = Prompts.discussContext(pr: pr, anchor: CodeAnchor(path: "x", side: .left, startLine: 1, endLine: 1),
                                          selectedLines: [], surrounding: nil, threadsOnFile: [], otherThreads: [], question: "q")
        #expect(left.contains("old (base) version") && !left.contains("Surrounding") && !left.contains("Existing review threads"))
    }

    @Test func otherPrompts() {
        #expect(Prompts.systemPrompt(pr: pr).contains("refs/review-buddy/base-42"))
        #expect(Prompts.systemPrompt(pr: pr).contains("Do not push"))
        #expect(Prompts.fixInstruction.contains("commit") && Prompts.fixInstruction.contains("Do not push"))
        let d = Prompts.draftComment(anchor: CodeAnchor(path: "a.swift", side: .right, startLine: 3, endLine: 5))
        #expect(d.contains("a.swift lines 3-5") && d.contains("```suggestion"))
        #expect(Prompts.summary(pr: pr).contains("Speeds up lookups."))
        // Summaries and pre-reviews read the PR as published, never local fix commits.
        #expect(Prompts.preReview(pr: pr).contains("base-42...refs/review-buddy/head-42"))
        #expect(Prompts.summary(pr: pr).contains("base-42...refs/review-buddy/head-42"))
        #expect(!Prompts.preReview(pr: pr).contains("...HEAD"))
        #expect(Prompts.systemPrompt(pr: pr).contains("refs/review-buddy/head-42"))
    }

    @Test func schemasAreValidJSON() throws {
        for schema in [Prompts.summarySchema, Prompts.preReviewSchema] {
            let obj = try JSONSerialization.jsonObject(with: Data(schema.utf8)) as? [String: Any]
            #expect(obj?["type"] as? String == "object")
        }
    }

    @Test func summarySchemaDecodesIntoModel() throws {
        let json = #"{"summary":"S","readingOrder":[{"path":"a","reason":"r"}],"risks":[{"description":"d"}]}"#
        let s = try JSONDecoder().decode(AISummary.self, from: Data(json.utf8))
        #expect(s.summary == "S" && s.readingOrder[0].path == "a" && s.risks[0].path == nil)
    }
}

@Suite struct AIModelTests {
    @Test func findingFromRawModelOutput() throws {
        let json = #"""
        {"findings":[
          {"path":"a.swift","side":"LEFT","startLine":9,"endLine":4,"severity":"high","title":"Bug","body":"b","suggestion":""},
          {"path":"b.swift","side":"weird","startLine":2,"severity":"blocker","title":"T"}
        ]}
        """#
        let r = try JSONDecoder().decode(PreReviewResult.self, from: Data(json.utf8))
        let f = r.findings[0]
        #expect(f.startLine == 4 && f.endLine == 9 && f.side == .left && f.severity == .high)
        #expect(f.suggestion == nil && f.state == .open && !f.id.isEmpty)
        let g = r.findings[1]
        #expect(g.side == .right && g.endLine == 2 && g.severity == .low && g.body == "")
        #expect(f.id != g.id)
    }

    @Test func findingRoundTripKeepsIdAndState() throws {
        var f = AIFinding(path: "p", side: .right, startLine: 1, endLine: 2, severity: .nit, title: "t", body: "b", suggestion: "x")
        f.state = .dismissed
        let back = try JSONDecoder().decode(AIFinding.self, from: JSONEncoder().encode(f))
        #expect(back == f)
    }

    @Test func commentBody() {
        let f = AIFinding(path: "p", side: .right, startLine: 3, endLine: 4, severity: .medium, title: "Leak", body: "Closes nothing.", suggestion: "close()")
        #expect(f.commentBody == "**Leak**\n\nCloses nothing.\n\n```suggestion\nclose()\n```")
        #expect(f.anchor == CodeAnchor(path: "p", side: .right, startLine: 3, endLine: 4))
        let g = AIFinding(path: "p", side: .right, startLine: 3, endLine: 3, severity: .low, title: "T", body: "B")
        #expect(g.commentBody == "**T**\n\nB")
    }

    @Test func severityOrdering() {
        #expect(AIFinding.Severity.allCases.sorted() == [.high, .medium, .low, .nit])
        #expect(AIFinding.Severity.high < .nit)
    }

    @Test func discussThreadHashAndTitle() {
        let h1 = DiscussThread.hashLines(["a", "b"])
        #expect(h1 == DiscussThread.hashLines(["a", "b"]))
        #expect(h1 != DiscussThread.hashLines(["a", "c"]))
        #expect(h1 != DiscussThread.hashLines(["ab"]))
        var t = DiscussThread(ref: PRRef(owner: "o", repo: "r", number: 1),
                              anchor: CodeAnchor(path: "x/y.swift", side: .right, startLine: 1, endLine: 2),
                              commitSHA: "s", anchorTextHash: h1)
        #expect(t.title == "y.swift L1-2")
        t.messages = [DiscussMessage(role: .assistant, text: "hi"), DiscussMessage(role: .user, text: "Why?\nmore")]
        #expect(t.title == "Why?")
    }
}

@Suite struct LocalStoreTests {
    func tempRoot() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("rb-store-\(UUID().uuidString)")
    }

    @Test func settingsRoundTrip() async throws {
        let root = tempRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = LocalStore(root: root)
        #expect(await store.loadSettings() == AppSettings())
        var s = AppSettings()
        s.pinnedRepos = ["o/r"]
        s.repoClones = ["o/r": "/code/r"]
        s.viewMode = .split
        s.claudeModel = "opus"
        try await store.saveSettings(s)
        #expect(await LocalStore(root: root).loadSettings() == s)
        #expect(s.localClone(for: "o/r")?.path == "/code/r")
        #expect(s.localClone(for: "x/y") == nil)
    }

    @Test func tolerantSettingsDecoding() async throws {
        let root = tempRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = LocalStore(root: root)
        try Data(#"{"pinnedRepos":["a/b"],"viewMode":"bogus","fontSize":"big","futureKey":1}"#.utf8)
            .write(to: root.appendingPathComponent("settings.json"))
        let s = await store.loadSettings()
        #expect(s.pinnedRepos == ["a/b"])
        #expect(s.viewMode == .unified && s.fontSize == 12 && s.largeFileThreshold == 2000)
        try Data("garbage".utf8).write(to: root.appendingPathComponent("settings.json"))
        #expect(await store.loadSettings() == AppSettings())
    }

    @Test func prStateDebounceFlushAndUpdate() async throws {
        let root = tempRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let ref = PRRef(owner: "o", repo: "r", number: 7)
        let file = root.appendingPathComponent("prs/o__r__7.json")
        let store = LocalStore(root: root)
        #expect(await store.load(ref) == PRLocalState())

        var st = PRLocalState()
        st.lastSeenHead = "abc"
        await store.save(ref, st, debounce: .seconds(30))
        #expect(await store.load(ref).lastSeenHead == "abc", "cache serves unsaved state")
        #expect(!FileManager.default.fileExists(atPath: file.path), "write is debounced")
        await store.flush()
        #expect(FileManager.default.fileExists(atPath: file.path))
        #expect(await LocalStore(root: root).load(ref).lastSeenHead == "abc")

        await store.update(ref) { $0.summary = AISummary(summary: "S", readingOrder: [], risks: []) }
        #expect(await store.load(ref).summary?.summary == "S")
        #expect(await store.load(ref).lastSeenHead == "abc")
        // Default debounce eventually writes.
        try await Task.sleep(for: .milliseconds(900))
        #expect(await LocalStore(root: root).load(ref).summary?.summary == "S")
    }

    @Test func coalescesBursts() async throws {
        let root = tempRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let ref = PRRef(owner: "o", repo: "r", number: 8)
        let store = LocalStore(root: root)
        for i in 0..<50 {
            var st = PRLocalState()
            st.lastSeenHead = "h\(i)"
            await store.save(ref, st, debounce: .milliseconds(100))
        }
        try await Task.sleep(for: .milliseconds(500))
        #expect(await LocalStore(root: root).load(ref).lastSeenHead == "h49")
    }
}
