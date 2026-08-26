import Testing
import Foundation
import RupuAPI
import SwiftUI
@testable import RupuRunDetail

/// `AstGrepTranscriptParsing` + `AstGrepBodyView`'s pure model-layer tests —
/// moved here from `SourcePreviewTests.swift` (Task 6, design-alignment Plan
/// 4) alongside the type itself (`Rendering/AstGrepBody.swift`), extended
/// with the new metavar-bindings coverage that task added.

// MARK: - `fromStructured` (moved, unchanged behavior; `fileCount` is a new
// required field on `StructuredResult` — every construction below supplies
// it explicitly)

@Test func fromStructuredParsesWellFormedMatchesAndSkipsMalformedOnes() {
    let structured = JSONValue.object([
        "matchCount": .number(1),
        "fileCount": .number(1),
        "truncated": .bool(false),
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object([
                    "startLine": .number(12), "startCol": .number(3),
                    "endLine": .number(12), "endCol": .number(9),
                ]),
                "text": .string("fn foo()"),
            ]),
            // Missing `range` — must be skipped, not crash or fabricate a location.
            .object(["file": .string("src/b.rs")]),
            // Missing `file` — must be skipped.
            .object(["range": .object(["startLine": .number(1), "startCol": .number(1)])]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult for a present matches array")
        return
    }

    #expect(result.matches == [
        AstGrepTranscriptParsing.Match(file: "src/a.rs", startLine: 12, startCol: 3, text: "fn foo()"),
    ])
    #expect(result.matchCount == 1)
    #expect(!result.truncated)
}

@Test func fromStructuredReturnsNilForAbsentOrShapelessInput() {
    #expect(AstGrepTranscriptParsing.fromStructured(nil) == nil)
    #expect(AstGrepTranscriptParsing.fromStructured(.object([:])) == nil, "no `matches` key at all must fall back, same as the web's own trigger")
    #expect(AstGrepTranscriptParsing.fromStructured(.string("not an object")) == nil)
}

// MARK: - Finding 1 (review fix, retained from the pre-move suite): truthful
// truncation — `matchCount`/`truncated` are read off the wire, not
// discarded, and the label renders the web's own "showing first N of M"
// shape under truncation.

@Test func fromStructuredReadsMatchCountAndTruncatedFromTheWire() {
    let structured = JSONValue.object([
        "matchCount": .number(250),
        "truncated": .bool(true),
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)]),
            ]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.matchCount == 250, "the server's real total, not matches.count (which is the capped prefix)")
    #expect(result.truncated)
}

@Test func fromStructuredFallsBackToMatchesCountWhenMatchCountFieldIsMissing() {
    let structured = JSONValue.object([
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)]),
            ]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.matchCount == 1, "an honest read of \"assume the total is exactly what we parsed,\" not a fabricated 0")
    #expect(!result.truncated, "an absent `truncated` field must never be read as truncated")
}

// MARK: - New (Task 6): `fileCount`/`pattern`/`lang` extraction

