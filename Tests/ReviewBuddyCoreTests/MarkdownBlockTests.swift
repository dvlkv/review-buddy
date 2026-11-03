import Foundation
import Testing
@testable import ReviewBuddyCore

private func parse(_ s: String) -> [MarkdownBlock] { MarkdownParser.parse(s) }

private func table(_ b: [MarkdownBlock], at i: Int = 0) -> MarkdownTable? {
    guard i < b.count, case .table(let t) = b[i] else { return nil }
    return t
}

private func list(_ b: [MarkdownBlock], at i: Int = 0) -> MarkdownList? {
    guard i < b.count, case .list(let l) = b[i] else { return nil }
    return l
}

@Suite struct MarkdownTableTests {
    @Test func basicTableWithAlignment() {
        let b = parse("""
        | Left | Center | Right | None |
        |:-----|:------:|------:|------|
        | a | **b** | `c` | d |
        """)
        let t = try! #require(table(b))
        #expect(b.count == 1)
        #expect(t.header == ["Left", "Center", "Right", "None"])
        #expect(t.alignments == [.left, .center, .right, .none])
        #expect(t.rows == [["a", "**b**", "`c`", "d"]])
    }

    @Test func missingOuterPipesAndRaggedRows() {
        let b = parse("""
        a | b | c
        --|---|--
        1 | 2
        1 | 2 | 3 | 4
        | only |
        """)
        let t = try! #require(table(b))
        #expect(t.header == ["a", "b", "c"])
        #expect(t.rows == [["1", "2", ""], ["1", "2", "3"], ["only", "", ""]])
    }

    @Test func escapedPipesAndPipesInCode() {
        let b = parse("""
        | Expr | Meaning |
        |---|---|
        | `a | b` | bitwise or |
        | a \\| b | escaped |
        | `x` | unclosed ` tick | extra |
        """)
        let t = try! #require(table(b))
        #expect(t.rows[0] == ["`a | b`", "bitwise or"])
        #expect(t.rows[1] == ["a | b", "escaped"])
        #expect(t.rows[2] == ["`x`", "unclosed ` tick"])
    }

    @Test func headerAndDelimiterCountsMustMatch() {
        // Three header cells, two delimiter cells: GFM says it's not a table.
        #expect(table(parse("| a | b | c |\n|---|---|")) == nil)
        #expect(parse("| a | b | c |\n|---|---|").allSatisfy { if case .paragraph = $0 { true } else { false } })
    }

    @Test func tableEndsAtBlankLineAndBeforeList() {
        let b = parse("""
        | k | v |
        |---|---|
        | 1 | 2 |
        - after
        - list

        tail
        """)
        #expect(table(b)?.rows == [["1", "2"]])
        #expect(list(b, at: 1)?.items.count == 2)
        #expect(b.last == .paragraph("tail"))
    }

    @Test func tableInterruptsParagraph() {
        let b = parse("Values:\n| a | b |\n|---|---|\n| 1 | 2 |")
        #expect(b.first == .paragraph("Values:"))
        #expect(table(b, at: 1)?.header == ["a", "b"])
    }

    @Test func brTagsBecomeLineBreaksInCells() {
        let t = table(parse("| a |\n|---|\n| one<br>two<br/>three |"))
        #expect(t?.rows == [["one\ntwo\nthree"]])
    }

    @Test func loneDashesAreNotATable() {
        #expect(parse("a\n---") == [.heading(level: 2, text: "a")])
        #expect(parse("---") == [.rule])
    }

    /// Streaming: each prefix of a table must parse without trapping and end as a table.
    @Test func everyStreamingPrefixParses() {
        let full = """
        | Значение | Где пишется | Когда |
        |---|---|---|
        | KICKED | src/domain/affiliates/kickAffiliate.ts:33 | Оператор кикает аффилиата |
        | REINSTATED | src/domain/affiliates/reinstateAffiliate.ts:18 | Оператор восстанавливает |
        """
        var idx = full.startIndex
        while idx < full.endIndex {
            _ = parse(String(full[..<idx]))
            idx = full.index(after: idx)
        }
        #expect(table(parse(full))?.rows.count == 2)
        // Half a delimiter row is a (body-less) table as soon as every column has a dash.
        #expect(table(parse("| a | b |\n|---|-"))?.rows == [])
        // Half a body row is padded.
        #expect(table(parse("| a | b |\n|---|---|\n| x"))?.rows == [["x", ""]])
    }
}

@Suite struct MarkdownCodeFenceTests {
    @Test func languageAndInfoString() {
        #expect(parse("```swift title=x\nlet a = 1\n```") == [.code(language: "swift", text: "let a = 1")])
        #expect(parse("````\n```\ninner\n```\n````") == [.code(language: nil, text: "```\ninner\n```")])
    }

