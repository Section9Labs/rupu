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
/// **Every other event kind renders as its own row too** (transcript-
/// fidelity v2, Plan 3 Task 2 — spec §5's display parity matrix: "no silent
/// drops anywhere"). `thinking`/`user_message`/`seed`/`notice`/`compaction`
/// and a forward-compat `unknown(type:)` are transcript-level narrative
/// events, not turn-scoped tool activity — `buildFeedRows` positions each as
/// a standalone row chronologically among the turns, the same way
/// `gate_requested` always has (see that function's doc comment). `turn_end`
/// likewise gains its own `turnSeparator` row surfacing that turn's
/// `tokens_in`/`tokens_out`, previously computed onto `TurnVM` but never
/// displayed anywhere.
///
/// **Only three kinds are deliberately still never rendered, and this is the
/// complete list**: `assistant_delta`/`thinking_delta` (review fix, web-
/// viewer parity — the transcript JSONL persists both the per-chunk delta
/// events *and* the consolidated `assistant_message`/`thinking` event for
/// the same content, so rendering both would show it twice), `run_start`/
/// `usage` (carry no per-row content this phase), and `net_flow` (the
/// Netflow tab's own concern, never the transcript feed's). `tool_audit`/
/// `action_emitted`/`file_edit`/`command_run` are NOT top-level rows either,
/// but they are not silently dropped: `buildTranscriptViewModel` folds each
/// onto its owning (or, for an audit/action-node call with none, a
/// synthesized standalone) `ToolEntry`, rendered as part of that entry's own
/// `ToolCardView`/`FindingCard` — an `AuditBadge` in the card header, the
/// action's merged payload in the card body, the diff/command in their
/// respective kind bodies — never emitted as a second, separate row.
public struct TranscriptFeed: View {
    private let events: [TranscriptEvent]
    private let runID: String?
    private let host: String?
    private let sourcePreviewStore: SourcePreviewStore?
    /// Plan 3, Task 2: how many trailing transcript lines the server
    /// dropped as unparseable (`APITranscriptPage.unparsed`) — defaulted
    /// `0` so no existing call site needed to change to keep compiling.
    /// Threaded from `RunDetailStore.transcriptUnparsedCount` by
    /// `TranscriptTabContent`; `SessionDetailScreen`/`AgentRunDetailScreen`
    /// don't hold an `APITranscriptPage` today, so they keep the default.
    private let unparsedCount: Int
    private let computeRows: ([TranscriptEvent]) -> [FeedRow]

    /// Perf & interaction arc, Plan 5 Task 3: `rows`/`sawRunComplete` used to
    /// be computed properties — `buildFeedRows` ran TWICE per body (once
    /// where `rowView`'s `ForEach` read `rows`, once more where `rows.
    /// isEmpty` read it again), and `sawRunComplete`'s own full `events.
    /// contains` scan ran once per body pass too, on top of that. Both are
    /// now `@State`, computed once at init (the initial mount) and
    /// recomputed exactly once per `.onChange(of: events.count)` firing —
    /// SwiftUI's own contract for that modifier ("fires only when the
    /// observed value actually changed") is what turns "once per distinct
    /// event-count" into "not once per body pass".
    ///
    /// Default (internal) access, not `private` — reached directly from
    /// `TranscriptFeedRecomputeTests` via `@testable import RupuRunDetail`
    /// (this test target has no SwiftUI view-hosting harness to render the
    /// body and observe `onChange`/`onAppear` firing instead).
    @State var rows: [FeedRow]
    @State var sawRunComplete: Bool

    public init(events: [TranscriptEvent], runID: String? = nil, host: String? = nil, sourcePreviewStore: SourcePreviewStore? = nil, unparsedCount: Int = 0) {
        self.init(events: events, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore, unparsedCount: unparsedCount, computeRows: buildFeedRows)
    }

    /// Test-only seam (default `internal`, reached via `@testable import
    /// RupuRunDetail`): `computeRows` is injectable so
    /// `TranscriptFeedRecomputeTests` can wrap `buildFeedRows` with a
    /// call-counting spy and assert the "computed once per event, not once
    /// per body pass" contract directly, without a SwiftUI view-hosting
    /// harness (this test target has none).
    init(
        events: [TranscriptEvent],
        runID: String?,
        host: String?,
        sourcePreviewStore: SourcePreviewStore?,
        unparsedCount: Int = 0,
        computeRows: @escaping ([TranscriptEvent]) -> [FeedRow]
    ) {
        self.events = events
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
        self.unparsedCount = unparsedCount
        self.computeRows = computeRows
        self._rows = State(initialValue: computeRows(events))
        self._sawRunComplete = State(initialValue: Self.sawRunComplete(in: events))
    }