@Test func fromStructuredReadsFileCountPatternAndLangFromTheWire() {
    let structured = JSONValue.object([
        "pattern": .string("fn $NAME($$$ARGS)"),
        "lang": .string("rust"),
        "matchCount": .number(1),
        "fileCount": .number(1),
        "truncated": .bool(false),
        "matches": .array([
            .object([
                "file": .string("src/lib.rs"),
                "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)]),
            ]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.pattern == "fn $NAME($$$ARGS)")
    #expect(result.lang == "rust")
    #expect(result.fileCount == 1)
}

@Test func fromStructuredFallsBackToUniqueFileCountWhenFileCountFieldIsMissing() {
    let structured = JSONValue.object([
        "matchCount": .number(2),
        "truncated": .bool(false),
        "matches": .array([
            .object(["file": .string("a.rs"), "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)])]),
            .object(["file": .string("a.rs"), "range": .object(["startLine": .number(2), "startCol": .number(1), "endLine": .number(2), "endCol": .number(1)])]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.fileCount == 1, "two matches in the SAME file must count as one file, not fabricate 2")
    #expect(result.pattern == nil)
    #expect(result.lang == nil)
}

// MARK: - New (Task 6): metavar decode against the real Task 2 fixture wire
// shape (`apps/rupu-macos/Fixtures/transcript_events.json`'s `call-3`
// `tool_result`, sourced from `crates/rupu-tools/src/ast_grep.rs:299-333`).

@Test func fromStructuredParsesTheRealMetaVarsShapeFromTheFixture() throws {
    let events = try JSONDecoder().decode([TranscriptEvent].self, from: Fixtures.data("transcript_events.json"))
    let call3Structured: JSONValue? = events.compactMap { event -> JSONValue? in
        if case .toolResult("call-3", _, _, _, let structured) = event { return structured }
        return nil
    }.first

    guard let call3Structured else {
        Issue.record("expected a call-3 tool_result in the fixture")
        return
    }
    guard let result = AstGrepTranscriptParsing.fromStructured(call3Structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.pattern == "fn $NAME($$$ARGS)")
    #expect(result.lang == "rust")
    #expect(result.matchCount == 1)
    #expect(result.fileCount == 1)
    #expect(!result.truncated)
    #expect(result.matches.count == 1)

    guard let match = result.matches.first else {
        Issue.record("expected one parsed match")
        return
    }
    #expect(match.file == "src/lib.rs")
    #expect(match.startLine == 10)
    #expect(match.startCol == 5)
    #expect(match.text == "fn process(x: i32) -> i32 {\n    x + 1\n}")

    // Flattened `single` (sorted by name) then `multi` (sorted by name, each
    // array's own bindings in original order) — see `parseMetaVars`'s doc
    // comment for the exact contract.
    #expect(match.metaVars == [
        AstGrepTranscriptParsing.MetaVar(
            name: "NAME", index: nil, text: "process",
            textOffset: .init(start: 3, end: 10)
        ),
        AstGrepTranscriptParsing.MetaVar(
            name: "ARGS", index: 0, text: "x: i32",
            textOffset: .init(start: 11, end: 17)
        ),
    ])
    #expect(match.metaVars[0].displayName == "$NAME")
    #expect(match.metaVars[1].displayName == "$ARGS[0]")
}

// MARK: - New (Task 6): `parseMetaVars` ordering/dropping contract in
// isolation (synthetic — multiple `multi` items, a binding missing `text`)

@Test func parseMetaVarsOrdersSingleBeforeMultiAndSortsByNameAndKeepsMultiArrayOrder() {
    let value = JSONValue.object([
        "single": .object([
            "Z": .object(["text": .string("z-value")]),
            "A": .object(["text": .string("a-value")]),
        ]),
        "multi": .object([
            "ITEMS": .array([
                .object(["text": .string("first")]),
                .object(["text": .string("second")]),
            ]),
        ]),
    ])

    let flattened = AstGrepTranscriptParsing.parseMetaVars(value)

    #expect(flattened.map(\.name) == ["A", "Z", "ITEMS", "ITEMS"], "single (sorted by name) before multi; multi array order preserved")
    #expect(flattened.map(\.text) == ["a-value", "z-value", "first", "second"])
    #expect(flattened[2].index == 0 && flattened[3].index == 1)
    #expect(flattened[0].index == nil && flattened[1].index == nil)
}

@Test func parseMetaVarsDropsABindingMissingTextRatherThanFabricatingAnEmptyString() {
    let value = JSONValue.object([
        "single": .object([
            "OK": .object(["text": .string("kept")]),
            "BAD": .object(["notText": .string("dropped")]),
        ]),
    ])

    let flattened = AstGrepTranscriptParsing.parseMetaVars(value)

    #expect(flattened == [AstGrepTranscriptParsing.MetaVar(name: "OK", index: nil, text: "kept", textOffset: nil)])
}

@Test func parseMetaVarsReturnsEmptyForAbsentOrShapelessInput() {
    #expect(AstGrepTranscriptParsing.parseMetaVars(nil).isEmpty)
    #expect(AstGrepTranscriptParsing.parseMetaVars(.object([:])).isEmpty)
    #expect(AstGrepTranscriptParsing.parseMetaVars(.string("nope")).isEmpty)
}

// MARK: - `matchCountLabel` (moved, `fileCount` now required on `StructuredResult`)

@Test func matchCountLabelRendersTheWebsShowingFirstNOfMShapeWhenTruncated() {
    let structured = AstGrepTranscriptParsing.StructuredResult(
        matches: [
            AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil),
            AstGrepTranscriptParsing.Match(file: "b.rs", startLine: 2, startCol: 1, text: nil),
        ],
        matchCount: 250,
        fileCount: 2,
        truncated: true
    )

    let label = AstGrepTranscriptParsing.matchCountLabel(structured: structured, matches: structured.matches)

    #expect(label == "showing first 2 of 250 matches")
}

@Test func matchCountLabelIsAPlainCountWhenNotTruncated() {
    let structured = AstGrepTranscriptParsing.StructuredResult(
        matches: [AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil)],
        matchCount: 1,
        fileCount: 1,
        truncated: false
    )

    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: structured, matches: structured.matches) == "1 match")
    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: nil, matches: structured.matches) == "1 match")

    let two = [
        AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil),
        AstGrepTranscriptParsing.Match(file: "b.rs", startLine: 2, startCol: 1, text: nil),
    ]
    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: nil, matches: two) == "2 matches", "the text-parsed fallback (structured == nil) carries no truncation signal — always a plain count")
}