    @Test func closingFenceMustBeBareAndLongEnough() {
        // "```swift" inside a block doesn't close it; neither does a shorter fence.
        #expect(parse("```\na\n```swift\nb\n```") == [.code(language: nil, text: "a\n```swift\nb")])
        #expect(parse("~~~~\na\n~~~\n~~~~") == [.code(language: nil, text: "a\n~~~")])
        #expect(parse("~~~\na\n```\n~~~") == [.code(language: nil, text: "a\n```")])
    }

    @Test func unterminatedFenceKeepsEverything() {
        #expect(parse("text\n```ts\nconst a = 1\n\n| not | a table |") ==
                [.paragraph("text"), .code(language: "ts", text: "const a = 1\n\n| not | a table |")])
        #expect(parse("```") == [.code(language: nil, text: "")])
    }

    @Test func markdownInsideCodeIsLiteral() {
        let b = parse("```md\n# not heading\n- not list\n<!-- kept -->\n```")
        #expect(b == [.code(language: "md", text: "# not heading\n- not list\n<!-- kept -->")])
    }

    @Test func indentedFenceStripsItsIndent() {
        #expect(parse("  ```\n  a\n    b\n  ```") == [.code(language: nil, text: "a\n  b")])
    }

    @Test func inlineTripleBackticksAreNotAFence() {
        #expect(parse("```x``` is inline") == [.paragraph("```x``` is inline")])
    }
}

@Suite struct MarkdownListTests {
    @Test func nestedListsWithTwoThreeAndFourSpaces() {
        let b = parse("""
        - a
          - a1
            - a1x
        - b
        1. one
           - one-a
        2. two
            1. two-1
        """)
        let outer = try! #require(list(b))
        #expect(!outer.ordered && outer.items.count == 2)
        let a = outer.items[0].blocks
        #expect(a.first == .paragraph("a"))
        let a1 = try! #require(list(a, at: 1))
        #expect(a1.items[0].blocks.first == .paragraph("a1"))
        #expect(list(a1.items[0].blocks, at: 1)?.items[0].blocks == [.paragraph("a1x")])
        let ordered = try! #require(list(b, at: 1))
        #expect(ordered.ordered && ordered.items.count == 2)
        #expect(list(ordered.items[0].blocks, at: 1)?.items[0].blocks == [.paragraph("one-a")])
        let two1 = try! #require(list(ordered.items[1].blocks, at: 1))
        #expect(two1.ordered && two1.items[0].blocks == [.paragraph("two-1")])
    }

    @Test func twoSpaceNestingUnderOrderedItem() {
        let b = parse("1. top\n  - child\n2. next")
        let l = try! #require(list(b))
        #expect(l.items.count == 2)
        #expect(list(l.items[0].blocks, at: 1)?.items.first?.blocks == [.paragraph("child")])
    }

    @Test func startNumberAndLazyContinuation() {
        let l = try! #require(list(parse("3. three\ncontinued\n4. four")))
        #expect(l.start == 3)
        #expect(l.items.map(\.blocks) == [[.paragraph("three\ncontinued")], [.paragraph("four")]])
    }

    @Test func looseListAndParagraphAfterBlank() {
        let b = parse("- a\n\n- b\n\n  more b\n\nafter")
        let l = try! #require(list(b))
        #expect(l.items.map(\.blocks) == [[.paragraph("a")], [.paragraph("b"), .paragraph("more b")]])
        #expect(b.last == .paragraph("after"))
    }

    @Test func codeAndTableInsideItems() {
        let b = parse("""
        - step:
          ```sh
          make
          ```
        - table:
          | a | b |
          |---|---|
          | 1 | 2 |
        """)
        let l = try! #require(list(b))
        #expect(l.items[0].blocks == [.paragraph("step:"), .code(language: "sh", text: "make")])
        #expect(table(l.items[1].blocks, at: 1)?.rows == [["1", "2"]])
    }

    @Test func taskItemsMixWithPlainItems() {
        let l = try! #require(list(parse("- [x] done\n- [ ] todo\n- plain\n- [X]")))
        #expect(l.items.map(\.checked) == [true, false, nil, true])
        #expect(l.items[3].blocks == [])
    }

    @Test func markersThatAreNotLists() {
        #expect(parse("**bold** start") == [.paragraph("**bold** start")])
        #expect(parse("-1 is negative") == [.paragraph("-1 is negative")])
        #expect(parse("* * *") == [.rule])
        // An ordered item not starting at 1 doesn't interrupt a paragraph.
        #expect(parse("in 2024\n2. not a list") == [.paragraph("in 2024\n2. not a list")])
        #expect(list(parse("text\n- interrupts"), at: 1) != nil)
    }

