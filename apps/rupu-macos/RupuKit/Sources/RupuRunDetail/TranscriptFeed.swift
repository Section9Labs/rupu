import SwiftUI
import Foundation
import RupuAPI
import RupuStore
import RupuDesign

/// Renders a flat `[TranscriptEvent]` array as a scrolling feed. Deliberately
/// has **no run-specific coupling of its own** — it doesn't know about
/// `RunDetailStore`, a run ID, or a transcript path; it just renders
/// whatever events it's handed, in order. `RunDetailScreen` (Task 8) feeds
/// it `store.transcript`; Task 9 (Session detail) reuses this exact view
/// unchanged for a session's own transcript.
///
/// **`sourcePreviewStore` is the one deliberate exception** (Phase 6B, Task
/// 5): an optional, defaulted-`nil` `SourcePreviewStore` that — when
/// present — lets an `ast_grep` tool-call row's matches expand into an
/// inline source slice / CST tree (`SourcePreview`/`AstTreeView`). This view
/// still doesn't know a run ID or host itself; those live inside the store
/// it's handed. Callers that don't pass one (`SessionDetailScreen`,
/// `AgentRunDetailScreen`) render exactly as before — plain `ast_grep`
/// match text, no expand affordance. Only `RunDetailScreen`'s transcript tab
/// passes one currently: `SourcePreviewStore`'s `id` targets `GET /api/runs/
/// :id/source|ast`, which 404s for a standalone agent run the same way `GET
/// /api/runs/:id` already does (see `AgentRunDetailScreen`'s own doc
/// comment) — wiring it into that screen, or into a session's per-turn
/// agent-run transcript, would point the fetch at an endpoint that can't
/// answer for that ID.
///
/// Per the brief's render list: `assistant_message` becomes a prose block;
/// `tool_call` pairs with its matching `tool_result` (by `call_id`) into one
/// collapsed row; `gate_requested` gets a 2px `Color.status(.awaiting)` left
/// edge; `command_run`/`file_edit` render as one-line summaries;
/// `run_complete` is a terminal row.
///
/// **`assistant_delta` is deliberately never rendered** (review fix, web-
/// viewer parity): the transcript JSONL persists both per-chunk
/// `assistant_delta` events *and* the consolidated `assistant_message` for
/// the same turn, so rendering both would show every assistant turn twice —
/// once streamed in pieces, once again whole. The CP web viewer already
/// made this call; this feed matches it. The `.assistantDelta` case stays
/// in `TranscriptEvent`'s decode enum (the JSONL still carries the events,
/// and other consumers may still want them) — it's filtered out here, in
/// the view layer, not at decode time.
///
/// Every other variant (`turn_start`/`turn_end`/`usage`/`action_emitted`/
/// `tool_audit`/`net_flow`/`.unknown`) — and an orphaned `tool_result` with
/// no matching `tool_call` still renders standalone rather than vanishing
/// silently) — is skipped: nothing in this phase's design renders them yet.
public struct TranscriptFeed: View {
    private let events: [TranscriptEvent]
    private let sourcePreviewStore: SourcePreviewStore?

    public init(events: [TranscriptEvent], sourcePreviewStore: SourcePreviewStore? = nil) {
        self.events = events
        self.sourcePreviewStore = sourcePreviewStore
    }

