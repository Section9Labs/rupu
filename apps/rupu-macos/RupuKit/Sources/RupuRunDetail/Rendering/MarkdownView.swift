import RupuDesign
import SwiftUI

/// One parsed block of a markdown document — the unit `parseMarkdownBlocks` emits and
/// `MarkdownView` renders. Deliberately line/block-granular only: inline formatting (bold,
/// italic, links, inline code) is NOT decomposed here — `paragraph`/`listItem`/`quote` carry raw
/// text that `MarkdownView` hands to `AttributedString(markdown:options:)` at render time (spec
/// §5, "Transcript").
public enum MarkdownBlock: Equatable, Sendable {
    case heading(level: Int, text: String)
    case paragraph(text: String) // inline md parsed by AttributedString at render
    case listItem(indent: Int, ordered: Bool, marker: String, text: String)
    case quote(text: String)
    case fence(language: String?, code: String)
    case rule
    case table(raw: String) // honest fallback: mono block (spec §5)
}

/// Pure, line-oriented markdown block splitter — no I/O, no SwiftUI/AppKit dependency. Handles
/// the subset the spec calls for (headings, paragraphs with soft-wrap joining, lists, quotes,
/// fences, thematic rules, and a GFM-table honest fallback); anything else falls through to
/// `paragraph`.
///
/// Line classification order matters: fences are checked first (their body lines must NOT be
/// reinterpreted as headings/quotes/etc while inside the fence), then rule, heading, table run,
/// quote run, list item, and finally paragraph (which itself runs until a blank line or a line
/// that starts a different block type).
public func parseMarkdownBlocks(_ source: String) -> [MarkdownBlock] {
    let lines = source.components(separatedBy: "\n")
    var blocks: [MarkdownBlock] = []
    var index = 0

    while index < lines.count {
        let line = lines[index]
        let trimmed = line.trimmingCharacters(in: .whitespaces)

        if trimmed.isEmpty {
            index += 1
            continue
        }

        if let language = fenceLanguage(trimmed) {
            var bodyLines: [String] = []
            index += 1
            while index < lines.count, !isFenceDelimiter(lines[index]) {
                bodyLines.append(lines[index])
                index += 1
            }
            // If we stopped on a closing delimiter, consume it; if we ran off the end of the
            // input (unterminated fence), there's nothing left to consume.
            if index < lines.count {
                index += 1
            }
            blocks.append(.fence(language: language, code: bodyLines.joined(separator: "\n")))
            continue
        }

        if isRule(trimmed) {
            blocks.append(.rule)
            index += 1
            continue
        }

        if let (level, text) = headingComponents(trimmed) {
            blocks.append(.heading(level: level, text: text))
            index += 1
            continue
        }

        if isTableLine(trimmed) {
            var tableLines: [String] = []
            while index < lines.count, isTableLine(lines[index].trimmingCharacters(in: .whitespaces)) {
                tableLines.append(lines[index].trimmingCharacters(in: .whitespaces))
                index += 1
            }
            blocks.append(.table(raw: tableLines.joined(separator: "\n")))
            continue
        }

        if isQuoteLine(trimmed) {
            var quoteParts: [String] = []
            while index < lines.count, isQuoteLine(lines[index].trimmingCharacters(in: .whitespaces)) {
                quoteParts.append(quoteContent(lines[index].trimmingCharacters(in: .whitespaces)))
                index += 1
            }
            blocks.append(.quote(text: quoteParts.joined(separator: " ")))
            continue
        }

        if let item = listItemComponents(line) {
            blocks.append(.listItem(indent: item.indent, ordered: item.ordered, marker: item.marker, text: item.text))
            index += 1
            continue
        }

        // Paragraph: soft-wrapped lines join with a space until a blank line, or a line that
        // starts one of the other block types above.
        var paragraphLines = [trimmed]
        index += 1
        while index < lines.count {
            let nextTrimmed = lines[index].trimmingCharacters(in: .whitespaces)
            if nextTrimmed.isEmpty { break }
            if fenceLanguage(nextTrimmed) != nil { break }
            if isRule(nextTrimmed) { break }
            if headingComponents(nextTrimmed) != nil { break }
            if isTableLine(nextTrimmed) { break }
            if isQuoteLine(nextTrimmed) { break }
            if listItemComponents(lines[index]) != nil { break }
            paragraphLines.append(nextTrimmed)
            index += 1
        }
        blocks.append(.paragraph(text: paragraphLines.joined(separator: " ")))
    }

    return blocks
}