    @Test func differentListTypesSplit() {
        let b = parse("- a\n1. b")
        #expect(list(b, at: 0)?.ordered == false)
        #expect(list(b, at: 1)?.ordered == true)
    }
}

@Suite struct MarkdownMiscBlockTests {
    @Test func quotesNestAndContainBlocks() {
        let b = parse("> # Title\n> - a\n> > inner\nlazy")
        guard case .quote(let q) = b.first else { Issue.record("no quote"); return }
        #expect(q.first == .heading(level: 1, text: "Title"))
        #expect(list(q, at: 1) != nil)
        guard case .quote(let inner) = q.last else { Issue.record("no inner quote"); return }
        #expect(inner == [.paragraph("inner\nlazy")])
    }

    @Test func headingsVariants() {
        #expect(parse("## Title ##") == [.heading(level: 2, text: "Title")])
        #expect(parse("#") == [.heading(level: 1, text: "")])
        #expect(parse("####### seven") == [.paragraph("####### seven")])
        #expect(parse("Big\n===") == [.heading(level: 1, text: "Big")])
        #expect(parse("C# is a language") == [.paragraph("C# is a language")])
    }

    @Test func htmlCommentsAreHidden() {
        #expect(parse("<!-- template\nhint -->\nBody") == [.paragraph("Body")])
        #expect(parse("Body\n<!-- never closed\nhidden") == [.paragraph("Body")])
    }

    @Test func tabsAndCRLines() {
        #expect(parse("-\ta\r\n\t- b") == parse("- a\n    - b"))
        #expect(parse("a\rb") == [.paragraph("a\nb")])
    }

    @Test func deepNestingDoesNotBlowTheStack() {
        let deep = String(repeating: "> ", count: 500) + "x"
        #expect(!parse(deep).isEmpty)
        let deepList = (0..<300).map { String(repeating: "  ", count: $0) + "- l\($0)" }.joined(separator: "\n")
        #expect(list(parse(deepList)) != nil)
    }

    /// The reply from the bug report: a paragraph, a table, a list, paragraphs.
    @Test func claudeReplyFromScreenshot() {
        let md = """
        В AffiliateEventType (prisma/schema.prisma:202) четыре значения:

        | Значение | Где пишется | Когда |
        |---|---|---|
        | KICKED | src/domain/affiliates/kickAffiliate.ts:33 | Оператор кикает аффилиата (status → KICKED) |
        | REINSTATED | src/domain/affiliates/reinstateAffiliate.ts:18 | Оператор восстанавливает кикнутого аффилиата |
        | TIER_CHANGED | src/domain/affiliates/updateAffiliateTier.ts:29 | Оператор меняет currentTierIndex |
        | REFERRAL_BOUND | src/domain/graph/bindReferral.ts:29 | **Новое в этом PR.** Оператор привязал юзера. В payload лежит `{ referredUserId, reason }` |

        Откуда они в базе:

        - Первые три появились в init-миграции: `prisma/migrations/20260721150000_init/migration.sql:20`.
        - REFERRAL_BOUND добавляет миграция `prisma/migrations/20261103120000_referral_bound_event/migration.sql:2` через `ALTER TYPE … ADD VALUE`.

        В OpenAPI-схеме админки enum дописан вручную, в `src/routes/admin.ts:131`.

        Не путать с HqEventType.
        """
        let b = parse(md)
        #expect(b.count == 6)
        #expect(b[0] == .paragraph("В AffiliateEventType (prisma/schema.prisma:202) четыре значения:"))
        let t = try! #require(table(b, at: 1))
        #expect(t.header == ["Значение", "Где пишется", "Когда"])
        #expect(t.rows.count == 4)
        #expect(t.rows[3][0] == "REFERRAL_BOUND")
        #expect(t.rows[3][2].hasSuffix("`{ referredUserId, reason }`"))
        #expect(b[2] == .paragraph("Откуда они в базе:"))
        #expect(list(b, at: 3)?.items.count == 2)
        #expect(b[5] == .paragraph("Не путать с HqEventType."))
    }

    @Test func randomInputNeverTraps() {
        var rng = SystemRandomNumberGenerator()
        let pieces = ["|", "-", ":", "`", "```", "~~~", ">", "#", " ", "  ", "\n", "\t", "1.", "*", "[x]", "a", "Ж", "<!--", "-->", "\\", "<br>"]
        for _ in 0..<400 {
            let s = (0..<Int.random(in: 0..<60, using: &rng)).map { _ in pieces.randomElement(using: &rng)! }.joined()
            _ = parse(s)
        }
    }
}
