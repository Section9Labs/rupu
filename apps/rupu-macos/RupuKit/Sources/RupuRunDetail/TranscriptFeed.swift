import SwiftUI
import Foundation
import RupuAPI
import RupuStore
import RupuDesign

/// Renders a flat `[TranscriptEvent]` array as a scrolling, turn-grouped
/// feed — Plan 4's rich rewrite of the earlier flat tool_call/tool_result
/// row list. Deliberately has **no run-specific coupling of its own beyond
/// the two optional identifiers below** — it doesn't know about
/// `RunDetailStore` or a transcript path; it just renders whatever events
/// it's handed, in order. `RunDetailScreen` feeds it `store.transcript`;
/// `SessionDetailScreen`/`AgentRunDetailScreen` reuse this exact view for a
/// session's/standalone agent run's own transcript.
///
/// **Turn grouping** (`buildTranscriptViewModel`, `TranscriptViewModel.swift`)
/// folds `assistant_message`/`tool_call`/`tool_result`/`tool_audit`/
/// `file_edit`/`command_run`/`action_emitted` into one `TurnVM` per turn — a
/// single collapsible `TurnRowView` per turn, replacing the old flat
/// tool-pair rows. `gate_requested` and `run_complete` are excluded from
/// every `TurnVM` by that model (its own doc comment) and instead render as
/// standalone rows here, positioned back among the turns by
/// `buildFeedRows` — see that function's doc comment for exactly how.
///
/// **`runID`/`host`/`sourcePreviewStore`** thread straight through to every
/// `ToolCardView`/`FindingCard` this view renders. `sourcePreviewStore` keeps
/// the exact optional, defaulted-`nil` convention Phase 6B, Task 5
/// established: present only on `RunDetailScreen`'s transcript tab (the one
/// screen whose `SourcePreviewStore.id` targets an endpoint — `GET /api/
/// runs/:id/source|ast` — that can actually answer for that run); absent on
/// `SessionDetailScreen` and `AgentRunDetailScreen`, whose runs 404 there
/// the same way `GET /api/runs/:id` already does for a standalone run (see
/// `AgentRunDetailScreen`'s own doc comment). `runID`/`host` are newly
/// threaded through by this task (both default `nil` so no existing call
/// site needed to change to keep compiling) — every `SessionDetailScreen`
/// call now passes its own `store.focusedRunID`/`nil` (sessions have no
/// host-scoped API surface yet, see `SessionDetailStore`'s own doc comment),
/// and `AgentRunDetailScreen`/`RunDetailScreen`'s transcript tab pass their
/// own `runID`/`host` straight through.
///
/// **`assistant_delta` is deliberately never rendered** (review fix, web-
/// viewer parity, carried over unchanged from the pre-Task-7 feed): the
/// transcript JSONL persists both per-chunk `assistant_delta` events *and*
/// the consolidated `assistant_message` for the same turn, so rendering both
/// would show every assistant turn twice. `usage`/`action_emitted` (once
/// merged onto a standalone audit entry)/`tool_audit`/`net_flow`/`run_start`/
/// `.unknown` all stay non-rendered top-level rows too — `usage`/`run_start`
/// carry no per-row content this phase; `tool_audit`/`action_emitted` are
/// folded into their `ToolEntry` by `buildTranscriptViewModel` and rendered
/// as part of that entry's `ToolCardView`/`FindingCard`, never as their own
/// row.
public struct TranscriptFeed: View {
    private let events: [TranscriptEvent]
    private let runID: String?
    private let host: String?
    private let sourcePreviewStore: SourcePreviewStore?

    public init(events: [TranscriptEvent], runID: String? = nil, host: String? = nil, sourcePreviewStore: SourcePreviewStore? = nil) {
        self.events = events
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
    }

    private var rows: [FeedRow] { buildFeedRows(events: events) }

    /// Whether the WHOLE transcript has reached its terminal `run_complete`
    /// event — every turn's result pill reads `.ok` once this is true
    /// (unless that turn itself `hasError`), `.running` otherwise. Ported
    /// from the web's own `sawRunComplete` (`transcriptView.ts:476`), which
    /// is likewise a transcript-global flag, not a per-turn one.
    private var sawRunComplete: Bool {
        events.contains {
            if case .runComplete = $0 { return true }
            return false
        }
    }

    public var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        rowView(row)
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