// MARK: - Line classifiers

private func isFenceDelimiter(_ line: String) -> Bool {
    line.trimmingCharacters(in: .whitespaces).hasPrefix("```")
}

/// Returns the fence's language tag (nil if untagged) when `trimmed` opens a fence, else nil.
private func fenceLanguage(_ trimmed: String) -> String?? {
    guard trimmed.hasPrefix("```") else { return nil }
    let tag = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
    return .some(tag.isEmpty ? nil : tag)
}

private func isRule(_ trimmed: String) -> Bool {
    for marker in ["-", "*", "_"] {
        let stripped = trimmed.replacingOccurrences(of: " ", with: "")
        if stripped.count >= 3, stripped.allSatisfy({ $0 == Character(marker) }) {
            return true
        }
    }
    return false
}

/// CommonMark ATX headings: 1-6 `#` characters, then a required space, then the text.
private func headingComponents(_ trimmed: String) -> (level: Int, text: String)? {
    var level = 0
    var chars = Substring(trimmed)
    while chars.first == "#", level < 6 {
        level += 1
        chars = chars.dropFirst()
    }
    guard level > 0, chars.first == " " else { return nil }
    let text = chars.trimmingCharacters(in: .whitespaces)
    return (level, text)
}

private func isTableLine(_ trimmed: String) -> Bool {
    trimmed.hasPrefix("|")
}

private func isQuoteLine(_ trimmed: String) -> Bool {
    trimmed.hasPrefix(">")
}

private func quoteContent(_ trimmed: String) -> String {
    var rest = trimmed.dropFirst()
    if rest.first == " " { rest = rest.dropFirst() }
    return String(rest)
}

/// Recognizes `-`/`*`/`+` (unordered) and `<digits>.` (ordered) markers, each followed by a
/// required space. Indent level is derived from leading-whitespace width: every 2 spaces (or one
/// tab) is one indent level, matching the nested-list fixture's two-space convention.
private func listItemComponents(_ line: String) -> (indent: Int, ordered: Bool, marker: String, text: String)? {
    var leading = 0
    var chars = Substring(line)
    while let first = chars.first, first == " " || first == "\t" {
        leading += first == "\t" ? 2 : 1
        chars = chars.dropFirst()
    }
    let indent = leading / 2

    if let first = chars.first, first == "-" || first == "*" || first == "+" {
        let afterMarker = chars.dropFirst()
        guard afterMarker.first == " " else { return nil }
        let text = afterMarker.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return (indent, false, String(first), text)
    }

    let digits = chars.prefix { $0.isNumber }
    if !digits.isEmpty {
        let afterDigits = chars.dropFirst(digits.count)
        guard afterDigits.first == "." else { return nil }
        let afterDot = afterDigits.dropFirst()
        guard afterDot.first == " " else { return nil }
        let text = afterDot.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return (indent, true, "\(digits).", text)
    }

    return nil
}

// MARK: - Rendering

/// One cached parse result — a class (not a struct) because `NSCache`
/// requires a class-typed `ObjectType`. Holds the exact `source` string
/// alongside the parsed `blocks` as a collision guard: `MarkdownBlockCache`
/// keys by a cheap `Hasher`-based digest of `source`, not the full string
/// (perf & interaction arc, Plan 5 Task 3), so a hash collision between two
/// different sources must never silently serve the wrong entry's blocks —
/// every lookup re-confirms the full string matches before trusting a hit.
private final class CachedMarkdownBlocks {
    let source: String
    let blocks: [MarkdownBlock]

