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

    /// Highlights `code` as `language` (nil auto-detects) using the theme closest to the web
    /// app's `codeHighlight.css` pairing (`atom-one-light` / `atom-one-dark`, both bundled by
    /// HighlighterSwift verbatim — no substitution needed). `toml` is remapped to `ini` first:
    /// highlight.js/HighlighterSwift ship no TOML grammar, and TOML's `key = value` /
    /// `[section]` shape reads well enough under the INI grammar.
    ///
    /// Never throws and never crashes: a failed `Highlighter` init, an unrecognized language,
    /// or any other failure all fall back to plain mono-styled text instead.
    public static func highlight(_ code: String, language: String?, dark: Bool) -> AttributedString {
        guard let highlighter = shared else {
            return fallback(code)
        }

        let themeName = dark ? "atom-one-dark" : "atom-one-light"
        guard highlighter.setTheme(themeName) else {
            return fallback(code)
        }

        let mappedLanguage = language == "toml" ? "ini" : language
        guard let rendered = highlighter.highlight(code, as: mappedLanguage) else {
            return fallback(code)
        }

        return convert(rendered)
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