    @ViewBuilder
    private func rowView(_ row: FeedRow) -> some View {
        switch row {
        case .turn(let turn):
            TurnRowView(turn: turn, sawRunComplete: sawRunComplete, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        case .gate(let gateID, let prompt, let decision, let decidedBy):
            GateRequestedRow(gateID: gateID, prompt: prompt, decision: decision, decidedBy: decidedBy)
        case .runComplete(let runID, let status, let totalTokens, let durationMS, let error):
            RunCompleteRow(runID: runID, status: status, totalTokens: totalTokens, durationMS: durationMS, error: error)
        }
    }
}

// ---------------------------------------------------------------------------
// Row merging — turns (from `buildTranscriptViewModel`) interleaved with the
// two standalone event kinds that model deliberately excludes
// ---------------------------------------------------------------------------

/// One rendered feed row: a full `TurnVM`, or a standalone `gate_requested`/
/// `run_complete` event — the same two variants the pre-Task-7 flat feed
/// rendered as its own rows, restyled in place (`GateRequestedRow`/
/// `RunCompleteRow` below) rather than removed.
enum FeedRow {
    case turn(TurnVM)
    case gate(gateID: String, prompt: String, decision: String?, decidedBy: String?)
    case runComplete(runID: String, status: String, totalTokens: UInt64, durationMS: UInt64, error: String?)
}

/// Merges `buildTranscriptViewModel`'s turn list back against the two
/// standalone event kinds it deliberately excludes from every `TurnVM`
/// (`gate_requested`, `run_complete` — see that function's own doc comment)
/// into one chronologically ordered row list. Not `private` — directly
/// `@Test`-able via `@testable import RupuRunDetail`, the same "testable
/// pure seam" convention `buildTranscriptViewModel` itself and
/// `styledInlineMarkdown` (`MarkdownView.swift`) establish.
///
/// **`run_complete` is always emitted last** — it's the transcript's own
/// terminal event; nothing is ever appended after it, on disk or mid live
/// tail, so no positional tracking is needed for it at all.
///
/// **`gate_requested` is positioned by a lightweight re-walk**, not a second
/// full pass through `buildTranscriptViewModel`'s own tool/audit FIFO-queue
/// machinery (duplicating that machinery here would risk silently drifting
/// from it as either side changes). This walk tracks only the much
/// narrower "is a turn currently open" signal — exactly the three
/// conditions that actually open one in the real builder: a `turn_start`
/// (turn-bounded transcripts), an `assistant_message` opening a synthetic
/// turn (fallback transcripts with no turn events at all), or any
/// `tool_call`/`tool_result`/`tool_audit` arriving while none is open
/// (`ensureTurn()`'s gap-turn case, present in both modes). A `turn_end`
/// closes it. Turns are dequeued from `buildTranscriptViewModel`'s own
/// output, in order, each time this walk crosses one of those boundaries.
///
/// **One accepted, documented gap**: the real builder's matched-`tool_audit`
/// case (its `tool_call` already recorded, possibly in an earlier,
/// already-closed turn) never itself calls `ensureTurn()` — but this walk,
/// lacking the real per-tool-name FIFO queue, can't distinguish "matched"
/// from "standalone" and so always treats a `tool_audit` arriving with no
/// turn open as a boundary. This never actually diverges on a well-formed
/// transcript: every audit for a call is emitted before that SAME call's
/// own turn closes (`TranscriptViewModel`'s own doc comment: "call A, call
/// B, audit A, result A, audit B, result B", all within one turn) — so a
/// matched audit is never encountered with no turn open in practice. Any
/// turn not yet dequeued once the walk ends (only possible if this gap
/// actually fired) is flushed before `run_complete` regardless, so a turn
/// is never silently dropped even then.
func buildFeedRows(events: [TranscriptEvent]) -> [FeedRow] {
    let turnVMs = buildTranscriptViewModel(events: events)
    let hasTurnEvents = events.contains {
        switch $0 {
        case .turnStart, .turnEnd: true
        default: false
        }
    }

    var rows: [FeedRow] = []
    var nextTurnIndex = 0
    var turnsOpenedSoFar = 0
    var turnOpen = false
    var runCompleteRow: FeedRow?

    func flushTurns(upTo target: Int) {
        let cap = min(target, turnVMs.count)
        while nextTurnIndex < cap {
            rows.append(.turn(turnVMs[nextTurnIndex]))
            nextTurnIndex += 1
        }
    }

    for event in events {
        switch event {
        case .turnStart:
            guard hasTurnEvents else { continue }
            turnsOpenedSoFar += 1
            turnOpen = true

        case .turnEnd:
            guard hasTurnEvents else { continue }
            turnOpen = false

        case .assistantMessage:
            if hasTurnEvents {
                if !turnOpen {
                    turnsOpenedSoFar += 1
                    turnOpen = true
                }
            } else {
                turnsOpenedSoFar += 1
                turnOpen = true
            }

        case .toolCall, .toolResult, .toolAudit:
            if !turnOpen {
                turnsOpenedSoFar += 1
                turnOpen = true
            }

        case .gateRequested(let gateID, let prompt, let decision, let decidedBy):
            flushTurns(upTo: turnsOpenedSoFar)
            rows.append(.gate(gateID: gateID, prompt: prompt, decision: decision, decidedBy: decidedBy))

        case .runComplete(let runID, let status, let totalTokens, let durationMS, let error):
            runCompleteRow = .runComplete(runID: runID, status: status, totalTokens: totalTokens, durationMS: durationMS, error: error)

        case .fileEdit, .commandRun, .actionEmitted, .runStart, .usage, .netFlow, .assistantDelta, .unknown:
            continue
        }
    }

    flushTurns(upTo: turnVMs.count)
    if let runCompleteRow {
        rows.append(runCompleteRow)
    }
    return rows
}

// ---------------------------------------------------------------------------
// TurnRowView — one collapsible turn (`Turn.tsx`)
// ---------------------------------------------------------------------------

/// One turn's collapsible row — direct port of the web's `Turn` component
/// (`crates/rupu-cp/web/src/components/transcript/Turn.tsx`).
///
/// Collapsed (the default for every turn but the last, `turn.
/// isOpenByDefault`): chevron + a ~100-char whitespace-flattened snippet of
/// `assistantText` (or an italic "no assistant message" placeholder) +
/// right-aligned pills — tool count (only when `> 0`), finding count (warn
/// tone, only when `> 0`), and a result pill (`ok`/`error`/`running`).
///
/// Expanded: the same header, then — an `assistant` `Eyebrow` + `MarkdownView`
/// (only when `assistantText` is non-empty), a collapsible `thinking`
/// disclosure (dim via `.opacity(0.8)`, a 2px `rupuBorder` left bar,
/// collapsed by default — mirrors `Turn.tsx`'s own dimming, which likewise
/// only ever dims via a wrapping opacity rather than overriding
/// `Markdown`'s own explicit per-block ink color), then each `ToolEntry`
/// dispatched to `FindingCard` (`.finding`) or `ToolCardView` (every other
/// kind — which itself further dispatches `.astGrep` to `AstGrepBodyView`,
/// `.diff` to `DiffView`, etc.).
private struct TurnRowView: View {
    let turn: TurnVM
    let sawRunComplete: Bool
    let runID: String?
    let host: String?
    let sourcePreviewStore: SourcePreviewStore?

