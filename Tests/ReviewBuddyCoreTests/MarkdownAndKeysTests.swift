import Foundation
import Testing
@testable import ReviewBuddyCore

@Suite struct MarkdownParserTests {
    @Test func mixedDocument() {
        let md = """
        # Title
        First line
        second line

        ```swift
        let x = 1
        ```

        ```suggestion
        let y = 2
        ```
        > quoted
        > more
        - a
        - b
        1. one
        2) two
        - [ ] todo
        - [x] done
        ---
        ### Small
        """
        let b = MarkdownParser.parse(md)
        #expect(b == [
            .heading(level: 1, text: "Title"),
            .paragraph("First line\nsecond line"),
            .code(language: "swift", text: "let x = 1"),
            .suggestion("let y = 2"),
            .quote("quoted\nmore"),
            .list(ordered: false, items: ["a", "b"]),
            .list(ordered: true, items: ["one", "two"]),
            .taskList([(done: false, text: "todo"), (done: true, text: "done")]),
            .rule,
            .heading(level: 3, text: "Small"),
        ])
    }

    @Test func edgeCases() {
        #expect(MarkdownParser.parse("") == [])
        #expect(MarkdownParser.parse("#nospace") == [.paragraph("#nospace")])
        #expect(MarkdownParser.parse("a\r\nb\r\n\r\nc") == [.paragraph("a\nb"), .paragraph("c")])
        #expect(MarkdownParser.parse("```\nunterminated\ncode") == [.code(language: nil, text: "unterminated\ncode")])
        #expect(MarkdownParser.parse("~~~\nx\n~~~") == [.code(language: nil, text: "x")])
        #expect(MarkdownParser.parse("```suggestion\n```") == [.suggestion("")])
        #expect(MarkdownParser.parse("- [ ] a\n- b") == [.list(ordered: false, items: ["[ ] a", "b"])])
    }

    @Test func suggestionTemplateRoundTrips() {
        let t = MarkdownParser.suggestionTemplate(for: ["a", "  b"])
        #expect(t == "```suggestion\na\n  b\n```")
        #expect(MarkdownParser.parse(t) == [.suggestion("a\n  b")])
    }
}

@Suite struct KeyBindingsTests {
    @Test func mapsKeys() {
        #expect(KeyBindings.action(for: KeyStroke("j"), textInputFocused: false) == .nextFile)
        #expect(KeyBindings.action(for: KeyStroke("J"), textInputFocused: false) == .nextFile)
        #expect(KeyBindings.action(for: KeyStroke("d"), textInputFocused: false) == .discuss)
        #expect(KeyBindings.action(for: KeyStroke("c"), textInputFocused: false) == .comment)
        #expect(KeyBindings.action(for: KeyStroke("k", command: true), textInputFocused: false) == .commandPalette)
        #expect(KeyBindings.action(for: KeyStroke("q"), textInputFocused: false) == nil)
        #expect(KeyBindings.action(for: KeyStroke("j", shift: true), textInputFocused: false) == nil)
    }

    @Test func textFocusSuppressesPlainKeys() {
        #expect(KeyBindings.action(for: KeyStroke("j"), textInputFocused: true) == nil)
        #expect(KeyBindings.action(for: KeyStroke("\u{1b}"), textInputFocused: true) == .cancel)
        #expect(KeyBindings.action(for: KeyStroke("\r", command: true), textInputFocused: true) == .submit)
        #expect(KeyBindings.action(for: KeyStroke("l", command: true), textInputFocused: true) == .openByURL)
    }

    @Test func everyActionHasTitle() {
        for a in ReviewAction.allCases { #expect(!a.title.isEmpty) }
        #expect(ReviewAction.discuss.shortcut == "D")
    }

    @Test func fuzzyScore() {
        #expect(KeyBindings.fuzzyScore("", "anything") == 0)
        #expect(KeyBindings.fuzzyScore("xyz", "next file") == nil)
        #expect(KeyBindings.fuzzyScore("nf", "Next file") != nil)
        let contiguous = KeyBindings.fuzzyScore("next", "Next file")!
        let scattered = KeyBindings.fuzzyScore("next", "No extra text")!
        #expect(contiguous > scattered)
        let shortName = KeyBindings.fuzzyScore("app", "src/App.swift")!
        let longName = KeyBindings.fuzzyScore("app", "src/very/long/path/to/App.swift")!
        #expect(shortName > longName)
        #expect(KeyBindings.fuzzyScore("ba", "ab") == nil, "order matters")
    }
}

@Suite struct ProcessRunnerTests {
    let r = ProcessRunner()

    @Test func stdoutAndStatus() async throws {
        let res = try await r.run("/bin/echo", ["hello"], cwd: nil, env: nil, stdin: nil)
        #expect(res.status == 0 && res.stdoutString == "hello\n")
        let cwd = try await r.check("/bin/pwd", [], cwd: URL(fileURLWithPath: "/usr"))
        #expect(cwd.stdoutString == "/usr\n")
        let env = try await r.check("/bin/sh", ["-c", "echo $RB_X"], env: ["RB_X": "42"])
        #expect(env.stdoutString == "42\n")
    }

    @Test func checkThrowsWithStderr() async throws {
        do {
            try await r.check("/bin/sh", ["-c", "echo oops >&2; exit 4"])
            Issue.record("should throw")
        } catch let e as ProcessError {
            #expect(e.status == 4 && e.stderr == "oops\n")
            #expect(e.errorDescription == "`sh -c echo oops >&2; exit 4` failed (4): oops")
        }
    }

    @Test func stdinPiping() async throws {
        let res = try await r.check("/bin/cat", [], stdin: Data("piped\ninput".utf8))
        #expect(res.stdoutString == "piped\ninput")
    }

    @Test func largeOutputDoesNotDeadlock() async throws {
        let res = try await r.check("/bin/sh", ["-c", "head -c 5000000 /dev/zero; head -c 300000 /dev/zero >&2"])
        #expect(res.stdout.count == 5_000_000)
        #expect(res.stderr.count == 300_000)
        let big = Data(repeating: 65, count: 3_000_000)
        let echoed = try await r.check("/bin/cat", [], stdin: big)
        #expect(echoed.stdout == big)
    }

    @Test func cancellationStopsProcess() async throws {
        let start = ContinuousClock.now
        let task = Task { try await r.run("/bin/sleep", ["10"], cwd: nil, env: nil, stdin: nil) }
        try await Task.sleep(for: .milliseconds(300))
        task.cancel()
        let res = try await task.value
        #expect(res.status != 0)
        #expect(ContinuousClock.now - start < .seconds(5))
    }

    @Test func missingExecutableThrows() async {
        await #expect(throws: (any Error).self) {
            _ = try await r.run("/nonexistent/tool", [], cwd: nil, env: nil, stdin: nil)
        }
    }

    @Test func toolLocator() {
        #expect(ToolLocator.find("sh") != nil)
        #expect(ToolLocator.find("definitely-not-a-tool-xyz") == nil)
        #expect(ToolLocator.searchPath.contains("/opt/homebrew/bin"))
    }
}
