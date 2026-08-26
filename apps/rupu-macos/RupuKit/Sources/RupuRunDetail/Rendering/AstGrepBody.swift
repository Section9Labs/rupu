import Foundation
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

/// Pure `ast_grep` match parsing — no view/state dependency, so it's
/// directly `@Test`-able via `@testable import RupuRunDetail` without any
/// SwiftUI rendering. Two sources: `fromStructured` reads `tool_result.
/// structured` (camelCase `{pattern?, lang?, matchCount, fileCount,
/// truncated, matches: [{ file, range?: {startLine,startCol,endLine,endCol},
/// text?, metaVars?: {single?, multi?} }]}` — built as a raw `serde_json::
/// json!` object by the actual wire producer, `crates/rupu-tools/src/
/// ast_grep.rs:299-333` (there is no Rust `AstGrepStructured` struct; the TS
/// interface of the same name the web side decodes into is `crates/rupu-cp/
/// web/src/components/transcript/ToolCard.tsx:177`). `fromText` falls back
/// to a line-by-line `path:line:col: text` parse of the plain output for a
/// run recorded before structured `ast_grep` output existed.
///
/// **Fallback trigger diverges from the web, by omission, not by design.**
/// The web's `asAstGrepStructured` (`ToolCard.tsx`) falls back to
/// `parseAstGrepText` ONLY when `structured` is absent or its `matches`
/// field isn't an array — a present-but-malformed individual match (e.g.
/// missing `range`) still renders as a range-less row, never triggers a
/// wholesale re-parse of `output`. The call sites in `AstGrepBodyView`
/// (below) instead fall back whenever `fromStructured(...)` itself is `nil`
/// — i.e. only when the wire's `matches` field isn't present as an array at
/// all; a present-but-empty (or fully-malformed) `matches` array still
/// renders the structured header with zero file groups, matching the web's
/// own "structured is non-null → always render the structured branch"
/// shape (`ToolCard.tsx:811-855`).
///
/// **Metavar bindings (Task 6).** `Match.metaVars` flattens the wire's
/// `metaVars.{single,multi}` object into one ordered list — see `MetaVar`'s
/// and `parseMetaVars`'s doc comments for the exact flatten/order contract.
/// A text-parsed (`fromText`) match always carries `metaVars: []`: the
/// compact `path:line:col: text` output has no metavar information at all.
enum AstGrepTranscriptParsing {
    /// One flattened metavar binding — `single`+`multi` unified into a
    /// single ordered list (see `parseMetaVars`'s doc comment for the exact
    /// flatten/order contract). `textOffset` is the wire's own `textOffset`
    /// (Rust CHAR/Unicode-scalar offsets into the match's own `text`,
    /// half-open, match-relative — `crates/rupu-tools/src/ast_grep.rs`'s
    /// `text_offset`) — absent whenever the emitter couldn't map the node's
    /// byte range onto a char index. Such a binding still renders in the
    /// bindings grid; it just never participates in snippet highlighting
    /// (only entries WITH a `textOffset` do — `HighlightedMatch`'s own
    /// guard, `ToolCard.tsx:597,601`).
    struct MetaVar: Equatable {
        struct Offset: Equatable {
            let start: Int
            let end: Int
        }

        let name: String
        /// `nil` for a `single` binding; the 0-based position within its
        /// `multi[name]` array otherwise. Carried through so the bindings
        /// grid can render the web's `$name[i]` label for a `multi` entry
        /// while the highlighted-snippet tooltip/label still uses the bare
        /// `$name` — `collectMetaVarSpans` (`ToolCard.tsx:589-605`) never
        /// includes the index in a span's own `name`.
        let index: Int?
        let text: String
        let textOffset: Offset?

        /// The bindings-grid label — `$name` for `single`, `$name[i]` for
        /// `multi` (`MetaVarTable`, `ToolCard.tsx:657-671`).
        var displayName: String {
            if let index { return "$\(name)[\(index)]" }
            return "$\(name)"
        }
    }

    struct Match: Equatable {
        let file: String
        let startLine: Int
        let startCol: Int
        let text: String?
        let metaVars: [MetaVar]

