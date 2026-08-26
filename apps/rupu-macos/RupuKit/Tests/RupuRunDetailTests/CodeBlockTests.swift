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
