import Foundation
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

// ---------------------------------------------------------------------------
// Header badges (ToolCard.tsx:281-344)
// ---------------------------------------------------------------------------

/// Marks a `tool_audit` outcome (step `actions:` enforcement's audit
/// trail) — direct port of `AuditBadge` (`ToolCard.tsx:315-344`). `blocked`
/// (a denied call) gets an err-tinted badge; `declared && !granted` (spec
/// §3c's authoring-mistake case) gets a warn-tinted badge even when the
/// call itself wasn't blocked; otherwise a neutral "audited" marker
/// confirms the trail exists without implying anything went wrong. Each
/// case's explanation travels as a `.help(_:)` tooltip, the exact strings
/// the web carries as its own `title` attribute.
public struct AuditBadge: View {
    private let audit: ToolEntry.Audit

    public init(audit: ToolEntry.Audit) {
        self.audit = audit
    }

    public var body: some View {
        if audit.blocked {
            Badge("blocked", tone: .rupuErr)
                .help("This tool call was denied (narrowed out of the step's actions: allowlist, or refused by the run's permission mode).")
        } else if audit.declared && !audit.granted {
            Badge("not granted", tone: .rupuWarn)
                .help("This step's actions: names a tool the agent's tools: grant does not cover — narrowed away, likely an authoring mistake.")
        } else {
            Badge("audited", tone: .rupuMute)
                .help("Audited catalog tool call — declared/granted/blocked all clean.")
        }
    }
}

/// Header status badge — port of `StatusBadge` (`ToolCard.tsx:289-305`),
/// with one deliberate tone deviation called out below.
///
/// - `error` set → "error", err tone (matches web).
/// - else `durationMS` present → `Fmt.duration(ms:)`, mute tone (matches
///   web's neutral `DurationBadge`).
/// - else → "ok", **ok tone** (`rupuOk`) — the task brief's explicit call;
///   the web's own fallback (`ToolCard.tsx:300-304`) renders "ok" in the
///   SAME neutral/mute tone as the duration badge (`bg-surface text-ink-
///   mute`), never green. This port intentionally diverges to give a
///   genuinely successful, no-duration-recorded call a distinct affirmative
///   tint rather than reusing the "we don't know" mute tone.
public struct StatusBadge: View {
    private let entry: ToolEntry

    public init(entry: ToolEntry) {
        self.entry = entry
    }

    public var body: some View {
        if entry.errorText != nil {
            Badge("error", tone: .rupuErr)
        } else if let ms = entry.durationMS {
            Badge(Fmt.duration(ms: ms), tone: .rupuMute)
        } else {
            Badge("ok", tone: .rupuOk)
        }
    }
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// Same 60-char / "…" truncation `summarizeInput` (`TranscriptViewModel.
/// swift`) uses — re-implemented here (rather than imported) since that
/// helper's `truncated(_:limit:)` is `private` to its own file; Swift's
/// `private` is file-scoped, not module-scoped.
private func headerTruncated(_ s: String, limit: Int = 60) -> String {
    guard s.count > limit else { return s }
    return String(s.prefix(limit - 3)) + "…"
}

private func nonEmptyLines(_ output: String?) -> [String] {
    guard let output else { return [] }
    return output.split(separator: "\n", omittingEmptySubsequences: false)
        .map(String.init)
        .filter { !$0.isEmpty }
}

/// A scrollable mono block capped at an approximate visible-line count —
/// shared shape behind `.read`/`.grep`/`.glob`'s bodies and `.terminal`'s
/// output section. `maxHeight` approximates `lineCount` rows of
/// `dataMono(11.5)` text at a ~14pt line height (matching the web's own
/// `max-h-*`/`overflow-y-auto` scroll caps, `ToolCard.tsx`'s Read/Grep/Glob
/// bodies and `TerminalBlock.tsx`'s output pre).
private struct MonoScrollBlock: View {
    let text: String
    let lineCount: Int
    var background: Color = .rupuSurface
    var foreground: Color = .rupuInk

