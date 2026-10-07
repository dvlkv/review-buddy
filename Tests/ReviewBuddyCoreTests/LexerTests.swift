import Testing
@testable import ReviewBuddyCore

@Suite struct LexerTests {
    func kinds(_ text: String, _ lang: LanguageSpec, state: LexState = .normal) -> [(String, TokenKind)] {
        var s = state
        let u = Array(text.utf16)
        return Lexer(spec: lang).tokenize(text, state: &s).map {
            (String(utf16CodeUnits: Array(u[Int($0.start)..<Int($0.start + $0.length)]), count: Int($0.length)), $0.kind)
        }
    }

    @Test func swiftBasics() {
        let t = kinds("let x: Int = foo(42) // hi", LanguageRegistry.swift)
        #expect(t.map(\.0) == ["let", "Int", "foo", "42", "// hi"])
        #expect(t.map(\.1) == [.keyword, .type, .function, .number, .comment])
    }

    @Test func stringsWithEscapes() {
        let t = kinds(#"print("a \"q\" b", 'c')"#, LanguageRegistry.typescript)
        #expect(t.contains { $0.0 == #""a \"q\" b""# && $0.1 == .string })
        #expect(t.contains { $0.0 == "'c'" && $0.1 == .string })
    }

    @Test func blockCommentSpansLines() {
        var s = LexState.normal
        let lx = Lexer(spec: LanguageRegistry.swift)
        _ = lx.tokenize("let a = 1 /* start", state: &s)
        #expect(s == .blockComment(0))
        let t2 = lx.tokenize("still comment", state: &s)
        #expect(t2 == [SyntaxToken(start: 0, length: 13, kind: .comment)])
        let t3 = lx.tokenize("end */ let b", state: &s)
        #expect(s == .normal)
        #expect(t3.map(\.kind) == [.comment, .keyword])
    }

    @Test func multilineStrings() {
        var s = LexState.normal
        let lx = Lexer(spec: LanguageRegistry.python)
        _ = lx.tokenize("x = \"\"\"doc", state: &s)
        #expect(s == .multilineString(0))
        let t = lx.tokenize("done\"\"\" + y", state: &s)
        #expect(t.first?.kind == .string)
        #expect(s == .normal)
    }

    @Test func memberAccessIsNotKeyword() {
        let t = kinds("obj.default = 1", LanguageRegistry.typescript)
        #expect(!t.contains { $0.0 == "default" && $0.1 == .keyword })
    }

    @Test func jsonKeysAreProperties() {
        let t = kinds(#"  "name": "x", "n": 1, "ok": true"#, LanguageRegistry.json)
        #expect(t.map(\.1) == [.property, .string, .property, .number, .property, .constant])
    }

    @Test func yamlKeys() {
        let t = kinds("  - name: build # comment", LanguageRegistry.yaml)
        #expect(t.map(\.1) == [.property, .comment])
    }

    @Test func markupTags() {
        let t = kinds(#"<div class="a">hi</div>"#, LanguageRegistry.markup)
        #expect(t.map(\.1) == [.tag, .attribute, .string, .tag])
    }

    @Test func attributesAndDecorators() {
        #expect(kinds("@MainActor final class A {}", LanguageRegistry.swift).first! == ("@MainActor", .attribute))
        #expect(kinds("@property", LanguageRegistry.python).first! == ("@property", .attribute))
    }

    @Test func unicodeOffsetsAreUTF16() {
        let t = kinds("let 🎉 = \"é\"", LanguageRegistry.swift)
        #expect(t.last! == ("\"é\"", .string))
    }

    @Test func registryByPath() {
        #expect(LanguageRegistry.language(forPath: "a/b/c.tsx").name == "TypeScript")
        #expect(LanguageRegistry.language(forPath: "Dockerfile").name == "Shell")
        #expect(LanguageRegistry.language(forPath: "README").name == "Text")
        #expect(LanguageRegistry.language(forPath: "x.YML").name == "YAML")
    }

    @Test func unterminatedStringStopsAtLineEnd() {
        let t = kinds("let s = \"oops", LanguageRegistry.swift)
        #expect(t.last! == ("\"oops", .string))
    }

    @Test func diffHighlighterTracksSidesSeparately() {
        let d = UnifiedDiffParser.parse("--- a/x.swift\n+++ b/x.swift\n@@ -1,2 +1,2 @@\n-/* old\n+let a = 1\n x */ let\n")
        let toks = DiffHighlighter.highlight(file: d.files[0], in: d)
        #expect(toks[0] == [SyntaxToken(start: 0, length: 6, kind: .comment)])
        #expect(toks[1].first?.kind == .keyword)
        // Context continues from the NEW side state (no open comment) → "let" keyword
        #expect(toks[2].contains { $0.kind == .keyword })
    }

    @Test func plainTextHasNoTokens() {
        let d = UnifiedDiffParser.parse("--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+b\n")
        #expect(DiffHighlighter.highlight(file: d.files[0], in: d).allSatisfy { $0.isEmpty })
    }
}