        /// `metaVars` defaults to empty so every pre-Task-6 call site
        /// (`fromText`, and the moved test suite's existing `Match(file:
        /// startLine:startCol:text:)` constructions) keeps compiling
        /// unchanged.
        init(file: String, startLine: Int, startCol: Int, text: String?, metaVars: [MetaVar] = []) {
            self.file = file
            self.startLine = startLine
            self.startCol = startCol
            self.text = text
            self.metaVars = metaVars
        }
    }

    /// `fromStructured`'s full result: the parsed (possibly match-dropping,
    /// see the type doc comment) match list alongside the wire's own
    /// `pattern`/`lang`/`matchCount`/`fileCount`/`truncated` — needed to
    /// render the web's `N matches in M files` header, `pattern`/`lang`
    /// Badges, and honest "showing first N of M" truncation Badge
    /// (`ToolCard.tsx:824-844`) instead of a plain count that would
    /// silently imply `matches.count` IS the total.
    struct StructuredResult: Equatable {
        let matches: [Match]
        /// The wire's own `matchCount` when present; falls back to
        /// `matches.count` for a malformed/absent field rather than reading
        /// as `0` — an honest reading of "we don't know the true total, so
        /// assume it's exactly what we parsed," never a fabricated number.
        let matchCount: Int
        let fileCount: Int
        let truncated: Bool
        let pattern: String?
        let lang: String?

        /// `pattern`/`lang` default to `nil` so the moved test suite's
        /// existing `StructuredResult(matches:matchCount:truncated:)`
        /// constructions (which predate both fields) keep compiling.
        init(matches: [Match], matchCount: Int, fileCount: Int, truncated: Bool, pattern: String? = nil, lang: String? = nil) {
            self.matches = matches
            self.matchCount = matchCount
            self.fileCount = fileCount
            self.truncated = truncated
            self.pattern = pattern
            self.lang = lang
        }
    }

    /// Parses `tool_result.structured`'s `{pattern?, lang?, matchCount,
    /// fileCount, truncated, matches: [{ file, range?: {startLine,startCol,
    /// endLine,endCol}, text?, metaVars?: {single?, multi?} }]}` shape — see
    /// the type doc comment for the exact wire producer and the documented
    /// fallback-trigger divergence from the web parser. `nil` when
    /// `structured` is absent or its `matches` field isn't present as an
    /// array at all (the web's own fallback trigger); a present-but-
    /// empty-after-parsing `matches` list still returns a non-`nil`
    /// `StructuredResult` with an empty `matches` array.
    static func fromStructured(_ value: JSONValue?) -> StructuredResult? {
        guard case .object(let root)? = value, case .array(let rawMatches)? = root["matches"] else { return nil }
        let matches = rawMatches.compactMap { raw -> Match? in
            guard case .object(let m) = raw, case .string(let file)? = m["file"] else { return nil }
            guard case .object(let range)? = m["range"],
                  case .number(let startLine)? = range["startLine"],
                  case .number(let startCol)? = range["startCol"]
            else { return nil }
            let text: String? = {
                if case .string(let t)? = m["text"] { return t }
                return nil
            }()
            return Match(
                file: file, startLine: Int(startLine), startCol: Int(startCol), text: text,
                metaVars: parseMetaVars(m["metaVars"])
            )
        }
        let matchCount: Int = {
            if case .number(let n)? = root["matchCount"] { return Int(n) }
            return matches.count
        }()
        let fileCount: Int = {
            if case .number(let n)? = root["fileCount"] { return Int(n) }
            return Set(matches.map(\.file)).count
        }()
        let truncated: Bool = {
            if case .bool(let t)? = root["truncated"] { return t }
            return false
        }()
        let pattern: String? = {
            if case .string(let p)? = root["pattern"] { return p }
            return nil
        }()
        let lang: String? = {
            if case .string(let l)? = root["lang"] { return l }
            return nil
        }()
        return StructuredResult(matches: matches, matchCount: matchCount, fileCount: fileCount, truncated: truncated, pattern: pattern, lang: lang)
    }