    public var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                        TranscriptRowView(row: row, sourcePreviewStore: sourcePreviewStore)
                            .id(index)
                    }
                    if rows.isEmpty {
                        Text("No transcript events yet")
                            .font(.noteText)
                            .foregroundStyle(Color.rupuMute)
                            .padding(.top, 24)
                    }
                    // Anchor for auto-scroll-to-bottom as new tail events arrive.
                    Color.clear.frame(height: 1).id(Self.bottomAnchorID)
                }
                .padding(12)
            }
            .onChange(of: events.count) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) {
                    proxy.scrollTo(Self.bottomAnchorID, anchor: .bottom)
                }
            }
        }
    }

    private static let bottomAnchorID = "transcript-feed-bottom"

    /// Pairs each `tool_call` with the `tool_result` sharing its `call_id`
    /// (a linear scan — transcripts this phase are small enough that this
    /// never needs to be an index) and drops the standalone `tool_result`
    /// row once it's been folded into its pair. A `tool_result` whose
    /// `call_id` never appears on any `tool_call` in this event list (a
    /// truncated snapshot, a pre-Phase-2 transcript) still renders on its
    /// own rather than disappearing.
    private var rows: [TranscriptRow] {
        let callIDsWithCall: Set<String> = Set(events.compactMap {
            if case .toolCall(let callID, _, _) = $0 { return callID }
            return nil
        })
        var resultsByCallID: [String: TranscriptEvent] = [:]
        for event in events {
            if case .toolResult(let callID, _, _, _, _) = event {
                resultsByCallID[callID] = event
            }
        }

        var built: [TranscriptRow] = []
        for event in events {
            switch event {
            case .assistantMessage, .gateRequested, .commandRun, .fileEdit, .runComplete:
                built.append(.single(event))
            case .toolCall(let callID, _, _):
                built.append(.toolPair(call: event, result: resultsByCallID[callID]))
            case .toolResult(let callID, _, _, _, _):
                guard !callIDsWithCall.contains(callID) else { continue } // folded into its pair above
                built.append(.single(event))
            // `assistant_delta` is never rendered — see the type doc
            // comment's "assistant_delta is deliberately never rendered"
            // section; the turn's `assistant_message` already carries the
            // same text as one consolidated block.
            case .assistantDelta, .runStart, .turnStart, .turnEnd, .usage, .actionEmitted, .toolAudit, .netFlow, .unknown:
                continue
            }
        }
        return built
    }
}

/// One rendered feed row: either a single event, or a `tool_call` folded
/// together with its (possibly absent, if not finished yet) `tool_result`.
private enum TranscriptRow {
    case single(TranscriptEvent)
    case toolPair(call: TranscriptEvent, result: TranscriptEvent?)
}

private struct TranscriptRowView: View {
    let row: TranscriptRow
    let sourcePreviewStore: SourcePreviewStore?

    var body: some View {
        switch row {
        case .single(let event):
            singleRow(event)
        case .toolPair(let call, let result):
            ToolCallRow(call: call, result: result, sourcePreviewStore: sourcePreviewStore)
        }
    }

    @ViewBuilder
    private func singleRow(_ event: TranscriptEvent) -> some View {
        switch event {
        case .assistantMessage(let content, let thinking):
            ProseRow(content: content, thinking: thinking)
        case .gateRequested(let gateID, let prompt, let decision, let decidedBy):
            GateRequestedRow(gateID: gateID, prompt: prompt, decision: decision, decidedBy: decidedBy)
        case .commandRun(let argv, let cwd, let exitCode, let stdoutBytes, let stderrBytes):
            CommandRunRow(argv: argv, cwd: cwd, exitCode: exitCode, stdoutBytes: stdoutBytes, stderrBytes: stderrBytes)
        case .fileEdit(let path, let kind, let diff):
            FileEditRow(path: path, kind: kind, diff: diff)
        case .runComplete(let runID, let status, let totalTokens, let durationMS, let error):
            RunCompleteRow(runID: runID, status: status, totalTokens: totalTokens, durationMS: durationMS, error: error)
        case .assistantDelta, .toolCall, .toolResult, .runStart, .turnStart, .turnEnd, .usage, .actionEmitted, .toolAudit, .netFlow, .unknown:
            EmptyView()
        }
    }
}

// MARK: - Row kinds

private struct ProseRow: View {
    let content: String
    let thinking: String?

