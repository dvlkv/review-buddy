import AppKit
import SwiftUI
import Testing
@testable import ReviewBuddyCore
@testable import ReviewBuddyUI

@Suite struct MarkdownTableLayoutTests {
    @Test func naturalWidthsWhenTheyFit() {
        #expect(MarkdownTableLayout.distribute(ideal: [50, 100], available: 400, floors: [72, 72]) == [50, 100])
        #expect(MarkdownTableLayout.distribute(ideal: [50, 100], available: nil, floors: [72, 72]) == [50, 100])
    }

    @Test func squeezesWideColumnsFirst() {
        // Short column keeps its width; the two wide ones share what's left by how much they want.
        let w = MarkdownTableLayout.distribute(ideal: [60, 300, 700], available: 500, floors: [72, 72, 72])
        #expect(abs(w.reduce(0, +) - 500) < 0.001)
        #expect(w[0] == 60)
        #expect(w[2] > w[1] && w[1] >= 72)
    }

    @Test func columnKeepsItsLongestWord() {
        let w = MarkdownTableLayout.distribute(ideal: [130, 400, 600], available: 420, floors: [130, 60, 60])
        #expect(w[0] == 130)
        #expect(abs(w.reduce(0, +) - 420) < 0.001)
        #expect(MarkdownTableLayout.unbreakableRuns("src/domain/a.ts:33 is new-ish") ==
                ["src/", "domain/", "a.ts:33", "is", "new-", "ish"])
    }

    @Test func tooManyColumnsKeepFloorsAndOverflow() {
        // Words stay whole; the table becomes wider than the panel and scrolls.
        let w = MarkdownTableLayout.distribute(ideal: Array(repeating: 200, count: 10), available: 360,
                                               floors: Array(repeating: 72, count: 10))
        #expect(w == Array(repeating: 72, count: 10))
    }

    @Test func degenerateInputs() {
        #expect(MarkdownTableLayout.distribute(ideal: [], available: 100, floors: []) == [])
        #expect(MarkdownTableLayout.distribute(ideal: [0, 0], available: 100, floors: []) == [0, 0])
        #expect(MarkdownTableLayout.distribute(ideal: [10, 10], available: 0, floors: [72, 72]) == [10, 10])
    }
}

@MainActor
@Suite struct MarkdownViewRenderTests {
    static let reply = """
    В AffiliateEventType (`prisma/schema.prisma:202`) четыре значения:

    | Значение | Где пишется | Когда |
    |---|:---:|---|
    | KICKED | src/domain/affiliates/kickAffiliate.ts:33 | Оператор кикает аффилиата (status → KICKED) |
    | REINSTATED | src/domain/affiliates/reinstateAffiliate.ts:18 | Оператор восстанавливает кикнутого аффилиата |
    | REFERRAL_BOUND | src/domain/graph/bindReferral.ts:29 | **Новое в этом PR.** Оператор привязал юзера к этому аффилиату как к рефереру. В payload лежит `{ referredUserId, reason }` |

    Откуда они в базе:

    - Первые три появились в init-миграции: `prisma/migrations/20260721150000_init/migration.sql:20`.
      - nested with a link https://github.com/dvlkv/review-buddy
    - [x] task done

    > Quote with **bold**

    ```swift
    let x = 1 // comment
    func f() -> String { "s" }
    ```

    ## Heading
    ---
    """

    /// Lays the view out in a real hosting view at a narrow (Discuss panel) width; the
    /// table must wrap to fit instead of overflowing, and nothing may trap.
    @Test func rendersReplyAtPanelWidth() throws {
        let host = NSHostingView(rootView: MarkdownView(text: Self.reply).frame(width: 380).fixedSize(horizontal: false, vertical: true))
        host.frame = NSRect(x: 0, y: 0, width: 380, height: 2000)
        host.layoutSubtreeIfNeeded()
        let size = host.fittingSize
        #expect(size.width <= 381)
        #expect(size.height > 200 && size.height < 2000)
        if let dir = ProcessInfo.processInfo.environment["RB_MARKDOWN_SNAPSHOT"] {
            host.frame.size = size
            host.layoutSubtreeIfNeeded()
            let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("markdown.png"))
        }
    }

    @Test func streamingPrefixesLayOut() {
        let text = Self.reply
        let host = NSHostingView(rootView: MarkdownView(text: ""))
        host.frame = NSRect(x: 0, y: 0, width: 380, height: 800)
        var idx = text.startIndex
        var step = 0
        while idx < text.endIndex {
            host.rootView = MarkdownView(text: String(text[..<idx]))
            host.layoutSubtreeIfNeeded()
            step += 1
            idx = text.index(idx, offsetBy: 37, limitedBy: text.endIndex) ?? text.endIndex
        }
        #expect(step > 10)
    }

    @Test func inlineMarkdownAddsCodeBackgroundAndAutolinks() {
        let a = MarkdownInline.attributed("see `x` and https://example.com/a and [l](https://b.dev)")
        #expect(a.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true && $0.backgroundColor != nil })
        let links = a.runs.compactMap(\.link).map(\.absoluteString)
        #expect(links.contains("https://example.com/a"))
        #expect(links.contains("https://b.dev"))
        // A URL in code stays code.
        #expect(MarkdownInline.attributed("`https://x.dev`").runs.allSatisfy { $0.link == nil })
    }

    @Test func codeHighlightingColorsKnownLanguagesOnly() {
        let swift = MarkdownInline.highlighted("let x = \"s\" // c", language: "swift")
        #expect(String(swift.characters) == "let x = \"s\" // c")
        #expect(swift.runs.contains { $0.foregroundColor != nil })
        let unknown = MarkdownInline.highlighted("let x", language: "nonsense")
        #expect(unknown.runs.allSatisfy { $0.foregroundColor == nil })
        let diff = MarkdownInline.highlighted("+a\n-b\n c", language: "diff")
        #expect(String(diff.characters) == "+a\n-b\n c")
        #expect(diff.runs.filter { $0.foregroundColor != nil }.count == 2)
    }
}