    var body: some View {
        ScrollView(.vertical, showsIndicators: true) {
            Text(text)
                .font(.dataMono(11.5))
                .foregroundStyle(foreground)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
        }
        .frame(maxHeight: CGFloat(lineCount) * 14)
        .background(background)
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

// ---------------------------------------------------------------------------
// Error block (ToolCard.tsx:374-383)
// ---------------------------------------------------------------------------

private struct ErrorBlockView: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Error")
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.rupuErr)
            Text(text)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuErr)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(Color.rupuErrBg)
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

// ---------------------------------------------------------------------------
// Kind-specific bodies (ToolCard.tsx:389-552)
// ---------------------------------------------------------------------------

/// `.terminal` — port of `TerminalBlock.tsx`, restyled onto this app's own
/// tokens rather than the web's dark slate chrome (this design system has
/// no dark-terminal treatment elsewhere; the header's `StatusBadge`/
/// `AuditBadge` already carry the card's own chrome). Prompt line prefers
/// the adjacency-paired `entry.command.argv` (the real recorded argv);
/// falls back to the same `command`/`cmd` input-field extraction
/// `summarizeInput` uses when no `command_run` event ever paired onto this
/// call (an older transcript, or a truncated one).
private struct TerminalBodyView: View {
    let entry: ToolEntry

    private var commandText: String {
        if let argv = entry.command?.argv, !argv.isEmpty {
            return argv.joined(separator: " ")
        }
        if case .object(let rec) = entry.input {
            if case .string(let cmd)? = rec["command"] { return cmd }
            if case .string(let cmd)? = rec["cmd"] { return cmd }
        }
        return ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 6) {
                Text("$")
                    .font(.dataMono(11.5).weight(.bold))
                    .foregroundStyle(Color.rupuOk)
                Text(commandText)
                    .font(.dataMono(11.5))
                    .foregroundStyle(Color.rupuInk)
                    .textSelection(.enabled)
            }
            if let output = entry.output, !output.isEmpty {
                MonoScrollBlock(text: output, lineCount: 24)
            }
            if entry.command != nil || entry.output != nil {
                HStack(spacing: 8) {
                    if let command = entry.command {
                        Badge("exit \(command.exitCode)", tone: command.exitCode == 0 ? .rupuOk : .rupuErr)
                    }
                    if let cwd = entry.command?.cwd, !cwd.isEmpty {
                        Text(cwd)
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuMute)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
    }
}

/// `.read` — mono block, capped scroll (`ReadBody`, `ToolCard.tsx:389-400`).
private struct ReadBodyView: View {
    let entry: ToolEntry

    var body: some View {
        if let output = entry.output, !output.isEmpty {
            MonoScrollBlock(text: output, lineCount: 24)
        }
    }
}

/// `.grep` — match-count line + mono lines (`GrepBody`, `ToolCard.tsx:
/// 402-419`).
private struct GrepBodyView: View {
    let entry: ToolEntry

    var body: some View {
        let lines = nonEmptyLines(entry.output)
        if !lines.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(lines.count) match\(lines.count == 1 ? "" : "es")")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                MonoScrollBlock(text: lines.joined(separator: "\n"), lineCount: 18)
            }
        }
    }
}

/// `.glob` — mono path list, no count line (`GlobBody`, `ToolCard.tsx:
/// 421-435`).
private struct GlobBodyView: View {
    let entry: ToolEntry

    var body: some View {
        let lines = nonEmptyLines(entry.output)
        if !lines.isEmpty {
            MonoScrollBlock(text: lines.joined(separator: "\n"), lineCount: 16)
        }
    }
}

// ---------------------------------------------------------------------------
// .subrun (ToolCard.tsx:437-509)
// ---------------------------------------------------------------------------

/// The fields this port actually reads off a `dispatch_agent`/
/// `dispatch_agents_parallel` result's JSON `output` string.
///
/// **Deliberate deviation from `SubrunBody` (`ToolCard.tsx:437-509`).** The
/// web reads `rec.status`/`rec.total_tokens` — but the real Rust wire
/// producers (`crates/rupu-tools/src/dispatch_agent.rs:156-168`,
/// `dispatch_agents_parallel.rs:235-247`) never emit either field; they
/// emit `ok` (bool) and `tokens_used` (number). Porting the web's field
/// names verbatim would silently render nothing for every real call.
/// `transcript_path`/`sub_run_id` DO match the web's own extraction
/// (`ToolCard.tsx:449-450`) and the real wire, so those two are read
/// as-is.
struct SubrunPayload: Equatable {
    let ok: Bool?
    let tokensUsed: Int?
    let transcriptPath: String?
    let subRunID: String?

    /// A sub-run identifier is "resolvable" (the task brief's own term)
    /// when either half of the pair the wire always emits together is
    /// present.
    var hasResolvableTarget: Bool { transcriptPath != nil || subRunID != nil }
}