    /// Flattens one match's `metaVars.{single,multi}` wire object (real
    /// shape: `{single: {NAME: {text, textOffset?}}, multi: {NAME: [{text,
    /// textOffset?}]}}`, `crates/rupu-tools/src/ast_grep.rs:299-333`) into a
    /// single ordered list — `single` bindings first, then `multi` bindings.
    /// Both halves are sorted by metavar NAME for determinism: `JSONValue.
    /// object` decodes into a Swift `Dictionary`, which carries no
    /// wire-order guarantee (unlike the web, whose `Object.entries` walks a
    /// JS object built from a `serde_json::Map` — itself a `BTreeMap`
    /// without the `preserve_order` feature, i.e. ALSO alphabetical — so
    /// this sort doesn't diverge from the web's actual observed order).
    /// Each `multi[name]` array's own bindings keep their original,
    /// already-ordered-by-position sequence (arrays, unlike objects, decode
    /// order-preserving). `nil` (empty list) when `value` isn't an object at
    /// all — a match with no `metaVars` key, or a non-`ast_grep` structured
    /// payload shape.
    static func parseMetaVars(_ value: JSONValue?) -> [MetaVar] {
        guard case .object(let root)? = value else { return [] }
        var result: [MetaVar] = []
        if case .object(let single)? = root["single"] {
            for name in single.keys.sorted() {
                if let binding = parseBinding(name: name, index: nil, value: single[name]) {
                    result.append(binding)
                }
            }
        }
        if case .object(let multi)? = root["multi"] {
            for name in multi.keys.sorted() {
                if case .array(let arr)? = multi[name] {
                    for (i, item) in arr.enumerated() {
                        if let binding = parseBinding(name: name, index: i, value: item) {
                            result.append(binding)
                        }
                    }
                }
            }
        }
        return result
    }

    /// Parses one `{text, textOffset?: {start, end}}` binding node. `nil`
    /// when `text` isn't a string present (the web's `asAstGrepBinding`'s own
    /// trigger, `ToolCard.tsx:187-194`) — a binding with no `text` at all is
    /// dropped rather than surfaced with a fabricated empty string.
    private static func parseBinding(name: String, index: Int?, value: JSONValue?) -> MetaVar? {
        guard case .object(let obj)? = value, case .string(let text)? = obj["text"] else { return nil }
        var offset: MetaVar.Offset?
        if case .object(let off)? = obj["textOffset"],
           case .number(let s)? = off["start"],
           case .number(let e)? = off["end"] {
            offset = MetaVar.Offset(start: Int(s), end: Int(e))
        }
        return MetaVar(name: name, index: index, text: text, textOffset: offset)
    }

    /// The matches section's count label — the web's own truthful-under-
    /// truncation shape (`ToolCard.tsx:697-701`, `"showing first N of M
    /// matches"`) whenever `structured` is present AND its own `truncated`
    /// flag is set (its `matches` array is a capped prefix of the server's
    /// real total, `MAX_STRUCTURED_MATCHES = 200` — `crates/rupu-tools/src/
    /// ast_grep.rs:34`); a plain `"N matches"` otherwise — including for the
    /// text-parsed fallback (`structured == nil`), which carries no
    /// truncation signal at all, and for an untruncated structured result
    /// (where `matches.count` already equals `matchCount`, so a plain count
    /// isn't misleading). `matches` is the list actually being RENDERED
    /// (post text-fallback if that's what fired) — always `structured?.
    /// matches` when `structured` is truncated, since truncation only ever
    /// applies to the structured path. Kept for `TranscriptFeed`'s own
    /// (pre-Task-7) flat-row rendering, which still calls this.
    static func matchCountLabel(structured: StructuredResult?, matches: [Match]) -> String {
        if let structured, structured.truncated {
            return "showing first \(structured.matches.count) of \(structured.matchCount) matches"
        }
        let count = matches.count
        return "\(count) match\(count == 1 ? "" : "es")"
    }

    /// Parses the compact `path:line:col: text` line format `ast_grep`'s
    /// plain-text output uses — direct port of the web `parseAstGrepText`'s
    /// regex, minus the per-file grouping this phase's flat row list didn't
    /// need (grouping is done separately, in `AstGrepBodyView`).
    static func fromText(_ output: String) -> [Match] {
        output.split(separator: "\n", omittingEmptySubsequences: true).compactMap { substring -> Match? in
            let line = String(substring)
            guard let match = Self.lineRegex.firstMatch(in: line, range: NSRange(line.startIndex..., in: line)),
                  match.numberOfRanges == 5,
                  let fileRange = Range(match.range(at: 1), in: line),
                  let lineRange = Range(match.range(at: 2), in: line),
                  let colRange = Range(match.range(at: 3), in: line),
                  let textRange = Range(match.range(at: 4), in: line),
                  let lineNumber = Int(line[lineRange]),
                  let colNumber = Int(line[colRange])
            else { return nil }
            return Match(file: String(line[fileRange]), startLine: lineNumber, startCol: colNumber, text: String(line[textRange]))
        }
    }

