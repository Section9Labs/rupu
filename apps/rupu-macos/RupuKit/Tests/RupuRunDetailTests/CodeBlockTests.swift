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

// MARK: - LRU eviction (perf & interaction arc, Plan 5 Task 3: generation-
// stamped O(1) touch replacing the old oldest-first array scan)

/// Fills the cache to EXACTLY its capacity (200) with distinct entries —
/// no eviction happens yet, since `store(_:_:)` only evicts on the insert
/// that would push the cache OVER capacity, never before.
@MainActor
private func fillCacheToCapacity() {
    for i in 0..<200 {
        _ = CodeHighlighter.highlight("let filler\(i) = \(i)", language: "swift", dark: false)
    }
}

/// Proves the LRU policy actually drives eviction (not insertion order):
/// touching an old entry again (a cache HIT) must protect it from the next
/// eviction, and the entry that was never touched again must be the one
/// that goes instead — regardless of which of the two was inserted first.
@Test @MainActor func lruEvictionKeepsTheMostRecentlyTouchedEntryOverAnUntouchedOne() {
    CodeHighlighter.resetCacheForTesting()
    fillCacheToCapacity()
    #expect(CodeHighlighter.highlightCallCount == 200)

    // Touch "filler0" — a cache HIT (no new miss), which must bump its
    // recency ahead of every other filler entry, including "filler1".
    _ = CodeHighlighter.highlight("let filler0 = 0", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == 200, "the touch must be a hit, not a new miss")

    // One more distinct entry pushes the cache over capacity: this must
    // evict the LEAST recently used entry — "filler1" (never touched again
    // after its initial insert) — never "filler0" (just touched).
    _ = CodeHighlighter.highlight("let trigger = 999", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == 201)

    let missesBeforeFiller0 = CodeHighlighter.highlightCallCount
    _ = CodeHighlighter.highlight("let filler0 = 0", language: "swift", dark: false)
    #expect(
        CodeHighlighter.highlightCallCount == missesBeforeFiller0,
        "filler0 must still be cached — it was the most recently touched entry"
    )

    let missesBeforeFiller1 = CodeHighlighter.highlightCallCount
    _ = CodeHighlighter.highlight("let filler1 = 1", language: "swift", dark: false)
    #expect(
        CodeHighlighter.highlightCallCount == missesBeforeFiller1 + 1,
        "filler1 must have been evicted — it was the least recently used entry"
    )
}

/// The cache never grows past its declared capacity, however many DISTINCT
/// keys pass through it — the generation-stamped eviction path fires on
/// every insert past capacity, not just the first one.
@Test @MainActor func cacheNeverExceedsCapacityAcrossManyDistinctInserts() {
    CodeHighlighter.resetCacheForTesting()
    fillCacheToCapacity()

    // 50 more distinct entries, all past capacity — each must evict
    // something rather than growing the cache unbounded.
    for i in 200..<250 {
        _ = CodeHighlighter.highlight("let filler\(i) = \(i)", language: "swift", dark: false)
    }
    #expect(CodeHighlighter.highlightCallCount == 250)

    // The oldest entries (never touched again) must be long gone —
    // re-requesting one is a fresh miss, not a hit.
    let missesBefore = CodeHighlighter.highlightCallCount
    _ = CodeHighlighter.highlight("let filler0 = 0", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == missesBefore + 1, "filler0 must have been evicted long ago")

    // The most recently inserted entry must still be a hit.
    let missesBeforeRecent = CodeHighlighter.highlightCallCount
    _ = CodeHighlighter.highlight("let filler249 = 249", language: "swift", dark: false)
    #expect(CodeHighlighter.highlightCallCount == missesBeforeRecent, "the most recently inserted entry must still be cached")
}

/// `CacheKey`'s `==` re-confirms the full `(code, language, dark)` triple,
/// not just the cheap `Hasher`-based digest it hashes on — so two DIFFERENT
/// code blocks can never silently serve each other's cached rendering even
/// if their digests happened to collide (a real forced collision isn't
/// reproducible in a unit test — `Hasher` is seeded per-process — so this
/// proves the same guarantee indirectly: every distinct real-world input
/// this suite exercises, across many entries, always renders correctly
/// rather than ever returning a wrong neighbor's cached value).
@Test @MainActor func manyDistinctEntriesNeverCrossContaminateEachOthersRendering() {
    CodeHighlighter.resetCacheForTesting()

    let samples = (0..<50).map { i in (code: "let value\(i) = \(i)", i: i) }
    for sample in samples {
        _ = CodeHighlighter.highlight(sample.code, language: "swift", dark: false)
    }

    for sample in samples {
        let rendered = CodeHighlighter.highlight(sample.code, language: "swift", dark: false)
        #expect(
            String(rendered.characters) == sample.code,
            "entry \(sample.i) must render its OWN code, never a different cached entry's"
        )
    }
}