    init(source: String, blocks: [MarkdownBlock]) {
        self.source = source
        self.blocks = blocks
    }
}

/// Memoizes `parseMarkdownBlocks(_:)` by source text (perf & interaction arc,
/// Plan 5 Task 3): `MarkdownView.init` used to re-parse its `source` on
/// EVERY init — and a `Turn`'s `assistantText`/`thinking` never changes once
/// a turn has landed, so the live transcript feed's expanded last turn was
/// re-parsing byte-identical markdown on every single re-render while a run
/// streamed. `NSCache` (not a plain dict, matching `CodeHighlighter`'s own
/// `@MainActor`-confined choice of a plain dict for ITS cache — see that
/// type's doc comment): this cache is reachable from any `MarkdownView`
/// init, which is not actor-isolated at all (a `View`'s `init` runs
/// wherever SwiftUI happens to construct it), so an actual cross-thread-safe
/// cache is needed here, and `NSCache` already provides that plus automatic
/// eviction under memory pressure for free.
enum MarkdownBlockCache {
    /// `nonisolated(unsafe)`: `NSCache` isn't `Sendable` in the type system,
    /// but it IS documented thread-safe internally (Apple's own docs: "you
    /// can add, remove, and query items in the cache from different threads
    /// without having to lock the cache yourself") — no external lock is
    /// needed the way `RenderMeter`'s plain `[String: Int]` dict required
    /// one. `blocks(for:)` is called from `MarkdownView.init`, which runs on
    /// whatever thread SwiftUI happens to construct that view on.
    nonisolated(unsafe) private static let cache = NSCache<NSNumber, CachedMarkdownBlocks>()

    /// Guards `parseCallCount` below — plain `NSLock`, same rationale as
    /// `RenderMeter.lock`: `blocks(for:)` isn't actor-isolated, so a counter
    /// bump needs its own synchronization independent of `NSCache`'s.
    private static let countLock = NSLock()

    /// Bumped on every actual `parseMarkdownBlocks` call (a cache MISS),
    /// never on a hit — the pure counter `MarkdownBlockCacheTests` asserts
    /// against, mirroring `CodeHighlighter.highlightCallCount`'s exact
    /// rationale: proves a repeated call for the same source text actually
    /// short-circuits the parse rather than merely happening to return an
    /// equal-looking result. Default (internal) access so `@testable import
    /// RupuRunDetail` can read it.
    nonisolated(unsafe) static var parseCallCount = 0

    static func blocks(for source: String) -> [MarkdownBlock] {
        var hasher = Hasher()
        hasher.combine(source)
        let key = NSNumber(value: hasher.finalize())

        if let cached = cache.object(forKey: key), cached.source == source {
            return cached.blocks
        }

        countLock.lock()
        parseCallCount += 1
        countLock.unlock()
        let parsed = parseMarkdownBlocks(source)
        cache.setObject(CachedMarkdownBlocks(source: source, blocks: parsed), forKey: key)
        return parsed
    }

    /// Test-only reset (default access — reached via `@testable import
    /// RupuRunDetail`): clears the process-wide cache and miss counter to a
    /// clean slate.
    static func resetForTesting() {
        cache.removeAllObjects()
        countLock.lock()
        parseCallCount = 0
        countLock.unlock()
    }
}