    @State private var expanded: Bool

    init(turn: TurnVM, sawRunComplete: Bool, runID: String?, host: String?, sourcePreviewStore: SourcePreviewStore?) {
        self.turn = turn
        self.sawRunComplete = sawRunComplete
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
        self._expanded = State(initialValue: turn.isOpenByDefault)
    }

    private var result: TurnResult {
        if turn.hasError { return .error }
        return sawRunComplete ? .ok : .running
    }

    private var snippet: String { flattenedTurnSnippet(turn.assistantText) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if expanded {
                expandedBody
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                    .padding(.top, 2)
            }
        }
        .panelStyle(.innerCard)
    }

    private var header: some View {
        Button {
            expanded.toggle()
        } label: {
            HStack(spacing: 8) {
                Icon(.chevronDown, size: 11)
                    .foregroundStyle(Color.rupuMute)
                    .rotationEffect(.degrees(expanded ? 0 : -90))
                snippetText
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
                pillRow
            }
            .padding(10)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var snippetText: some View {
        if snippet.isEmpty {
            Text("no assistant message")
                .font(.noteText)
                .italic()
                .foregroundStyle(Color.rupuMute)
        } else {
            Text(snippet)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
        }
    }

    private var pillRow: some View {
        HStack(spacing: 6) {
            if turn.tools.count > 0 {
                IconCountPill(
                    icon: .settings,
                    text: "\(turn.tools.count) \(turn.tools.count == 1 ? "tool" : "tools")",
                    foreground: .rupuInk,
                    background: .rupuSurface
                )
            }
            if turn.findingCount > 0 {
                IconCountPill(
                    icon: .shieldAlert,
                    text: "\(turn.findingCount) \(turn.findingCount == 1 ? "finding" : "findings")",
                    foreground: .rupuWarn,
                    background: .rupuWarnBg
                )
            }
            Badge(result.label, tone: result.tone)
        }
    }

    @ViewBuilder
    private var expandedBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let assistantText = turn.assistantText, !assistantText.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Eyebrow("assistant")
                    MarkdownView(assistantText)
                }
            }
            if let thinking = turn.thinking, !thinking.isEmpty {
                ThinkingDisclosure(thinking: thinking)
            }
            ForEach(turn.tools) { entry in
                entryView(entry)
            }
        }
    }

    @ViewBuilder
    private func entryView(_ entry: ToolEntry) -> some View {
        if entry.kind == .finding {
            FindingCard(entry: entry, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        } else {
            ToolCardView(entry: entry, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        }
    }
}

/// A turn's whitespace-flattened assistant-text snippet — direct port of
/// `Turn.tsx`'s own `snippet()` (`content.replace(/\s+/g, ' ').trim()`, then
/// truncated to 99 chars + `…` past 100). Not `private` — `@Test`-able via
/// `@testable import RupuRunDetail`. Empty (never `nil`) for a `nil`/empty
/// `content` — `TurnRowView` itself renders the "no assistant message"
/// placeholder for that case, matching the web's own `snippet(content) ||
/// <em>no assistant message</em>` fallback.
func flattenedTurnSnippet(_ content: String?, limit: Int = 100) -> String {
    guard let content else { return "" }
    let flat = content
        .split(whereSeparator: { $0.isWhitespace })
        .joined(separator: " ")
    guard flat.count > limit else { return flat }
    return String(flat.prefix(limit - 1)) + "…"
}

/// A turn's result pill — `ok`/`error`/`running`, ported from `Turn.tsx`'s
/// `RESULT_PILL`/`RESULT_LABEL` tables. `.running`'s amber tone is the web's
/// own literal choice (`bg-warn-bg text-warn`), not this app's blue
/// `StatusTone.running` — kept as the closer web-parity read since a turn's
/// "still going" state is presentation-distinct from a run's own `running`
/// status tone elsewhere in this app.
private enum TurnResult {
    case ok, error, running

    var label: String {
        switch self {
        case .ok: "ok"
        case .error: "error"
        case .running: "running"
        }
    }

    var tone: Color {
        switch self {
        case .ok: .rupuOk
        case .error: .rupuErr
        case .running: .rupuWarn
        }
    }
}

/// A small tone-tinted pill combining an `Icon` with a text label — `Badge`'s
/// own chrome (mono text, `ChromeShape.pill`, 6/2 padding) plus a leading
/// icon `Badge` itself doesn't support. Scoped to this file; `Turn.tsx`'s
/// tool-count/finding-count pills are this component's only two call sites.
private struct IconCountPill: View {
    let icon: LucideIcon
    let text: String
    let foreground: Color
    let background: Color

    var body: some View {
        HStack(spacing: 3) {
            Icon(icon, size: 9)
            Text(text).font(.dataMono(10))
        }
        .foregroundStyle(foreground)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(background)
        .clipShape(ChromeShape.pill)
    }
}

/// The `thinking` disclosure — collapsed by default, a chevron + `metaText`
/// "thinking" label toggling a `MarkdownView` rendering dimmed via
/// `.opacity(0.8)` behind a 2px `rupuBorder` left bar. Mirrors `Turn.tsx`'s
/// own `showThinking` toggle and its `border-l-2 border-border pl-2 text-
/// ink-mute opacity-80` wrapper — including the fact that the opacity, not a
/// foreground-color override, is what actually dims the rendered markdown:
/// both this view's `MarkdownView` and the web's own `<Markdown>` set each
/// block's ink color explicitly, so a wrapping color modifier alone
/// wouldn't reach the text; the shared opacity does, in both.
private struct ThinkingDisclosure: View {
    let thinking: String

    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                expanded.toggle()
            } label: {
                HStack(spacing: 4) {
                    Icon(.chevronDown, size: 10)
                        .rotationEffect(.degrees(expanded ? 0 : -90))
                    Text("thinking")
                        .font(.metaText)
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.rupuMute)

            if expanded {
                HStack(alignment: .top, spacing: 8) {
                    Rectangle()
                        .fill(Color.rupuBorder)
                        .frame(width: 2)
                    MarkdownView(thinking)
                }
                .opacity(0.8)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Standalone rows — `gate_requested` / `run_complete` (restyled in place)
// ---------------------------------------------------------------------------

/// A parked (or since-decided) gate — restyled onto `TintBanner` (an
/// awaiting-tone callout) rather than the old bespoke left-edge-bar
/// treatment. Same fields, same content as before this task.
private struct GateRequestedRow: View {
    let gateID: String
    let prompt: String
    let decision: String?
    let decidedBy: String?

    var body: some View {
        TintBanner(tone: Color.status(.awaiting), toneBg: Color.status(.awaiting).opacity(0.08)) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Eyebrow("Gate")
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
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// The transcript's terminal event — restyled onto `TintBanner` in the
/// run's own outcome tone (`done`/`failed`) rather than the old bespoke
/// panel treatment. Same fields, same content as before this task.
private struct RunCompleteRow: View {
    let runID: String
    let status: String
    let totalTokens: UInt64
    let durationMS: UInt64
    let error: String?

    private var tone: StatusTone { status == "ok" ? .done : .failed }

    var body: some View {
        TintBanner(tone: Color.status(tone), toneBg: Color.status(tone).opacity(0.08)) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Circle().fill(Color.status(tone)).frame(width: 6, height: 6)
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
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
