import RupuDesign
import SwiftUI
import Testing
@testable import RupuRunDetail

/// Pure parser tests — `parseMarkdownBlocks` takes a `String`, returns `[MarkdownBlock]`, does no
/// I/O and touches no SwiftUI/AppKit type, so these run with no `@MainActor` isolation (unlike
/// `CodeBlockTests`, which needs it for `CodeHighlighter`). The trailing "Inline styling" section
/// below tests `styledInlineMarkdown` (also not `@MainActor`-isolated — it only builds an
/// `AttributedString` value, never touches a live `Highlighter`/`JSContext`) at the
/// `AttributedString` level, per the brief's "if cheaply assertable" ask.
struct MarkdownBlocksTests {
    // MARK: - Fences

    @Test func fenceWithLanguageCapturesLanguageAndBody() {
        let source = "```swift\nlet x = 1\nlet y = 2\n```"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.fence(language: "swift", code: "let x = 1\nlet y = 2")])
    }

    @Test func fenceWithNoLanguageTagHasNilLanguage() {
        let source = "```\nplain text\n```"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.fence(language: nil, code: "plain text")])
    }

    @Test func unterminatedFenceConsumesToEndOfInput() {
        let source = "```python\nimport os\nprint(os.getcwd())"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.fence(language: "python", code: "import os\nprint(os.getcwd())")])
    }

    @Test func unterminatedFenceWithNoBodyLinesIsEmptyCode() {
        let source = "```swift"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.fence(language: "swift", code: "")])
    }

    @Test func fenceIsFollowedByOtherBlocksWhenTerminated() {
        let source = "```swift\nlet x = 1\n```\n\nAfter the fence."
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .fence(language: "swift", code: "let x = 1"),
            .paragraph(text: "After the fence."),
        ])
    }

    // MARK: - Lists

    @Test func nestedListMarkersCaptureIndentLevel() {
        let source = """
        - top level
          - nested one level
            - nested two levels
        """
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .listItem(indent: 0, ordered: false, marker: "-", text: "top level"),
            .listItem(indent: 1, ordered: false, marker: "-", text: "nested one level"),
            .listItem(indent: 2, ordered: false, marker: "-", text: "nested two levels"),
        ])
    }

    @Test func unorderedMarkersSupportDashAsteriskAndPlus() {
        let source = "- dash\n* asterisk\n+ plus"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .listItem(indent: 0, ordered: false, marker: "-", text: "dash"),
            .listItem(indent: 0, ordered: false, marker: "*", text: "asterisk"),
            .listItem(indent: 0, ordered: false, marker: "+", text: "plus"),
        ])
    }

    @Test func orderedMarkersCaptureTheirOwnNumeral() {
        let source = "1. first\n2. second\n10. tenth"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .listItem(indent: 0, ordered: true, marker: "1.", text: "first"),
            .listItem(indent: 0, ordered: true, marker: "2.", text: "second"),
            .listItem(indent: 0, ordered: true, marker: "10.", text: "tenth"),
        ])
    }

    // MARK: - Quotes

    @Test func consecutiveQuoteLinesMergeIntoOneQuoteBlockJoinedBySpace() {
        let source = "> first line\n> second line\n> third line"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.quote(text: "first line second line third line")])
    }

    @Test func quoteBlockEndsAtANonQuoteLine() {
        let source = "> quoted\n\nNot quoted."
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .quote(text: "quoted"),
            .paragraph(text: "Not quoted."),
        ])
    }

    // MARK: - Tables

    @Test func pipeLedLineRunsAreDetectedAsATable() {
        let source = """
        | a | b |
        |---|---|
        | 1 | 2 |
        """
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.table(raw: "| a | b |\n|---|---|\n| 1 | 2 |")])
    }

    @Test func singlePipeLedLineIsStillATable() {
        let blocks = parseMarkdownBlocks("| solo |")
        #expect(blocks == [.table(raw: "| solo |")])
    }

    // MARK: - Headings

    @Test func headingLevelsAreCapturedFromHashCount() {
        let source = "# h1\n## h2\n### h3\n#### h4"
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .heading(level: 1, text: "h1"),
            .heading(level: 2, text: "h2"),
            .heading(level: 3, text: "h3"),
            .heading(level: 4, text: "h4"),
        ])
    }

    @Test func headingRequiresASpaceAfterHashes() {
        // `#tag` (no space) is not a heading — falls through to a paragraph, per CommonMark.
        let blocks = parseMarkdownBlocks("#tag")
        #expect(blocks == [.paragraph(text: "#tag")])
    }

    // MARK: - Rule

    @Test func threeDashesOnTheirOwnLineIsARule() {
        let blocks = parseMarkdownBlocks("above\n\n---\n\nbelow")
        #expect(blocks == [
            .paragraph(text: "above"),
            .rule,
            .paragraph(text: "below"),
        ])
    }

    @Test func threeAsterisksOnTheirOwnLineIsARule() {
        let blocks = parseMarkdownBlocks("***")
        #expect(blocks == [.rule])
    }

    // MARK: - Paragraphs

    @Test func softWrappedParagraphLinesJoinWithASpace() {
        let source = "This is line one\nand this is line two\nand line three."
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [.paragraph(text: "This is line one and this is line two and line three.")])
    }

    @Test func blankLineSeparatesParagraphs() {
        let source = "First paragraph.\n\nSecond paragraph."
        let blocks = parseMarkdownBlocks(source)
        #expect(blocks == [
            .paragraph(text: "First paragraph."),
            .paragraph(text: "Second paragraph."),
        ])
    }

    @Test func emptySourceProducesNoBlocks() {
        #expect(parseMarkdownBlocks("").isEmpty)
        #expect(parseMarkdownBlocks("   \n\n  ").isEmpty)
    }

    // MARK: - Inline styling (AttributedString level)

    @Test func inlineCodeSpanGetsMonoFontAndSurfaceBackground() {
        let attributed = styledInlineMarkdown("plain `code span` plain")
        var sawStyledCodeRun = false
        for run in attributed.runs {
            let substring = String(attributed[run.range].characters)
            if substring == "code span" {
                sawStyledCodeRun = true
                #expect(run.backgroundColor == .rupuSurface)
            } else {
                #expect(run.backgroundColor == nil)
            }
        }
        #expect(sawStyledCodeRun)
    }

    @Test func linkGetsBrandForegroundColor() {
        let attributed = styledInlineMarkdown("see [docs](https://example.com) for more")
        var sawLinkRun = false
        for run in attributed.runs {
            if run.link != nil {
                sawLinkRun = true
                #expect(run.foregroundColor == .rupuBrand)
            }
        }
        #expect(sawLinkRun)
    }

    @Test func plainTextWithNoInlineMarkupHasNoStyledRuns() {
        let attributed = styledInlineMarkdown("just plain text")
        #expect(String(attributed.characters) == "just plain text")
        for run in attributed.runs {
            #expect(run.backgroundColor == nil)
            #expect(run.link == nil)
        }
    }

}

