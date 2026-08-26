import AppKit
import Highlighter
import RupuDesign
import SwiftUI

/// Thin, crash-safe facade over HighlighterSwift's `Highlighter` (a Swift wrapper around a
/// bundled `highlight.js`, evaluated in a `JSContext`).
///
/// Concurrency: `Highlighter` is not `Sendable` — it owns a `JSContext` and mutable `theme`
/// state — and its docs make no thread-safety claim. Rather than add locking for an unproven
/// concurrent-access need, this facade is `@MainActor`-confined: SwiftUI already calls into it
/// from view `body`/task code that runs on the main actor, so isolation here is free and
/// correct without extra machinery.
@MainActor
public enum CodeHighlighter {
    /// One shared `Highlighter` instance, reused across calls and themes. `setTheme(_:)` is
    /// cheap (parses one bundled CSS file) and safe to call again before every highlight, so a
    /// single instance suffices rather than a per-theme cache.
    private static let shared: Highlighter? = Highlighter()

    // -------------------------------------------------------------------
    // Memo cache — promoted from a deferred nice-to-have to load-bearing
    // once the live transcript feed shipped (Task 7): the expanded LAST
    // turn re-runs `body` on every SwiftUI diff pass while a run is
    // streaming, and `SourcePreview` calls `highlightedLineText` once PER
    // rendered source line — both re-ran a full highlight.js pass through
    // the shared `JSContext` for byte-identical `(code, language, dark)`
    // input on every one of those re-evaluations. Keyed on the exact
    // triple; a plain dict with an LRU eviction order (capped at
    // `cacheCapacity`) rather than `NSCache` — `NSCache` needs class-typed
    // keys/values and `AttributedString` is a value type, so caching it
    // there would need a wrapper box anyway, and this type is already
    // `@MainActor`-confined, so a plain dict needs no locking either.
    // -------------------------------------------------------------------

    /// Perf & interaction arc, Plan 5 Task 3: keyed on a cheap `Hasher`-based
    /// digest of `(code, language, dark)`, not the full `code` string —
    /// hashing (and therefore every dictionary lookup) no longer costs
    /// O(code.length) per call. `code` is still carried on the key as a
    /// collision guard: `==` re-confirms the full string matches before
    /// trusting a hit, so two different code blocks that happen to digest
    /// to the same value can never silently serve each other's rendering.
    private struct CacheKey: Hashable {
        let digest: Int
        let language: String?
        let dark: Bool
        let code: String

        init(code: String, language: String?, dark: Bool) {
            self.code = code
            self.language = language
            self.dark = dark
            var hasher = Hasher()
            hasher.combine(code)
            hasher.combine(language)
            hasher.combine(dark)
            self.digest = hasher.finalize()
        }

        static func == (lhs: CacheKey, rhs: CacheKey) -> Bool {
            lhs.digest == rhs.digest && lhs.dark == rhs.dark && lhs.language == rhs.language && lhs.code == rhs.code
        }

        func hash(into hasher: inout Hasher) {
            hasher.combine(digest)
        }
    }

    /// One cached rendering plus the tick it was last touched on — the
    /// generation-stamped replacement (Plan 5 Task 3) for the old
    /// oldest-first `cacheOrder` array, whose "touch" (`firstIndex(of:)` +
    /// `remove` + `append`) scanned up to `cacheCapacity` entries on EVERY
    /// cache hit. `lastUsed` makes a touch an O(1) dictionary write instead;
    /// only eviction (a genuinely new key landing at capacity, not every
    /// hit) still scans the whole cache for the minimum.
    private struct CacheEntry {
        let value: AttributedString
        var lastUsed: UInt64
    }

    private static let cacheCapacity = 200
    private static var cache: [CacheKey: CacheEntry] = [:]

    /// Monotonic tick, bumped on every touch/insert — the LRU ordering
    /// `evictLRU()` reads back via each entry's `lastUsed`.
    private static var cacheTick: UInt64 = 0

    private static func nextTick() -> UInt64 {
        cacheTick += 1
        return cacheTick
    }

    /// Bumped on every cache MISS only (never on a hit) — the pure counter
    /// `CodeHighlighterCacheTests` asserts against to prove a repeated call
    /// actually short-circuits the highlight.js pass rather than merely
    /// happening to return an equal-looking result. Default (internal)
    /// access so `@testable import RupuRunDetail` can read it.
    static var highlightCallCount = 0

    /// The currently-applied `Highlighter` theme name, or `nil` before the
    /// first successful `setTheme` call — `compute(_:language:dark:)` skips
    /// re-calling `setTheme` when the requested theme already matches this
    /// (Plan 5 Task 3): a burst of distinct code blocks in the SAME
    /// light/dark mode used to re-parse the identical bundled theme CSS on
    /// every one of them, even though the cache above already stops
    /// re-highlighting identical CODE.
    private static var currentThemeName: String?

    /// Test-only reset — clears the cache, miss counter, and remembered
    /// theme to a clean slate. This type's cache is process-wide static
    /// state, so a test that wants a deterministic hit/miss count must
    /// reset first regardless of what earlier tests (or earlier
    /// `CodeBlock`/`SourcePreview` renders in the same process) already
    /// populated.
    static func resetCacheForTesting() {
        cache.removeAll()
        cacheTick = 0
        highlightCallCount = 0
        currentThemeName = nil
    }