    @State private var thinkingExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(content)
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
            if let thinking, !thinking.isEmpty {
                DisclosureGroup(isExpanded: $thinkingExpanded) {
                    Text(thinking)
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                        .padding(.top, 4)
                        .textSelection(.enabled)
                } label: {
                    Eyebrow("Thinking")
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

private struct ToolCallRow: View {
    let call: TranscriptEvent
    let result: TranscriptEvent?
    let sourcePreviewStore: SourcePreviewStore?

    @State private var expanded = false

    private var callID: String {
        if case .toolCall(let id, _, _) = call { return id }
        return ""
    }
    private var tool: String {
        if case .toolCall(_, let tool, _) = call { return tool }
        return ""
    }
    private var input: JSONValue? {
        if case .toolCall(_, _, let input) = call { return input }
        return nil
    }
    private var resultOutput: String? {
        if case .toolResult(_, let output, _, _, _) = result { return output }
        return nil
    }
    private var resultError: String? {
        if case .toolResult(_, _, let error, _, _) = result { return error }
        return nil
    }
    private var resultDurationMS: UInt64? {
        if case .toolResult(_, _, _, let durationMS, _) = result { return durationMS }
        return nil
    }
    private var resultStructured: JSONValue? {
        if case .toolResult(_, _, _, _, let structured) = result { return structured }
        return nil
    }

    /// `ast_grep` tool calls carry the raw builtin tool name on the wire
    /// (verified against `crates/rupu-cp/web/src/components/transcript/
    /// transcriptView.ts`'s `classify(tool:)`, which switches on this exact
    /// literal) — no separate `kind` field exists on `TranscriptEvent`
    /// itself, so this checks `tool` directly.
    private var isAstGrep: Bool { tool == "ast_grep" }

    /// `fromStructured`'s parsed result — `nil` for a non-`ast_grep` tool or
    /// when `resultStructured` doesn't carry a `matches` array at all (see
    /// `AstGrepTranscriptParsing`'s doc comment for the exact trigger and
    /// its documented divergence from the web parser).
    private var astGrepStructured: AstGrepTranscriptParsing.StructuredResult? {
        guard isAstGrep else { return nil }
        return AstGrepTranscriptParsing.fromStructured(resultStructured)
    }

    /// Parsed `ast_grep` matches — structured (`resultStructured`) first,
    /// falling back to a text-parse of `resultOutput` when structured
    /// parsing yields nothing (an older run, a text-only transcript, or —
    /// per `AstGrepTranscriptParsing`'s doc comment — every individual
    /// match in a present `matches` array failing to parse). Empty (not
    /// just for a non-`ast_grep` tool) whenever neither source yields
    /// anything to show.
    private var astGrepMatches: [AstGrepTranscriptParsing.Match] {
        guard isAstGrep else { return [] }
        if let structured = astGrepStructured, !structured.matches.isEmpty { return structured.matches }
        return AstGrepTranscriptParsing.fromText(resultOutput ?? "")
    }

    /// The matches section's count label — see `AstGrepTranscriptParsing.
    /// matchCountLabel`'s doc comment for the truncation-honesty contract.
    private var astGrepMatchCountLabel: String {
        AstGrepTranscriptParsing.matchCountLabel(structured: astGrepStructured, matches: astGrepMatches)
    }

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 8) {
                if let input {
                    jsonBlock(label: "Input", json: input)
                }
                if let resultError {
                    Text("Error")
                        .font(.metaText)
                        .foregroundStyle(Color.status(.failed))
                    Text(resultError)
                        .font(.dataMono(11.5))
                        .foregroundStyle(Color.status(.failed))
                        .textSelection(.enabled)
                } else if let resultOutput {
                    jsonBlock(label: "Output", text: resultOutput)
                }
                if let resultStructured {
                    jsonBlock(label: "Structured", json: resultStructured)
                }
                // Phase 6B, Task 5: an `ast_grep` result's matches each get
                // their own file:line[:col] row, with an inline "source" /
                // "tree" disclosure per match once `sourcePreviewStore` is
                // available — additive alongside the raw Output/Structured
                // blocks above, never replacing them (nothing this view
                // already showed becomes harder to see).
                if isAstGrep, resultError == nil, !astGrepMatches.isEmpty {
                    astGrepMatchesSection
                }
                if result == nil {
                    Text("Awaiting result")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }
            }
            .padding(.top, 6)
        } label: {
            HStack(spacing: 8) {
                Text(tool)
                    .font(.dataMono(11.5))
                    .foregroundStyle(Color.rupuInk)
                Text("#\(callID)")
                    .font(.dataMono(11.5))
                    .foregroundStyle(Color.rupuMute)
                if let resultDurationMS {
                    Text(Fmt.duration(ms: resultDurationMS))
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                }
                if resultError != nil {
                    Circle().fill(Color.status(.failed)).frame(width: 6, height: 6)
                }
                Spacer(minLength: 0)
            }
        }
        .padding(10)
        .panelStyle(.innerCard)
    }