// MARK: - `fromText` (moved, unchanged — a text-parsed match always carries
// `metaVars == []` via `Match`'s default)

@Test func fromTextParsesTheCompactPathLineColFormatAndSkipsUnparseableLines() {
    let output = """
    src/a.rs:12:3: fn foo() {
    this line has no match at all
    src/b.rs:1:1: use bar;
    """

    let matches = AstGrepTranscriptParsing.fromText(output)

    #expect(matches == [
        AstGrepTranscriptParsing.Match(file: "src/a.rs", startLine: 12, startCol: 3, text: "fn foo() {"),
        AstGrepTranscriptParsing.Match(file: "src/b.rs", startLine: 1, startCol: 1, text: "use bar;"),
    ])
    #expect(matches.allSatisfy { $0.metaVars.isEmpty })
}

@Test func fromTextReturnsEmptyForBlankOutput() {
    #expect(AstGrepTranscriptParsing.fromText("").isEmpty)
}

// MARK: - `highlightedAstGrepMatch` (new, Task 6): codepoint-slicing correctness

@Test func highlightedAstGrepMatchTintsExactlyTheSpanBoundsForAnAsciiMatch() {
    let text = "fn process(x: i32) -> i32 {\n    x + 1\n}"
    let metaVars = [
        AstGrepTranscriptParsing.MetaVar(name: "NAME", index: nil, text: "process", textOffset: .init(start: 3, end: 10)),
    ]

    let attributed = highlightedAstGrepMatch(text: text, metaVars: metaVars)

    #expect(String(attributed.characters) == text, "highlighting must never drop or reorder any text")
    var highlighted = ""
    for run in attributed.runs where run.backgroundColor != nil {
        highlighted += String(attributed[run.range].characters)
    }
    #expect(highlighted == "process")
}

@Test func highlightedAstGrepMatchOmitsHighlightingForABindingWithNoTextOffset() {
    let text = "fn foo()"
    let metaVars = [
        AstGrepTranscriptParsing.MetaVar(name: "NAME", index: nil, text: "foo", textOffset: nil),
    ]

    let attributed = highlightedAstGrepMatch(text: text, metaVars: metaVars)

    #expect(String(attributed.characters) == text)
    for run in attributed.runs {
        #expect(run.backgroundColor == nil, "a binding with no textOffset must never participate in snippet highlighting")
    }
}

/// **Controller ruling #3**: a non-ASCII regression case. `"e\u{0301}"` is a
/// DECOMPOSED "é" — base `e` (U+0065) plus a combining acute accent
/// (U+0301) — ONE Swift grapheme cluster (`String.count` sees it as a
/// single `Character`) but TWO Unicode scalars. Followed by `"fn foo(x) {}"`
/// (12 more scalars, 12 more graphemes — all ASCII, no further divergence),
/// the full string is 13 graphemes / 14 Unicode scalars. `foo`'s wire
/// `textOffset` (a Rust CHAR/scalar count) is `{start: 5, end: 8}` — scalar
/// indices `5,6,7`. A slice built by walking Swift's grapheme-based
/// `String.Index`/`Array(text)` at the SAME numeric bounds would instead
/// land on grapheme indices `5,6,7`, which are `'o','o','('` — i.e. the
/// substring `"oo("`, not `"foo"`. This asserts the CORRECT (codepoint)
/// answer, proving the implementation walks `unicodeScalars`, not graphemes.
@Test func highlightedAstGrepMatchSlicesByCodepointNotGraphemeForANonAsciiPrefix() {
    let text = "e\u{0301}fn foo(x) {}"
    // Sanity on the setup itself — this really does diverge.
    #expect(text.count == 13, "13 Swift graphemes: the decomposed é collapses to one Character")
    #expect(Array(text.unicodeScalars).count == 14, "14 Unicode scalars: e + combining-acute + fn foo(x) {}")

    let metaVars = [
        AstGrepTranscriptParsing.MetaVar(name: "NAME", index: nil, text: "foo", textOffset: .init(start: 5, end: 8)),
    ]

    let attributed = highlightedAstGrepMatch(text: text, metaVars: metaVars)

    #expect(String(attributed.characters) == text, "highlighting must never drop or reorder any text, even with a decomposed accent in it")

    var highlighted = ""
    for run in attributed.runs where run.backgroundColor != nil {
        highlighted += String(attributed[run.range].characters)
    }
    #expect(highlighted == "foo", "textOffset is a Rust char (Unicode-scalar) count — slicing must walk unicodeScalars; a grapheme-based slice at the same numeric bounds would wrongly yield \"oo(\"")
}
