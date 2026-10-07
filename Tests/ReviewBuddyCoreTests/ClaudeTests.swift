import Foundation
import Testing
@testable import ReviewBuddyCore

func fixtureURL(_ name: String) -> URL {
    Bundle.module.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures")!
}

func fixtureEvents(_ name: String) throws -> [ClaudeEvent] {
    let data = try Data(contentsOf: fixtureURL(name))
    var splitter = LineSplitter()
    return splitter.feed(data).compactMap { StreamJSON.decode(line: $0) }
}

/// Writes an executable shell script and returns its path.
func makeScript(_ body: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("rb-claude-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let url = dir.appendingPathComponent("claude")
    try ("#!/bin/sh\n" + body).write(to: url, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
    return url
}

@Suite struct StreamJSONTests {
    @Test func decodesRecordedToolSession() throws {
        let ev = try fixtureEvents("claude-stream")
        #expect(ev.count == 9)
        guard case .initialized(let sid, let model) = ev[0] else { Issue.record("no init"); return }
        #expect(sid == "77dde8c7-4613-4ed2-bfe5-4fa278d39098")
        #expect(model != nil)
        guard case .assistant(_, let blocks) = ev[1], case .toolUse(let id, let name, let input) = blocks.first else {
            Issue.record("no tool use"); return
        }
        #expect(name == "Read" && id.hasPrefix("toolu_") && input.contains("file_path"))
        #expect(ev[2] == .other(type: "rate_limit_event"))
        guard case .toolResults(let res) = ev[3] else { Issue.record("no tool result"); return }
        #expect(res[0].toolUseId == id && res[0].content.contains("hello world") && !res[0].isError)
        #expect(ev[4] == .other(type: "system/thinking_tokens"))
        guard case .result(let r) = ev[8] else { Issue.record("no result"); return }
        #expect(r.sessionId == sid && !r.isError && r.subtype == "success" && (r.costUSD ?? 0) > 0)
        #expect(r.structuredOutput == nil)
    }

    @Test func decodesPartialMessages() throws {
        let ev = try fixtureEvents("claude-partial")
        let deltas = ev.compactMap { e -> String? in if case .textDelta(_, let t) = e { t } else { nil } }
        #expect(deltas.joined() == "one two three")
        #expect(ev.contains { if case .other(let t) = $0 { t == "stream_event/message_start" } else { false } })
    }

    @Test func toleratesGarbage() {
        #expect(StreamJSON.decode(line: Data()) == nil)
        #expect(StreamJSON.decode(line: Data("not json".utf8)) == nil)
        #expect(StreamJSON.decode(line: Data("[1,2]".utf8)) == nil)
        #expect(StreamJSON.decode(line: Data(#"{"type":"brand_new"}"#.utf8)) == .other(type: "brand_new"))
        #expect(StreamJSON.decode(line: Data(#"{"type":"assistant"}"#.utf8)) == .other(type: "assistant"))
        #expect(StreamJSON.decode(line: Data(#"{"type":"user","message":{"content":[{"type":"text","text":"x"}]}}"#.utf8)) == .other(type: "user"))
    }

    @Test func structuredOutputAndToolResultArrays() {
        let r = StreamJSON.decode(line: Data(#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"a":1},"result":"x"}"#.utf8))
        guard case .result(let res) = r else { Issue.record(); return }
        let obj = try? JSONSerialization.jsonObject(with: res.structuredOutput!) as? [String: Int]
        #expect(obj == ["a": 1])
        let n = StreamJSON.decode(line: Data(#"{"type":"result","structured_output":null}"#.utf8))
        guard case .result(let res2) = n else { Issue.record(); return }
        #expect(res2.structuredOutput == nil)
        let tr = StreamJSON.decode(line: Data(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":true,"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}]}}"#.utf8))
        #expect(tr == .toolResults([.init(toolUseId: "t", content: "a\nb", isError: true)]))
    }

    @Test func encodesUserAndInterrupt() throws {
        let u = StreamJSON.userMessage("hi \"there\"\nline2")
        #expect(u.last == 0x0A)
        #expect(u.dropLast().firstIndex(of: 0x0A) == nil, "message must be a single NDJSON line")
        let obj = try JSONSerialization.jsonObject(with: u.dropLast()) as! [String: Any]
        #expect(obj["type"] as? String == "user")
        let msg = obj["message"] as! [String: Any]
        #expect(msg["role"] as? String == "user" && msg["content"] as? String == "hi \"there\"\nline2")
        let i = try JSONSerialization.jsonObject(with: StreamJSON.interruptRequest(id: "abc").dropLast()) as! [String: Any]
        #expect(i["type"] as? String == "control_request" && i["request_id"] as? String == "abc")
        #expect((i["request"] as? [String: String])?["subtype"] == "interrupt")
    }

    @Test func lineSplitterHandlesChunkBoundaries() {
        var s = LineSplitter()
        #expect(s.feed(Data("ab".utf8)).isEmpty)
        #expect(s.feed(Data("c\nde".utf8)) == [Data("abc".utf8)])
        #expect(s.feed(Data("\n\nf\ng".utf8)) == [Data("de".utf8), Data("f".utf8)])
        #expect(s.flush() == Data("g".utf8))
        #expect(s.flush() == nil)
        // Byte-at-a-time feeding of a whole fixture yields the same lines.
        let data = try! Data(contentsOf: fixtureURL("claude-stream"))
        var whole = LineSplitter()
        let expected = whole.feed(data)
        var bytewise = LineSplitter()
        var got: [Data] = []
        for b in data { got += bytewise.feed(Data([b])) }
        #expect(got == expected && got.count == 9)
    }
}

@Suite struct TranscriptReducerTests {
    @Test func foldsToolSession() throws {
        var r = TranscriptReducer()
        r.userSent("read it", display: "Read note")
        #expect(r.isRunning)
        for e in try fixtureEvents("claude-stream") { r.apply(e) }
        #expect(!r.isRunning)
        #expect(r.sessionId == "77dde8c7-4613-4ed2-bfe5-4fa278d39098")
        #expect(r.messages.map(\.role) == [.user, .tool, .assistant])
        #expect(r.messages[0].text == "Read note")
        let tool = r.messages[1]
        #expect(tool.toolName == "Read" && tool.toolResult?.contains("hello world") == true && !tool.toolFailed)
        #expect(tool.toolSummary.hasPrefix("Read "))
        #expect(r.messages[2].text.contains("hello world") && !r.messages[2].isStreaming)
        #expect(r.lastResult?.subtype == "success")
    }

    @Test func streamedTextIsFinalized() throws {
        var r = TranscriptReducer()
        r.userSent("say")
        var sawStreaming = false
        for e in try fixtureEvents("claude-partial") {
            r.apply(e)
            if r.messages.last?.isStreaming == true { sawStreaming = true }
        }
        #expect(sawStreaming)
        let assistants = r.messages.filter { $0.role == .assistant }
        #expect(assistants.count == 1)
        #expect(assistants[0].text == "one two three" && !assistants[0].isStreaming)
    }

    @Test func errorsAndExit() {
        var r = TranscriptReducer(sessionId: "old")
        r.userSent("x")
        r.apply(.textDelta(messageId: nil, text: "partial"))
        r.apply(.result(ClaudeResult(sessionId: "new", isError: true, subtype: "error_max_turns", text: nil, costUSD: nil, structuredOutput: nil)))
        #expect(r.sessionId == "new" && !r.isRunning)
        #expect(r.messages.last?.role == .error && r.messages.last?.text.contains("error_max_turns") == true)
        #expect(r.messages.allSatisfy { !$0.isStreaming })

        r.userSent("y")
        r.processExited(status: 1, stderr: "  boom \n")
        #expect(r.messages.last?.text == "boom" && !r.isRunning)
        let count = r.messages.count
        r.processExited(status: 0, stderr: "")
        #expect(r.messages.count == count, "exit while idle adds nothing")

        r.userSent("z")
        r.processExited(status: 2, stderr: "")
        #expect(r.messages.last?.text == "Claude exited (2).")

        r.userSent("w")
        r.interrupted()
        #expect(r.messages.last?.role == .system && !r.isRunning)
    }

    @Test func toolResultMatchedAfterReload() {
        let saved = [DiscussMessage(id: "tool1", role: .tool, text: "Bash", toolName: "Bash", toolInput: #"{"command":"ls"}"#)]
        var r = TranscriptReducer(messages: saved)
        r.apply(.toolResults([.init(toolUseId: "tool1", content: "out", isError: true)]))
        #expect(r.messages[0].toolResult == "out" && r.messages[0].toolFailed)
    }

    @Test func toolSummaries() {
        func m(_ name: String, _ input: String) -> String {
            DiscussMessage(role: .tool, text: name, toolName: name, toolInput: input).toolSummary
        }
        #expect(m("Read", #"{"file_path":"/a/b/c/d/e.swift"}"#) == "Read …/c/d/e.swift")
        #expect(m("Edit", #"{"file_path":"x.swift"}"#) == "Edit x.swift")
        #expect(m("Bash", #"{"command":"npm test\necho done"}"#) == "Bash: npm test")
        #expect(m("Grep", #"{"pattern":"foo"}"#) == "Grep \"foo\"")
        #expect(m("Glob", #"{"pattern":"**/*.ts"}"#) == "Glob **/*.ts")
        #expect(m("Mystery", #"{}"#) == "Mystery")
        #expect(m("Read", "not json") == "Read")
        #expect(DiscussMessage(role: .user, text: "plain").toolSummary == "plain")
    }
}

@Suite struct ClaudeProcessTests {
    @Test func configArgs() {
        let cfg = ClaudeConfig(executable: "/bin/claude", cwd: URL(fileURLWithPath: "/tmp"), resumeSessionId: "S",
                               forkSession: true, appendSystemPrompt: "SYS", model: "opus", jsonSchema: "{}", extraArgs: ["--x"])
        let s = cfg.streamingArgs()
        #expect(s.starts(with: ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]))
        #expect(s.contains("--include-partial-messages"))
        #expect(s.contains("bypassPermissions") && s.contains("--fork-session") && s.last == "--x")
        #expect(zip(s, s.dropFirst()).contains { $0 == "--resume" && $1 == "S" })
        #expect(zip(s, s.dropFirst()).contains { $0 == "--append-system-prompt" && $1 == "SYS" })
        #expect(zip(s, s.dropFirst()).contains { $0 == "--model" && $1 == "opus" })
        #expect(!s.contains("--json-schema"))
        let o = cfg.oneShotArgs(prompt: "PROMPT")
        #expect(o.last == "PROMPT" && o.contains("json") && !o.contains("stream-json"))
        #expect(zip(o, o.dropFirst()).contains { $0 == "--json-schema" && $1 == "{}" })
        let bare = ClaudeConfig(executable: "c", cwd: URL(fileURLWithPath: "/"), appendSystemPrompt: "", model: "")
        #expect(!bare.streamingArgs().contains("--model") && !bare.streamingArgs().contains("--append-system-prompt"))
        #expect(!bare.streamingArgs().contains("--resume"))
    }

    @Test func sessionEndToEnd() async throws {
        let fixture = fixtureURL("claude-stream").path
        let argsFile = FileManager.default.temporaryDirectory.appendingPathComponent("rb-args-\(UUID().uuidString)").path
        let script = try makeScript("echo \"$@\" > '\(argsFile)'\nread line\ncat '\(fixture)'\n")
        let session = ClaudeSession(config: ClaudeConfig(executable: script.path, cwd: FileManager.default.temporaryDirectory,
                                                         resumeSessionId: "prev"))
        try session.send("hello")
        var events: [ClaudeEvent] = []
        var exitStatus: Int32?
        let collect = Task {
            for await out in session.output {
                switch out {
                case .event(let e): events.append(e)
                case .exited(let s, _): exitStatus = s; return (events, exitStatus)
                }
            }
            return (events, exitStatus)
        }
        let timeout = Task { try await Task.sleep(for: .seconds(15)); collect.cancel() }
        let (evs, status) = await collect.value
        timeout.cancel()
        #expect(status == 0)
        #expect(evs.count == 9, "\(evs)")
        #expect(evs.first.map { if case .initialized = $0 { true } else { false } } == true)
        #expect(evs.last.map { if case .result = $0 { true } else { false } } == true)
        #expect(session.currentSessionId == "77dde8c7-4613-4ed2-bfe5-4fa278d39098")
        let args = try String(contentsOfFile: argsFile, encoding: .utf8)
        #expect(args.contains("--resume prev"))
        session.finish()
    }

    @Test func oneShotParsesStructuredOutput() async throws {
        let script = try makeScript(#"""
        echo '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"S1","structured_output":{"findings":[]}}'
        """#)
        let res = try await ClaudeOneShot().run(config: ClaudeConfig(executable: script.path, cwd: FileManager.default.temporaryDirectory), prompt: "p")
        #expect(res.text == "ok" && res.sessionId == "S1")
        let obj = try JSONSerialization.jsonObject(with: res.structuredOutput!) as? [String: [Int]]
        #expect(obj == ["findings": []])
    }

    @Test func oneShotErrors() async throws {
        let cwd = FileManager.default.temporaryDirectory
        let errResult = try makeScript(#"echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"rate limited"}'; exit 1"#)
        await #expect(throws: ClaudeError.failed("rate limited")) {
            _ = try await ClaudeOneShot().run(config: ClaudeConfig(executable: errResult.path, cwd: cwd), prompt: "p")
        }
        let crash = try makeScript("echo 'auth required' >&2; exit 3")
        await #expect(throws: ClaudeError.failed("auth required\n")) {
            _ = try await ClaudeOneShot().run(config: ClaudeConfig(executable: crash.path, cwd: cwd), prompt: "p")
        }
        let junk = try makeScript("echo hello")
        await #expect(throws: ClaudeError.badOutput) {
            _ = try await ClaudeOneShot().run(config: ClaudeConfig(executable: junk.path, cwd: cwd), prompt: "p")
        }
    }
}