/// Renders a parsed markdown document per the V2-CONTRACT "Transcript" spec: prose is sans at
/// `uiText` scale; inline code is `dataMono` on `rupuSurface`; quotes get a 2px `rupuBorder` left
/// bar with `rupuDim` text; fenced code goes through Task 1's `CodeBlock` (syntax highlighting);
/// GFM tables fall back to a `dataMono(11)` block (honest — no table layout this plan); headings
/// scale by level; rules render as a hairline divider; list items show their marker plus
/// inline-parsed text, indented per nesting level.
///
/// Heading scale (3 steps, spec's "pick a consistent 3-step scale and document it") — built
/// entirely from existing named tokens (`Typography.swift`'s doc comment asks for a new named
/// case rather than an ad hoc `.system(size:)`, so no bespoke size is introduced here):
/// - level 1: `subheadText` (14pt, semibold) — the "leadText scaled up one step" token, and the
///   largest heading this view renders.
/// - level 2: `leadText` (13pt), semibold.
/// - level 3+: `uiText` (12pt), semibold — every deeper level (4-6) collapses to this floor
///   rather than continuing to shrink, since transcript markdown rarely nests past h3 and a
///   4th distinct size would undercut `metaText`'s legibility floor.
public struct MarkdownView: View {
    private let blocks: [MarkdownBlock]

    public init(_ source: String) {
        self.blocks = MarkdownBlockCache.blocks(for: source)
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                MarkdownBlockView(block: block)
            }
        }
        .textSelection(.enabled)
    }
}

private struct MarkdownBlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case let .heading(level, text):
            InlineText(text)
                .font(headingFont(level))
                .foregroundStyle(Color.rupuInk)

        case let .paragraph(text):
            InlineText(text)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)

        case let .listItem(indent, ordered, marker, text):
            HStack(alignment: .top, spacing: 6) {
                Text(ordered ? marker : "\u{2022}")
                    .font(.uiText)
                    .foregroundStyle(Color.rupuDim)
                InlineText(text)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuInk)
            }
            .padding(.leading, CGFloat(indent) * 16)

        case let .quote(text):
            HStack(spacing: 8) {
                Rectangle()
                    .fill(Color.rupuBorder)
                    .frame(width: 2)
                InlineText(text)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuDim)
            }

        case let .fence(language, code):
            CodeBlock(code, language: language)

        case .rule:
            Rectangle()
                .fill(Color.rupuBorder)
                .frame(height: 1)

        case let .table(raw):
            Text(raw)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 6))
        }
    }

    private func headingFont(_ level: Int) -> Font {
        switch level {
        case 1: .subheadText
        case 2: .leadText.weight(.semibold)
        default: .uiText.weight(.semibold)
        }
    }
}

/// Parses `raw` markdown's INLINE formatting only (bold/italic/links/inline code) via
/// `AttributedString(markdown:options:)` with `.inlineOnlyPreservingWhitespace` — block-level
/// syntax (headings, lists, quotes, fences) was already stripped out by `parseMarkdownBlocks`,
/// so this only ever needs to resolve emphasis/links/code spans within one block's text.
///
/// Post-processes the parsed result to apply this app's own presentation on top of the parser's
/// semantic intents: inline-code runs (`inlinePresentationIntent` contains `.code`) get
/// `dataMono` + a `rupuSurface` background; link runs get the brand color. Falls back to plain
/// text (no crash) if the markdown fails to parse — untrusted/malformed transcript content
/// should never take the view down.
///
/// Not `private` (default internal access) so `MarkdownInlineStylingTests` can assert on the
/// returned `AttributedString`'s runs directly via `@testable import`, rather than only at the
/// view level.
func styledInlineMarkdown(_ raw: String) -> AttributedString {
    let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    guard var parsed = try? AttributedString(markdown: raw, options: options) else {
        return AttributedString(raw)
    }

    for run in parsed.runs {
        let range = run.range
        if let intent = run.inlinePresentationIntent, intent.contains(.code) {
            parsed[range].font = .dataMono(12)
            parsed[range].backgroundColor = .rupuSurface
        }
        if run.link != nil {
            parsed[range].foregroundColor = .rupuBrand
        }
    }

    return parsed
}

private struct InlineText: View {
    private let attributed: AttributedString

    init(_ raw: String) {
        self.attributed = styledInlineMarkdown(raw)
    }

    var body: some View {
        Text(attributed)
    }
}