    private static let lineRegex = try! NSRegularExpression(pattern: #"^(.*?):(\d+):(\d+): (.*)$"#)
}

// ---------------------------------------------------------------------------
// AstGrepBodyView — rich `.astGrep` body (`AstGrepBody`, `ToolCard.tsx:
// 806-874`)
// ---------------------------------------------------------------------------

/// `.astGrep` tool-call body: web-parity rich rendering over
/// `AstGrepTranscriptParsing`'s parsed matches — a header (`N matches in M
/// files` + `pattern`/`lang` Badges + an amber "showing first N of M" badge
/// under truncation), per-file collapsible groups (default open), and per
/// match a toggleable `path:line:col` (source preview) + a `tree` ghost
/// button (AST view), the match's snippet with each metavar span tinted
/// `rupuWarnBg`/`rupuWarn`, and a `$name = text` bindings grid. Falls back to
/// the text-parsed match list (no metavars, no pattern/lang/truncation
/// badges — the plain-text output carries none of that) whenever `tool.
/// structured` doesn't parse at all, exactly mirroring `AstGrepBody`'s own
/// structured-vs-fallback split (`ToolCard.tsx:811-874`). Renders nothing
/// when `entry.errorText` is set — the header-level `ErrorBlockView`
/// (`ToolCardView.body`) already shows the error, matching the web's own
/// `if (tool.error) return null;` (`ToolCard.tsx:812`).
///
/// `sourcePreviewStore` is optional (defaulted `nil`), following the exact
/// convention `TranscriptFeed`'s own `ast_grep` rendering already
/// established (Phase 6B, Task 5): when absent, every match's `source`/
/// `tree` toggle button is simply omitted rather than rendered inert —
/// nothing here invents plumbing (a `CPClient`/`SourcePreviewStore`)
/// `ToolCardView` isn't itself handed a live one for; Task 7's transcript
/// rewrite is the seam that threads a real store through, the same way
/// `ToolCardView` picked up `runID`/`host` in Task 3.
struct AstGrepBodyView: View {
    let entry: ToolEntry
    let runID: String?
    let host: String?
    var sourcePreviewStore: SourcePreviewStore?

    init(entry: ToolEntry, runID: String?, host: String?, sourcePreviewStore: SourcePreviewStore? = nil) {
        self.entry = entry
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
    }

    var body: some View {
        if entry.errorText != nil {
            EmptyView()
        } else if let structured = AstGrepTranscriptParsing.fromStructured(entry.structured) {
            structuredBody(structured)
        } else {
            fallbackBody
        }
    }

    // MARK: - Structured path

    private func structuredBody(_ structured: AstGrepTranscriptParsing.StructuredResult) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            header(structured)
            ForEach(Self.groupByFile(structured.matches)) { group in
                FileGroupView(file: group.file, count: group.matches.count) {
                    ForEach(Array(group.matches.enumerated()), id: \.offset) { _, match in
                        AstGrepMatchRowView(file: group.file, match: match, sourcePreviewStore: sourcePreviewStore)
                    }
                }
            }
        }
    }

    private func header(_ structured: AstGrepTranscriptParsing.StructuredResult) -> some View {
        HStack(spacing: 6) {
            Text("\(structured.matchCount) match\(structured.matchCount == 1 ? "" : "es") in \(structured.fileCount) file\(structured.fileCount == 1 ? "" : "s")")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
            if let pattern = structured.pattern, !pattern.isEmpty {
                Badge(pattern, tone: .rupuMute)
            }
            if let lang = structured.lang, !lang.isEmpty {
                Badge(lang, tone: .rupuMute)
            }
            if structured.truncated {
                Badge("showing first \(structured.matches.count) of \(structured.matchCount)", tone: .rupuWarn)
            }
            Spacer(minLength: 0)
        }
    }

    // MARK: - Fallback (text-parsed) path

    private var fallbackBody: some View {
        let matches = AstGrepTranscriptParsing.fromText(entry.output ?? "")
        let groups = Self.groupByFile(matches)
        return VStack(alignment: .leading, spacing: 6) {
            Text("\(matches.count) match\(matches.count == 1 ? "" : "es") in \(groups.count) file\(groups.count == 1 ? "" : "s")")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
            ForEach(groups) { group in
                FileGroupView(file: group.file, count: group.matches.count) {
                    ForEach(Array(group.matches.enumerated()), id: \.offset) { _, match in
                        AstGrepMatchRowView(file: group.file, match: match, sourcePreviewStore: sourcePreviewStore)
                    }
                }
            }
        }
    }

    // MARK: - File grouping — order-preserving (NOT a `Dictionary`, which
    // has no key-order guarantee), matching `AstGrepBody`'s own `Map`
    // insertion-order grouping (`ToolCard.tsx:817-822`).

    private struct FileGroup: Identifiable {
        let id: String
        var file: String { id }
        let matches: [AstGrepTranscriptParsing.Match]
    }

    private static func groupByFile(_ matches: [AstGrepTranscriptParsing.Match]) -> [FileGroup] {
        var order: [String] = []
        var byFile: [String: [AstGrepTranscriptParsing.Match]] = [:]
        for match in matches {
            if byFile[match.file] == nil { order.append(match.file) }
            byFile[match.file, default: []].append(match)
        }
        return order.map { FileGroup(id: $0, matches: byFile[$0] ?? []) }
    }
}