/// Parses `entry.output`'s JSON body into a `SubrunPayload`. `nil` when
/// `output` is absent, isn't valid JSON, isn't a top-level object, or
/// (a `dispatch_agents_parallel` result, keyed by request id rather than
/// carrying these fields at its own top level) yields none of the four
/// fields — an honest "nothing to show" rather than a payload of all-nil
/// fields that would render empty chips.
func parseSubrunPayload(_ output: String?) -> SubrunPayload? {
    guard let output, let data = output.data(using: .utf8) else { return nil }
    guard let value = try? JSONDecoder().decode(JSONValue.self, from: data) else { return nil }
    guard case .object(let rec) = value else { return nil }

    let ok: Bool? = { if case .bool(let b)? = rec["ok"] { return b }; return nil }()
    let tokensUsed: Int? = { if case .number(let n)? = rec["tokens_used"] { return Int(n) }; return nil }()
    let transcriptPath: String? = { if case .string(let s)? = rec["transcript_path"] { return s }; return nil }()
    let subRunID: String? = { if case .string(let s)? = rec["sub_run_id"] { return s }; return nil }()

    guard ok != nil || tokensUsed != nil || transcriptPath != nil || subRunID != nil else { return nil }
    return SubrunPayload(ok: ok, tokensUsed: tokensUsed, transcriptPath: transcriptPath, subRunID: subRunID)
}

/// `.subrun` — status + token Badge chips, plus a "View sub-run transcript
/// →" affordance.
///
/// **Navigation decision (task brief's explicit instruction).** `runID`/
/// `host` on `ToolCardView` carry the CURRENT run's context, not a
/// navigation callback — nothing in `RupuRunDetail` today lets a leaf
/// rendering view push navigation (every `model.navigate(to:)` call site
/// in this codebase sits at a screen's own top level, wired from an
/// explicit closure or the shared `AppModel`, never reached from inside a
/// transcript row). Rather than invent a navigation path this module has
/// no precedent for, this body renders the row as inert, ghost-styled
/// TEXT (not a `Button`) whenever a sub-run target (`transcript_path`/
/// `sub_run_id`) IS resolvable from the parsed payload — visible, honestly
/// non-interactive — and omits the row entirely when nothing is
/// resolvable. Task 7 (which owns feed wiring — CLAUDE.md's `RupuRunDetail`
/// module note) is the right place to thread a real navigation callback
/// through and turn this into a live `Button`.
private struct SubrunBodyView: View {
    let entry: ToolEntry
    let runID: String?
    let host: String?

    var body: some View {
        if let payload = parseSubrunPayload(entry.output) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    if let ok = payload.ok {
                        Badge(ok ? "ok" : "failed", tone: ok ? .rupuOk : .rupuErr)
                    }
                    if let tokensUsed = payload.tokensUsed {
                        Badge("\(Fmt.count(tokensUsed)) tokens", tone: .rupuMute)
                    }
                    Spacer(minLength: 0)
                }
                if payload.hasResolvableTarget {
                    Text("View sub-run transcript →")
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                }
            }
        } else if let output = entry.output, !output.isEmpty {
            // Unparseable output (not JSON, or JSON without any of the
            // four fields) — an honest raw fallback rather than silence.
            MonoScrollBlock(text: output, lineCount: 16)
        }
    }
}

// ---------------------------------------------------------------------------
// StructuredView (StructuredView.tsx) — .coverage/.generic, and the
// .finding fallthrough documented on `ToolCardView` below.
// ---------------------------------------------------------------------------

/// Dependency-free recursive key/value renderer for arbitrary `JSONValue`
/// data — port of `StructuredView.tsx`. Dispatch order mirrors the web
/// exactly: depth cap first, then null/bool/number/string, then array
/// (homogeneous-object-array → table, scalar-array → chip list, else
/// numbered rows), then object.
public struct StructuredView: View {
    private let value: JSONValue
    private let depth: Int

    private static let depthCap = 4
    private static let stringInlineMax = 120

    public init(value: JSONValue, depth: Int = 0) {
        self.value = value
        self.depth = depth
    }

    public var body: some View {
        if depth > Self.depthCap {
            MonoScrollBlock(text: Self.prettyPrinted(value), lineCount: 12)
        } else {
            switch value {
            case .null:
                Text("null").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
            case .bool(let b):
                boolPill(b)
            case .number(let n):
                Text(Self.numberText(n)).font(.dataMono(10)).foregroundStyle(Color.rupuInk)
            case .string(let s):
                stringView(s)
            case .array(let items):
                arrayView(items)
            case .object(let obj):
                objectView(obj)
            }
        }
    }

    private func boolPill(_ b: Bool) -> some View {
        Text(b ? "true" : "false")
            .font(.dataMono(10).weight(.medium))
            .foregroundStyle(b ? Color.rupuOk : Color.rupuDim)
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(b ? Color.rupuOkBg : Color.rupuSurface)
            .clipShape(Capsule())
    }