    private var astGrepMatchesSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow(astGrepMatchCountLabel)
            ForEach(Array(astGrepMatches.enumerated()), id: \.offset) { _, match in
                AstGrepMatchRow(match: match, sourcePreviewStore: sourcePreviewStore)
            }
        }
    }

    @ViewBuilder
    private func jsonBlock(label: String, json: JSONValue) -> some View {
        jsonBlock(label: label, text: prettyPrintedJSON(json))
    }

    @ViewBuilder
    private func jsonBlock(label: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow(label)
            Text(text)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 4))
        }
    }
}

private struct GateRequestedRow: View {
    let gateID: String
    let prompt: String
    let decision: String?
    let decidedBy: String?

    var body: some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.status(.awaiting))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text("Gate")
                        .font(.metaText)
                        .foregroundStyle(Color.status(.awaiting))
                    Text("#\(gateID)")
                        .font(.dataMono(11.5))
                        .foregroundStyle(Color.rupuMute)
                }
                Text(prompt)
                    .font(.leadText)
                    .foregroundStyle(Color.rupuInk)
                if let decision {
                    Text(decidedBy.map { "\(decision) by \($0)" } ?? decision)
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                }
            }
            .padding(.leading, 10)
            .padding(.vertical, 8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.status(.awaiting).opacity(0.06))
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}

private struct CommandRunRow: View {
    let argv: [String]
    let cwd: String
    let exitCode: Int32
    let stdoutBytes: UInt64
    let stderrBytes: UInt64

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(Color.status(exitCode == 0 ? .done : .failed))
                .frame(width: 6, height: 6)
            Text("$ \(argv.joined(separator: " "))")
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 0)
            Text("exit \(exitCode)")
                .font(.dataMono(10))
                .foregroundStyle(exitCode == 0 ? Color.rupuDim : Color.status(.failed))
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

private struct FileEditRow: View {
    let path: String
    let kind: String
    let diff: String

    var body: some View {
        HStack(spacing: 8) {
            Text(kind)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
            Text(path)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

private struct RunCompleteRow: View {
    let runID: String
    let status: String
    let totalTokens: UInt64
    let durationMS: UInt64
    let error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle()
                    .fill(Color.status(status == "ok" ? .done : .failed))
                    .frame(width: 6, height: 6)
                Text("Run complete — \(status)")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuInk)
                Spacer(minLength: 0)
                Text("\(Fmt.count(Int(totalTokens))) tok · \(Fmt.duration(ms: durationMS))")
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuDim)
            }
            if let error {
                Text(error)
                    .font(.uiText)
                    .foregroundStyle(Color.status(.failed))
            }
        }
        .padding(10)
        .panelStyle(.innerCard)
    }
}

// MARK: - ast_grep match rows (Phase 6B, Task 5)

/// One parsed `ast_grep` match's `file:line:col` header, its matched text
/// (when known), and — once `sourcePreviewStore` is non-`nil` — independent
/// "source"/"tree" toggle buttons mounting `SourcePreview`/`AstTreeView`
/// beneath it. Mirrors the web `AstGrepMatchRow`/`AstGrepTextMatchRow`'s own
/// "either, both, or neither can be open at once" shape via two separate
/// `@State` flags, ported as one row type since this phase doesn't
/// distinguish the structured-vs-text-parsed match shapes the web keeps as
/// two components (`AstGrepTranscriptParsing.Match` already unifies them).
private struct AstGrepMatchRow: View {
    let match: AstGrepTranscriptParsing.Match
    let sourcePreviewStore: SourcePreviewStore?