    /// Highlights `code` as `language` (nil auto-detects) using the theme closest to the web
    /// app's `codeHighlight.css` pairing (`atom-one-light` / `atom-one-dark`, both bundled by
    /// HighlighterSwift verbatim — no substitution needed). `toml` is remapped to `ini` first:
    /// highlight.js/HighlighterSwift ship no TOML grammar, and TOML's `key = value` /
    /// `[section]` shape reads well enough under the INI grammar.
    ///
    /// Never throws and never crashes: a failed `Highlighter` init, an unrecognized language,
    /// or any other failure all fall back to plain mono-styled text instead.
    public static func highlight(_ code: String, language: String?, dark: Bool) -> AttributedString {
        let key = CacheKey(code: code, language: language, dark: dark)
        if var entry = cache[key] {
            entry.lastUsed = nextTick()
            cache[key] = entry
            return entry.value
        }

        highlightCallCount += 1
        let result = compute(code, language: language, dark: dark)
        store(key, result)
        return result
    }

    private static func compute(_ code: String, language: String?, dark: Bool) -> AttributedString {
        guard let highlighter = shared else {
            return fallback(code)
        }

        let themeName = dark ? "atom-one-dark" : "atom-one-light"
        if currentThemeName != themeName {
            guard highlighter.setTheme(themeName) else {
                return fallback(code)
            }
            currentThemeName = themeName
        }

        let mappedLanguage = language == "toml" ? "ini" : language
        guard let rendered = highlighter.highlight(code, as: mappedLanguage) else {
            return fallback(code)
        }

        return convert(rendered)
    }

    /// Inserts a brand-new key (the caller already confirmed a miss),
    /// evicting the least-recently-used entry first if the cache is
    /// already at capacity.
    private static func store(_ key: CacheKey, _ value: AttributedString) {
        if cache.count >= cacheCapacity {
            evictLRU()
        }
        cache[key] = CacheEntry(value: value, lastUsed: nextTick())
    }

    /// O(cache size) — but only ever runs on a genuinely new key landing at
    /// capacity, not on every touch (that's the whole point of the
    /// `lastUsed` tick above: the HOT path — a repeated hit — stays O(1)).
    private static func evictLRU() {
        guard let oldestKey = cache.min(by: { $0.value.lastUsed < $1.value.lastUsed })?.key else { return }
        cache.removeValue(forKey: oldestKey)
    }

    /// Rebuilds `rendered` as an `AttributedString` carrying only SwiftUI-scoped `foregroundColor`
    /// attributes (per run, where the theme set an `NSColor`).
    ///
    /// `NSAttributedString(...) -> AttributedString` bridging lands attributes in the AppKit
    /// attribute scope, not the SwiftUI one — `Text` and `AttributedString.foregroundColor`/
    /// `.font` resolve dynamic-member access against the SwiftUI scope whenever SwiftUI is
    /// imported (as it always is here), so a bridged-only string silently reads back with no
    /// colors or font. Explicitly re-homing the color into the SwiftUI scope, run by run, avoids
    /// that cross-scope ambiguity entirely. The theme's background color (page background, not
    /// wanted — `CodeBlock` paints `Color.rupuSurface` itself) and font (Highlighter defaults to
    /// Courier) are both intentionally dropped rather than carried over: no font attribute is
    /// set here at all, so `CodeBlock`'s own `.font(.dataMono(11.5))` view modifier is always
    /// what wins.
    private static func convert(_ rendered: NSAttributedString) -> AttributedString {
        var result = AttributedString()
        let fullRange = NSRange(location: 0, length: rendered.length)
        rendered.enumerateAttributes(in: fullRange) { attrs, range, _ in
            let substring = (rendered.string as NSString).substring(with: range)
            var piece = AttributedString(substring)
            if let nsColor = attrs[.foregroundColor] as? NSColor {
                piece.foregroundColor = Color(nsColor: nsColor)
            }
            result += piece
        }
        return result
    }

    /// Plain, unstyled mono text — used when `Highlighter` fails to initialize (missing/bad
    /// bundle resources) or a language name isn't recognized by highlight.js.
    private static func fallback(_ code: String) -> AttributedString {
        var attributed = AttributedString(code)
        attributed.font = Font.system(size: 11.5, design: .monospaced).monospacedDigit()
        return attributed
    }
}

/// A syntax-highlighted, horizontally scrollable code block: `rupuSurface` fill, 6pt corner
/// radius, selectable text. Re-highlights whenever the effective color scheme changes (light vs
/// dark drives which highlight.js theme `CodeHighlighter` applies).
public struct CodeBlock: View {
    private let code: String
    private let language: String?

    @Environment(\.colorScheme) private var colorScheme

    public init(_ code: String, language: String?) {
        self.code = code
        self.language = language
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: true) {
            Text(CodeHighlighter.highlight(code, language: language, dark: colorScheme == .dark))
                .font(.dataMono(11.5))
                .textSelection(.enabled)
                .padding(10)
                .fixedSize(horizontal: true, vertical: false)
        }
        .background(Color.rupuSurface)
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