// MARK: - MarkdownBlockCache (perf & interaction arc, Plan 5 Task 3:
// `MarkdownView.init` used to re-run `parseMarkdownBlocks` on every init —
// a live-streaming turn's expanded body re-parses the same byte-identical
// markdown on every re-render otherwise.)
//
// A separate, `.serialized` suite (not folded into `MarkdownBlocksTests`
// above): these tests share `MarkdownBlockCache`'s process-wide static
// cache/counter, the same reason `ActivityStoreTests` needs `.serialized` —
// Swift Testing runs a struct-based suite's tests concurrently by default,
// which would race these against each other and against `resetForTesting()`.
@Suite(.serialized)
struct MarkdownBlockCacheTests {
    @Test func repeatedIdenticalSourceReturnsEqualBlocksAndOnlyParsesOnce() {
        MarkdownBlockCache.resetForTesting()
        let source = "# Heading\n\nSome *text* and a [link](https://x.test)."

        let first = MarkdownBlockCache.blocks(for: source)
        #expect(MarkdownBlockCache.parseCallCount == 1, "the first call for new source text must be a cache miss")

        let second = MarkdownBlockCache.blocks(for: source)
        #expect(MarkdownBlockCache.parseCallCount == 1, "identical source text must be a cache hit, not a second parse")
        #expect(first == second, "a cache hit must return the exact same blocks as the original parse")
    }

    @Test func differentSourceTextEachParsesIndependently() {
        MarkdownBlockCache.resetForTesting()

        _ = MarkdownBlockCache.blocks(for: "first paragraph")
        _ = MarkdownBlockCache.blocks(for: "second, different paragraph")
        _ = MarkdownBlockCache.blocks(for: "first paragraph") // repeat of the first — must hit

        #expect(MarkdownBlockCache.parseCallCount == 2, "two distinct source strings must miss independently; the repeat must not")
    }

    /// The collision guard: `MarkdownBlockCache` keys by a digest of the
    /// source, not the full string — `blocks(for:)` re-confirms the full
    /// string on every lookup (`cached.source == source`) before trusting a
    /// hit, so two different sources always parse independently and never
    /// cross-contaminate even though many entries share the cache.
    @Test func manyDistinctSourcesEachReturnTheirOwnBlocksNeverACachedNeighbors() {
        MarkdownBlockCache.resetForTesting()
        let sources = (0..<30).map { "paragraph number \($0)" }
        for source in sources {
            _ = MarkdownBlockCache.blocks(for: source)
        }

        for source in sources {
            let blocks = MarkdownBlockCache.blocks(for: source)
            guard case .paragraph(let text) = blocks.first else {
                Issue.record("expected a single paragraph block for \"\(source)\"")
                continue
            }
            #expect(text == source, "must return this source's OWN parsed text, never a different cached entry's")
        }
    }
}