// ---------------------------------------------------------------------------
// Sub-views
// ---------------------------------------------------------------------------

/// One per-file collapsible group — path + match-count `Badge`, default
/// open (`FileGroup`, `ToolCard.tsx:557-587`).
private struct FileGroupView<Content: View>: View {
    let file: String
    let count: Int
    @ViewBuilder let content: () -> Content

    @State private var expanded = true

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 4) {
                content()
            }
            .padding(.top, 4)
        } label: {
            HStack(spacing: 6) {
                Text(file)
                    .font(.dataMono(10.5))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
                Badge("\(count) match\(count == 1 ? "" : "es")", tone: .rupuMute)
            }
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

/// One match row: a toggleable `path:line:col` (opens `SourcePreview`) + a
/// `tree` ghost button (opens `AstTreeView`), the metavar-highlighted
/// snippet, and the bindings grid (`AstGrepMatchRow`/`AstGrepTextMatchRow`,
/// `ToolCard.tsx:678-804`) — one row type for both the structured and
/// text-parsed paths, the same unification `AstGrepTranscriptParsing.Match`
/// already establishes (a text-parsed match simply carries `metaVars: []`,
/// so its bindings grid renders nothing and its snippet has no highlighted
/// spans). The two toggles use independent `@State` — either, both, or
/// neither can be open at once, matching the web's own two independent
/// `useState`s.
private struct AstGrepMatchRowView: View {
    let file: String
    let match: AstGrepTranscriptParsing.Match
    let sourcePreviewStore: SourcePreviewStore?

    @State private var sourceOpen = false
    @State private var treeOpen = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 10) {
                coordinateLabel
                if sourcePreviewStore != nil {
                    Button {
                        treeOpen.toggle()
                    } label: {
                        Text("tree")
                    }
                    .buttonStyle(.plain)
                    .font(.metaText)
                    .foregroundStyle(treeOpen ? Color.rupuInk : Color.rupuBrand)
                    .accessibilityLabel(treeOpen ? "Hide AST tree" : "Show AST tree")
                }
                Spacer(minLength: 0)
            }
            if let text = match.text, !text.isEmpty {
                Text(highlightedAstGrepMatch(text: text, metaVars: match.metaVars))
                    .font(.dataMono(10.5))
                    .foregroundStyle(Color.rupuInk)
                    .textSelection(.enabled)
            }
            MetaVarGridView(metaVars: match.metaVars)
            if sourceOpen, let sourcePreviewStore {
                SourcePreview(store: sourcePreviewStore, path: file, line: match.startLine)
            }
            if treeOpen, let sourcePreviewStore {
                AstTreeView(store: sourcePreviewStore, path: file, line: match.startLine, col: match.startCol)
            }
        }
        .padding(8)
        .panelStyle(.innerCard)
    }

    @ViewBuilder
    private var coordinateLabel: some View {
        let label = "\(file):\(match.startLine):\(match.startCol)"
        if sourcePreviewStore != nil {
            Button {
                sourceOpen.toggle()
            } label: {
                Text(label)
            }
            .buttonStyle(.plain)
            .font(.dataMono(10.5))
            .foregroundStyle(sourceOpen ? Color.rupuInk : Color.rupuMute)
            .accessibilityLabel(sourceOpen ? "Hide source preview" : "Show source preview")
        } else {
            Text(label)
                .font(.dataMono(10.5))
                .foregroundStyle(Color.rupuMute)
        }
    }
}

