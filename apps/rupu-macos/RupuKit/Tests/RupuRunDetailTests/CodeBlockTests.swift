import AppKit
import Foundation
import Testing
@testable import RupuRunDetail

/// Collects the distinct `.foregroundColor` values across an `AttributedString`'s runs. Colors
/// bridge in from HighlighterSwift's NSAttributedString output as `NSColor`/`CGColor` — compare
/// on the underlying sRGB components (via `NSColor`) rather than object identity, since distinct
/// `NSColor` instances can represent the same color.
private func distinctForegroundColors(_ attributed: AttributedString) -> Set<String> {
    var colors = Set<String>()
    for run in attributed.runs {
        if let color = run.foregroundColor {
            colors.insert(String(describing: color))
        } else {
            colors.insert("<none>")
        }
    }
    return colors
}

@Test @MainActor func highlightSwiftCodeProducesMultipleDistinctForegroundColors() {
    let attributed = CodeHighlighter.highlight("let x = 1", language: "swift", dark: false)
    let colors = distinctForegroundColors(attributed)
    #expect(colors.count > 1)
}

@Test @MainActor func highlightUnknownLanguageFallsBackToUnstyledTextWithoutCrashing() {
    let attributed = CodeHighlighter.highlight("let x = 1", language: "not-a-real-language", dark: false)
    let colors = distinctForegroundColors(attributed)
    #expect(colors.count == 1)
    #expect(String(attributed.characters) == "let x = 1")
}

@Test @MainActor func highlightTomlRequestDoesNotThrow() {
    let code = "[table]\nkey = \"value\"\n"
    let attributed = CodeHighlighter.highlight(code, language: "toml", dark: false)
    #expect(String(attributed.characters) == code)
}

@Test @MainActor func highlightDarkThemeAlsoProducesMultipleDistinctForegroundColors() {
    let attributed = CodeHighlighter.highlight("let x = 1", language: "swift", dark: true)
    let colors = distinctForegroundColors(attributed)
    #expect(colors.count > 1)
}

@Test @MainActor func highlightNeverLeavesABackgroundColorForCodeBlockToClashWith() {
    let attributed = CodeHighlighter.highlight("let x = 1", language: "swift", dark: false)
    for run in attributed.runs {
        #expect(run.backgroundColor == nil)
    }
}

// MARK: - Memo cache (fix round 2, finding 4: promoted from deferred once
// the live transcript feed made repeated re-highlighting of identical input
// load-bearing — the expanded last turn re-renders on every event, and
// `SourcePreview` calls this once per source line).

@Test @MainActor func repeatedIdenticalCallsReturnEqualResultsAndOnlyMissTheCacheOnce() {
    CodeHighlighter.resetCacheForTesting()

    let first = CodeHighlighter.highlight("let x = 1", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == 1, "the first call for a new key must be a cache miss")

    let second = CodeHighlighter.highlight("let x = 1", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == 1, "an identical (code, language, dark) call must be a cache hit, not a second miss")

    #expect(first == second, "a cache hit must return the exact same rendering as the original computation")
}

@Test @MainActor func differentCacheKeysEachMissIndependently() {
    CodeHighlighter.resetCacheForTesting()

    _ = CodeHighlighter.highlight("let x = 1", language: "swift", dark: false)
    _ = CodeHighlighter.highlight("let x = 1", language: "swift", dark: true) // different `dark`
    _ = CodeHighlighter.highlight("let y = 2", language: "swift", dark: false) // different `code`
    _ = CodeHighlighter.highlight("let x = 1", language: "rust", dark: false) // different `language`

    #expect(CodeHighlighter.highlightCallCount == 4, "each distinct (code, language, dark) triple must miss independently")
}