    /// Whether the WHOLE transcript has reached its terminal `run_complete`
    /// event — every turn's result pill reads `.ok` once this is true
    /// (unless that turn itself `hasError`), `.running` otherwise. Ported
    /// from the web's own `sawRunComplete` (`transcriptView.ts:476`), which
    /// is likewise a transcript-global flag, not a per-turn one.
    private static func sawRunComplete(in events: [TranscriptEvent]) -> Bool {
        events.contains {
            if case .runComplete = $0 { return true }
            return false
        }
    }

    public var body: some View {
        // RenderMeter seam (Plan 5, Task 1) — one line, safe to delete.
        let _ = RenderMeter.tick("TranscriptFeed")
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if unparsedCount > 0 {
                        MetaLineRow(
                            label: "warning",
                            detail: "\(unparsedCount) transcript line\(unparsedCount == 1 ? "" : "s") could not be parsed (older app or newer rupu)"
                        )
                    }
                    ForEach(rows) { row in
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
                rows = computeRows(events)
                sawRunComplete = Self.sawRunComplete(in: events)
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
        case .turnSeparator(let turnIdx, let tokensIn, let tokensOut, _):
            TurnSeparatorRow(turnIdx: turnIdx, tokensIn: tokensIn, tokensOut: tokensOut)
        case .thinking(let text, _):
            ThinkingRow(text: text)
        case .userMessage(let content, _):
            UserPromptRow(content: content)
        case .seed(let messageCount, let sourceTranscript, _):
            MetaLineRow(label: "seed", detail: Self.seedDetail(messageCount: messageCount, sourceTranscript: sourceTranscript))
        case .notice(let kind, let message, _):
            MetaLineRow(label: kind, detail: message)
        case .compaction(let seq, let summarizedMessages, _):
            MetaLineRow(label: "compaction", detail: "seq \(seq) · summarized \(summarizedMessages) messages")
        case .unknownEvent(let type, _):
            MetaLineRow(label: "event", detail: "unrecognized: \(type)")
        }
    }

    /// `seed`'s detail line — names the source transcript's own filename
    /// (not its full path, which is a local disk path with no meaning to
    /// the reader) when the seed chains onto one, per spec §3's "reference,
    /// not full embed" seed-dedup rule.
    private static func seedDetail(messageCount: UInt32, sourceTranscript: String?) -> String {
        guard let sourceTranscript else { return "\(messageCount) prior messages seed this run" }
        return "\(messageCount) prior messages seed this run · from \((sourceTranscript as NSString).lastPathComponent)"
    }
}

// ---------------------------------------------------------------------------
// Row merging — turns (from `buildTranscriptViewModel`) interleaved with the
// two standalone event kinds that model deliberately excludes
// ---------------------------------------------------------------------------

/// One rendered feed row: a full `TurnVM`, a standalone `gate_requested`/
/// `run_complete` event — the same two variants the pre-Task-7 flat feed
/// rendered as its own rows, restyled in place (`GateRequestedRow`/
/// `RunCompleteRow` below) rather than removed — or (Plan 3, Task 2) one of
/// the six transcript-fidelity v2 narrative events `buildTranscriptViewModel`
/// excludes from every `TurnVM` (see that function's doc comment), plus a
/// `turnSeparator` surfacing `turn_end`'s own `tokens_in`/`tokens_out` (spec
/// §5's "new turn separator row" cell) and an `unknownEvent` for a
/// forward-compat `.unknown(type:)` decode.
enum FeedRow: Identifiable {
    case turn(TurnVM)
    case gate(gateID: String, prompt: String, decision: String?, decidedBy: String?)
    case runComplete(runID: String, status: String, totalTokens: UInt64, durationMS: UInt64, error: String?)
    case turnSeparator(turnIdx: UInt32, tokensIn: UInt64?, tokensOut: UInt64?, rowIndex: Int)
    case thinking(text: String?, rowIndex: Int)
    case userMessage(content: String, rowIndex: Int)
    case seed(messageCount: UInt32, sourceTranscript: String?, rowIndex: Int)
    case notice(kind: String, message: String, rowIndex: Int)
    case compaction(seq: UInt32, summarizedMessages: UInt32, rowIndex: Int)
    case unknownEvent(type: String, rowIndex: Int)