    @ViewBuilder
    private func stringView(_ s: String) -> some View {
        if s.count > Self.stringInlineMax || s.contains("\n") {
            MonoScrollBlock(text: s, lineCount: 8)
        } else {
            Text(s).font(.uiText).foregroundStyle(Color.rupuInk).textSelection(.enabled)
        }
    }

    @ViewBuilder
    private func arrayView(_ items: [JSONValue]) -> some View {
        if items.isEmpty {
            Text("[]").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
        } else if Self.isHomogeneousObjectArray(items) {
            let rows: [[String: JSONValue]] = items.compactMap {
                if case .object(let o) = $0 { return o }
                return nil
            }
            ObjectArrayTable(rows: rows, depth: depth)
        } else if Self.isScalarArray(items) {
            ScalarChipList(items: items)
        } else {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .top, spacing: 6) {
                        Text("\(index)").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
                        StructuredView(value: item, depth: depth + 1)
                    }
                }
            }
            .padding(.leading, 6)
            .overlay(Rectangle().fill(Color.rupuBorder).frame(width: 1), alignment: .leading)
        }
    }

    @ViewBuilder
    private func objectView(_ obj: [String: JSONValue]) -> some View {
        if obj.isEmpty {
            Text("{}").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
        } else {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Self.orderedKeys(obj), id: \.self) { key in
                    HStack(alignment: .top, spacing: 6) {
                        Text(key)
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuBrand700)
                        StructuredView(value: obj[key] ?? .null, depth: depth + 1)
                    }
                }
            }
        }
    }

    /// `JSONValue.object` is backed by `[String: JSONValue]` — insertion
    /// order isn't preserved by the Swift dictionary itself, unlike the
    /// web's `Object.entries` (which walks in insertion order). Sorting the
    /// keys is the closest STABLE ordering available here (repeated
    /// renders of the same value show the same row order) — an accepted,
    /// documented divergence from the web's insertion order, not a bug.
    private static func orderedKeys(_ obj: [String: JSONValue]) -> [String] {
        obj.keys.sorted()
    }

    private static func isHomogeneousObjectArray(_ items: [JSONValue]) -> Bool {
        !items.isEmpty && items.allSatisfy {
            if case .object = $0 { return true }
            return false
        }
    }

    private static func isScalarArray(_ items: [JSONValue]) -> Bool {
        items.allSatisfy {
            switch $0 {
            case .null, .string, .number, .bool: return true
            case .object, .array: return false
            }
        }
    }

    private static func numberText(_ n: Double) -> String {
        n.truncatingRemainder(dividingBy: 1) == 0 && abs(n) < 1e15 ? String(Int64(n)) : String(n)
    }

    private static func prettyPrinted(_ value: JSONValue) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: foundationValue(value), options: [.prettyPrinted, .sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return String(describing: value)
        }
        return text
    }

    private static func foundationValue(_ value: JSONValue) -> Any {
        switch value {
        case .string(let s): return s
        case .number(let n): return n
        case .bool(let b): return b
        case .object(let o): return o.mapValues(foundationValue)
        case .array(let a): return a.map(foundationValue)
        case .null: return NSNull()
        }
    }
}

/// Scalar array → chip list (`ScalarChip`/wrapping span, `StructuredView.
/// tsx:56-63,207-215`). Wraps left-to-right, top-to-bottom via a small
/// custom `Layout` — SwiftUI has no built-in flow container.
private struct ScalarChipList: View {
    let items: [JSONValue]

    var body: some View {
        FlowLayout(spacing: 4) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                Text(chipText(item))
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuInk)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.rupuSurface)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }
        }
    }

    private func chipText(_ v: JSONValue) -> String {
        switch v {
        case .string(let s): return s
        case .number(let n): return n.truncatingRemainder(dividingBy: 1) == 0 ? String(Int64(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        case .object, .array: return "" // unreachable — isScalarArray excludes these
        }
    }
}

/// Homogeneous object array → compact table (`TableView`, `StructuredView.
/// tsx:65-105`). Column set is the union of keys across every row,
/// `Grid`-laid-out so columns align.
private struct ObjectArrayTable: View {
    let rows: [[String: JSONValue]]
    let depth: Int

    private var keys: [String] {
        var union = Set<String>()
        for row in rows { union.formUnion(row.keys) }
        return union.sorted()
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: true) {
            Grid(alignment: .topLeading, horizontalSpacing: 8, verticalSpacing: 4) {
                GridRow {
                    ForEach(keys, id: \.self) { key in
                        Text(key)
                            .font(.dataMono(10).weight(.semibold))
                            .foregroundStyle(Color.rupuBrand700)
                    }
                }
                Divider()
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(keys, id: \.self) { key in
                            StructuredView(value: row[key] ?? .null, depth: depth + 1)
                        }
                    }
                }
            }
            .padding(6)
        }
        .background(Color.rupuSurface)
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