    @State private var sourceOpen = false
    @State private var treeOpen = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                Text("\(match.file):\(match.startLine):\(match.startCol)")
                    .font(.dataMono(10.5))
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if sourcePreviewStore != nil {
                    disclosureToggle(title: "source", isOpen: $sourceOpen, accessibilityVerb: "source preview")
                    disclosureToggle(title: "tree", isOpen: $treeOpen, accessibilityVerb: "AST tree")
                }
                Spacer(minLength: 0)
            }
            if let text = match.text, !text.isEmpty {
                Text(text)
                    .font(.dataMono(10.5))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
            if sourceOpen, let sourcePreviewStore {
                SourcePreview(store: sourcePreviewStore, path: match.file, line: match.startLine)
            }
            if treeOpen, let sourcePreviewStore {
                AstTreeView(store: sourcePreviewStore, path: match.file, line: match.startLine, col: match.startCol)
            }
        }
        .padding(8)
        .panelStyle(.innerCard)
    }

    /// One "source"/"tree" toggle button — review fix (finding 7): a bare
    /// text button gave no visible sign of which of the two independent
    /// disclosures was currently open. A rotating chevron (same `chevronDown`
    /// + `rotationEffect` idiom `AstNodeRow`'s own expand chevron uses) plus
    /// an ink-vs-brand color swap when open, and an explicit accessibility
    /// label describing the CURRENT action ("Show"/"Hide" — not just the
    /// static title), close both gaps.
    private func disclosureToggle(title: String, isOpen: Binding<Bool>, accessibilityVerb: String) -> some View {
        Button {
            isOpen.wrappedValue.toggle()
        } label: {
            HStack(spacing: 2) {
                Icon(.chevronDown, size: 9)
                    .rotationEffect(.degrees(isOpen.wrappedValue ? 0 : -90))
                Text(title)
            }
        }
        .buttonStyle(.plain)
        .font(.metaText)
        .foregroundStyle(isOpen.wrappedValue ? Color.rupuInk : Color.rupuBrand)
        .accessibilityLabel(isOpen.wrappedValue ? "Hide \(accessibilityVerb)" : "Show \(accessibilityVerb)")
    }
}

// `AstGrepTranscriptParsing` — moved to `Rendering/AstGrepBody.swift` (Task
// 6, design-alignment Plan 4) alongside the new `AstGrepBodyView`, which
// extends it with flattened metavar bindings. Same module (`RupuRunDetail`),
// so every reference to it below (`astGrepStructured`, `astGrepMatches`,
// `astGrepMatchCountLabel`, `AstGrepMatchRow`) still resolves without an
// import change. This file's own `ast_grep` preview-mounting code (below)
// is superseded by `AstGrepBodyView` in Task 7's transcript rewrite — left
// as-is here per that task's explicit "minimal TranscriptFeed edit, keep it
// compiling" instruction.

// MARK: - JSON pretty printing

private func prettyPrintedJSON(_ value: JSONValue) -> String {
    switch value {
    case .object, .array:
        let object = foundationValue(value)
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else { return "" }
        return text
    case .string(let s): return s
    case .number(let n): return n.truncatingRemainder(dividingBy: 1) == 0 ? String(Int64(n)) : String(n)
    case .bool(let b): return b ? "true" : "false"
    case .null: return "null"
    }
}

private func foundationValue(_ value: JSONValue) -> Any {
    switch value {
    case .string(let s): return s
    case .number(let n): return n
    case .bool(let b): return b
    case .object(let o): return o.mapValues(foundationValue)
    case .array(let a): return a.map(foundationValue)
    case .null: return NSNull()
    }
}