    /// Perf & interaction arc, Plan 5 Task 3: lets `TranscriptFeed`'s
    /// `ForEach` key on stable row identity instead of positional
    /// `\.offset` — a row's underlying turn/gate/run id never changes
    /// across a recompute, so SwiftUI can diff/reuse rather than treating
    /// every recompute as an entirely new row list. `TurnVM.id` is already
    /// unique per transcript, including negative synthetic "gap turn" ids
    /// (see that type's own doc comment); `gateID`/`runID` are likewise
    /// unique per their own event kind. Kind-prefixed so the three variants
    /// can never collide with each other even if their underlying ids ever
    /// happened to share a value.
    ///
    /// The six new variants have no inherent unique identifier of their own
    /// (unlike `gateID`/`runID`) — more than one `notice`, for instance, can
    /// appear in the same transcript — so each carries its own position in
    /// the source `events` array (`rowIndex`, assigned once by
    /// `buildFeedRows`'s `enumerated()` walk) purely to make this id stable
    /// and unique across an unchanged transcript, the same role a real id
    /// plays for the other variants.
    var id: String {
        switch self {
        case .turn(let turn): "turn:\(turn.id)"
        case .gate(let gateID, _, _, _): "gate:\(gateID)"
        case .runComplete(let runID, _, _, _, _): "runComplete:\(runID)"
        case .turnSeparator(_, _, _, let rowIndex): "turnSeparator:\(rowIndex)"
        case .thinking(_, let rowIndex): "thinking:\(rowIndex)"
        case .userMessage(_, let rowIndex): "userMessage:\(rowIndex)"
        case .seed(_, _, let rowIndex): "seed:\(rowIndex)"
        case .notice(_, _, let rowIndex): "notice:\(rowIndex)"
        case .compaction(_, _, let rowIndex): "compaction:\(rowIndex)"
        case .unknownEvent(_, let rowIndex): "unknown:\(rowIndex)"
        }
    }
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
///
/// The identical gap, and the identical "never fires on a well-formed
/// transcript" reasoning, applies to a MATCHED `tool_result` too: the real
/// builder's matched branch (`toolByCallID[callID]` found) only mutates the
/// existing `ToolBuilder` in place and likewise never calls `ensureTurn()`,
/// while this walk again can't distinguish "matched" from "orphan" and
/// would treat a `tool_result` arriving with no turn open as a boundary
/// either way. A call's own result is always recorded before that same
/// call's turn closes (the same ordering guarantee the audit case relies
/// on), so a matched result is likewise never encountered with no turn
/// open in practice.
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

    /// Review fix (critical): flushes only turns that have already
    /// CLOSED — never the turn currently open. `turnsOpenedSoFar` counts a
    /// turn as soon as its `turn_start` arrives, before its `turn_end`, so
    /// flushing straight to `turnsOpenedSoFar` (as every other flush site
    /// here does) would treat a still-open turn as flushable. That is
    /// exactly wrong for a narrative event: the runner's emission-order
    /// contract writes `Thinking` right after `turn_start` and BEFORE that
    /// turn's own `assistant_message`/`tool_call` — the normal case, not an
    /// edge case — so a `thinking` (or `notice`/`seed`/etc.) arriving while
    /// its enclosing turn is still open must render BEFORE that turn's
    /// card, never after it. Subtracting one open turn, when one is open,
    /// defers that turn's card until something else flushes it for real —
    /// its own `turn_end` (which flushes unadjusted, right before that
    /// turn's `turnSeparator`), a later `gate_requested`/`run_complete`
    /// (which also flush unadjusted — a gate/run-complete legitimately
    /// follows a turn's activity, so holding it back would be wrong), or
    /// the final catch-all flush at the end of the walk.
    ///
    /// Only used by the six narrative-event cases below — never by
    /// `turn_end`'s own flush (which must include the turn it's closing)
    /// or `gate_requested`'s (a gate is a pause point that follows a
    /// turn's activity, not content the turn produced, so it keeps the
    /// old "flush everything opened so far" contract).
    func flushClosedTurnsOnly() {
        flushTurns(upTo: turnsOpenedSoFar - (turnOpen ? 1 : 0))
    }