/// `$name = text` bindings grid (`MetaVarTable`, `ToolCard.tsx:642-676`) —
/// renders nothing when `metaVars` is empty (a text-parsed match, or a
/// structured match whose pattern bound no metavariables).
private struct MetaVarGridView: View {
    let metaVars: [AstGrepTranscriptParsing.MetaVar]

    var body: some View {
        if !metaVars.isEmpty {
            Grid(alignment: .topLeading, horizontalSpacing: 8, verticalSpacing: 1) {
                ForEach(Array(metaVars.enumerated()), id: \.offset) { _, mv in
                    GridRow {
                        Text(mv.displayName)
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuBrand700)
                        Text(mv.text)
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuDim)
                            .textSelection(.enabled)
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Metavar-highlighted snippet (`HighlightedMatch`, `ToolCard.tsx:607-640`)
// ---------------------------------------------------------------------------

/// Renders `text` with each metavar span whose `textOffset` is present,
/// in-range, and non-overlapping tinted `rupuWarn`/`rupuWarnBg`. Not
/// `private` — `AstGrepModelTests` asserts on the returned `AttributedString`
/// directly (same "testable pure seam" convention `styledInlineMarkdown`
/// establishes in `MarkdownView.swift`), without any SwiftUI rendering.
///
/// **Codepoint slicing, not grapheme slicing.** The wire's `textOffset` is a
/// Rust `char` (Unicode-SCALAR) count into `text`
/// (`crates/rupu-tools/src/ast_grep.rs`'s `byte_to_char_idx`/`text_offset`) —
/// NOT a UTF-16 code-unit count and NOT a grapheme-cluster count. Swift's own
/// `String.Index`/`String.count` are grapheme-cluster based, so indexing via
/// `String.index(_:offsetBy:)` (or comparing against `text.count`) would
/// silently misalign whenever a grapheme spans more than one Unicode scalar —
/// an accented letter written as a base character plus a combining mark is
/// the classic case: ONE grapheme, TWO scalars. This walks
/// `text.unicodeScalars` instead — the Swift analogue of the web's own
/// `Array.from(text)` codepoint array (`ToolCard.tsx:608-609`) — so a span
/// starting after such a character still lands on the correct scalar
/// boundary. See `AstGrepModelTests.
/// highlightedAstGrepMatchSlicesByCodepointNotGrapheme` for the non-ASCII
/// regression case.
func highlightedAstGrepMatch(text: String, metaVars: [AstGrepTranscriptParsing.MetaVar]) -> AttributedString {
    let scalars = Array(text.unicodeScalars)

    func slice(_ range: Range<Int>) -> AttributedString {
        guard range.lowerBound >= 0, range.upperBound <= scalars.count, range.lowerBound <= range.upperBound else {
            return AttributedString("")
        }
        var view = String.UnicodeScalarView()
        view.append(contentsOf: scalars[range])
        return AttributedString(String(view))
    }

    // Only entries with a resolvable `textOffset` participate — sorted by
    // start so sequential slicing (mirroring the web's `cursor`) is safe; an
    // out-of-range or overlapping span is skipped rather than corrupting the
    // render (`ToolCard.tsx:624-625`'s own guard, ported verbatim).
    let spans = metaVars
        .compactMap { mv -> (start: Int, end: Int)? in
            guard let offset = mv.textOffset else { return nil }
            return (offset.start, offset.end)
        }
        .sorted { $0.start < $1.start }

    var result = AttributedString()
    var cursor = 0
    for span in spans {
        guard span.start >= cursor, span.end >= span.start, span.end <= scalars.count else { continue }
        if span.start > cursor {
            result += slice(cursor..<span.start)
        }
        var highlighted = slice(span.start..<span.end)
        highlighted.foregroundColor = .rupuWarn
        highlighted.backgroundColor = .rupuWarnBg
        result += highlighted
        cursor = span.end
    }
    if cursor < scalars.count {
        result += slice(cursor..<scalars.count)
    }
    return result
}