/// Minimal left-to-right, top-to-bottom wrapping layout — used only by
/// `ScalarChipList` above. Not a general-purpose addition to `RupuDesign`;
/// scoped here since nothing else in this module needs flow wrapping yet.
private struct FlowLayout: Layout {
    var spacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > maxWidth {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: maxWidth.isFinite ? maxWidth : x, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x: CGFloat = bounds.minX
        var y: CGFloat = bounds.minY
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), anchor: .topLeading, proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCardView — the public entry point (ToolCard.tsx:880-952)
// ---------------------------------------------------------------------------

/// One tool call's card: a collapsed-by-default header (gear + tool name +
/// input summary + audit/status badges) disclosing a kind-specific body.
/// Direct port of `ToolCard`'s non-`finding` anatomy (`ToolCard.tsx:
/// 916-951`) — `.finding` gets its own full-chrome card with no header in
/// the web (`ToolCard.tsx:897-900`) and, in this app, its own `FindingCard`
/// dispatch in Task 7; until then it falls through to `StructuredView` here
/// so every `ToolKind` renders SOMETHING rather than a compile-time gap or a
/// silent blank. `.astGrep` dispatches to `AstGrepBodyView` (Task 6).
public struct ToolCardView: View {
    private let entry: ToolEntry
    private let runID: String?
    private let host: String?
    /// Optional (defaulted `nil`) — threaded straight through to
    /// `AstGrepBodyView`'s own `sourcePreviewStore`, following the same
    /// "omit the toggle rather than invent plumbing" convention `TranscriptFeed`
    /// established in Phase 6B, Task 5. No existing call site passes this yet;
    /// Task 7's transcript rewrite is the seam that will.
    private let sourcePreviewStore: SourcePreviewStore?

    @State private var expanded = false

    public init(entry: ToolEntry, runID: String?, host: String?, sourcePreviewStore: SourcePreviewStore? = nil) {
        self.entry = entry
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
    }

    public var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 8) {
                kindBody
                if let errorText = entry.errorText {
                    ErrorBlockView(text: errorText)
                }
            }
            .padding(.top, 6)
        } label: {
            header
        }
        .padding(10)
        .panelStyle(.innerCard)
    }

    private var header: some View {
        HStack(spacing: 8) {
            Icon(.settings, size: 11)
                .foregroundStyle(Color.rupuBrand700)
            Text(entry.tool.isEmpty ? "—" : entry.tool)
                .font(.dataMono(11.5).weight(.semibold))
                .foregroundStyle(Color.rupuBrand700)
            if let summary = headerSummary, !summary.isEmpty {
                Text(summary)
                    .font(.metaText)
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 0)
            if let audit = entry.audit {
                AuditBadge(audit: audit)
            }
            StatusBadge(entry: entry)
        }
    }

    /// Header input summary — prefers the adjacency-paired `entry.
    /// command.argv` for `.terminal` (matching `ToolCard.tsx:904-914`'s
    /// `terminalSummary`, which prefers the paired `tool.terminal.command`
    /// over a generic `summarizeInput` read of raw input fields), else
    /// falls back to `summarizeInput` for every other kind.
    private var headerSummary: String? {
        if entry.kind == .terminal, let argv = entry.command?.argv, !argv.isEmpty {
            return headerTruncated(argv.joined(separator: " "))
        }
        return summarizeInput(tool: entry.tool, kind: entry.kind, input: entry.input)
    }

    @ViewBuilder
    private var kindBody: some View {
        switch entry.kind {
        case .diff:
            DiffView(diff: entry.fileEdit?.diff ?? entry.output ?? "")
        case .terminal:
            TerminalBodyView(entry: entry)
        case .read:
            ReadBodyView(entry: entry)
        case .grep:
            GrepBodyView(entry: entry)
        case .glob:
            GlobBodyView(entry: entry)
        case .subrun:
            SubrunBodyView(entry: entry, runID: runID, host: host)
        case .astGrep:
            AstGrepBodyView(entry: entry, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        case .coverage, .generic, .finding:
            // `.finding` → Task 7's `FindingCard`. Doesn't exist yet, so it
            // falls through to the same generic `StructuredView` `.coverage`/
            // `.generic` already use — compiles, and renders something
            // honest, rather than a silent gap. See this view's own doc
            // comment.
            StructuredView(value: entry.structured ?? entry.input)
        }
    }
}