    for (index, event) in events.enumerated() {
        switch event {
        case .turnStart:
            guard hasTurnEvents else { continue }
            turnsOpenedSoFar += 1
            turnOpen = true

        case .turnEnd(let turnIdx, let tokensIn, let tokensOut):
            guard hasTurnEvents else { continue }
            turnOpen = false
            // The just-closed turn was already counted into
            // `turnsOpenedSoFar` when its `turn_start` opened it — flushing
            // up to that count here always includes it, so the separator
            // lands directly after its own turn row, never ahead of it.
            flushTurns(upTo: turnsOpenedSoFar)
            rows.append(.turnSeparator(turnIdx: turnIdx, tokensIn: tokensIn, tokensOut: tokensOut, rowIndex: index))

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

        // Transcript-fidelity v2 (Plan 3, Task 2): transcript-level
        // narrative events, not turn-scoped tool activity — each flushes
        // only turns that have already closed (`flushClosedTurnsOnly` —
        // see its own doc comment for why: a narrative event arriving
        // while its enclosing turn is still open, the normal case for
        // `thinking`, must render BEFORE that turn's card), then appends
        // this row where it chronologically occurred. None of these ever
        // open/close a turn themselves.
        case .thinking(let text, _, _):
            flushClosedTurnsOnly()
            rows.append(.thinking(text: text, rowIndex: index))

        case .userMessage(let content):
            flushClosedTurnsOnly()
            rows.append(.userMessage(content: content, rowIndex: index))

        case .seed(let messageCount, let sourceTranscript):
            flushClosedTurnsOnly()
            rows.append(.seed(messageCount: messageCount, sourceTranscript: sourceTranscript, rowIndex: index))

        case .notice(let kind, let message):
            flushClosedTurnsOnly()
            rows.append(.notice(kind: kind, message: message, rowIndex: index))

        case .compaction(let seq, let summarizedMessages, _):
            flushClosedTurnsOnly()
            rows.append(.compaction(seq: seq, summarizedMessages: summarizedMessages, rowIndex: index))

        // Forward-compat fallback (an unrecognized wire `type`) — rendered,
        // never dropped, same "no event may vanish" contract as every other
        // variant here.
        case .unknown(let type):
            flushClosedTurnsOnly()
            rows.append(.unknownEvent(type: type, rowIndex: index))

        // `file_edit`/`command_run` are adjacency-paired onto their owning
        // `ToolEntry` by `buildTranscriptViewModel` and rendered as part of
        // that entry's card — never their own row. `action_emitted` is
        // merged onto a standalone audit entry the same way. `run_start`/
        // `usage` carry no per-row content this phase. `assistant_delta`/
        // `thinking_delta` are dropped outright — the consolidated
        // `assistant_message`/`thinking` event for the same content already
        // renders; showing both would duplicate it.
        case .fileEdit, .commandRun, .actionEmitted, .runStart, .usage, .netFlow, .assistantDelta, .thinkingDelta:
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

// ---------------------------------------------------------------------------
// Transcript-fidelity v2 rows (Plan 3, Task 2) — the six narrative events
// `buildTranscriptViewModel` excludes from every `TurnVM`, plus the
// `turn_end` separator. Each mirrors this file's existing standalone-row
// idiom (`.panelStyle(.innerCard)`, `Eyebrow`, `Color.rupu*`) rather than
// introducing new chrome.
// ---------------------------------------------------------------------------

/// A standalone `thinking` event — full reasoning block in its true
/// transcript position, collapsed by default (spec §5: "dim DisclosureGroup
/// row in true position"). `text == nil` (or empty) is a redacted/omitted
/// reasoning block, per spec §3's `text: Option<String>` contract — shown as
/// a plain italic marker rather than an empty, uselessly-toggleable
/// disclosure.
private struct ThinkingRow: View {
    let text: String?
    @State private var expanded = false

    var body: some View {
        Group {
            if let text, !text.isEmpty {
                DisclosureGroup(isExpanded: $expanded) {
                    Text(text)
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                        .padding(.top, 4)
                        .textSelection(.enabled)
                } label: {
                    Eyebrow("Thinking")
                }
            } else {
                Text("[redacted reasoning]")
                    .font(.noteText)
                    .italic()
                    .foregroundStyle(Color.rupuMute)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

/// A standalone `user_message` event — the turn-opening prompt, rendered at
/// full weight (`leadText`, `rupuInk`) rather than the dimmer treatment
/// every other narrative row here uses, since this is the one row a reader
/// is most likely to actually want to read in full.
private struct UserPromptRow: View {
    let content: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("Prompt")
            Text(content)
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

/// Shared one-line label + detail treatment for `seed`/`notice`/
/// `compaction`/`unknown(type:)` — and the unparsed-lines warning banner
/// (`TranscriptFeed.body`) — none of which need more than a label and a
/// short line of context.
struct MetaLineRow: View {
    let label: String
    let detail: String

    var body: some View {
        HStack(spacing: 8) {
            Text(label).font(.metaText).foregroundStyle(Color.rupuDim)
            Text(detail)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

/// A `turn_end` boundary — spec §5's "new turn separator row" cell,
/// surfacing that turn's `tokens_in`/`tokens_out` (computed onto `TurnVM`
/// by `buildTranscriptViewModel` but, before this task, never actually
/// displayed anywhere). Renders with no token suffix when the transcript's
/// `turn_end` didn't carry them (both fields stay optional on the wire).
private struct TurnSeparatorRow: View {
    let turnIdx: UInt32
    let tokensIn: UInt64?
    let tokensOut: UInt64?

    var body: some View {
        HStack(spacing: 8) {
            Rectangle().fill(Color.rupuMute.opacity(0.25)).frame(height: 1)
            Text("turn \(turnIdx)\(tokenSuffix)")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .fixedSize()
            Rectangle().fill(Color.rupuMute.opacity(0.25)).frame(height: 1)
        }
    }

    private var tokenSuffix: String {
        guard let tokensIn, let tokensOut else { return "" }
        return " · \(Fmt.count(Int(tokensIn))) in · \(Fmt.count(Int(tokensOut))) out"
    }
}
