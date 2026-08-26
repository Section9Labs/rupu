import RupuAPI

/// Pure event -> view-model pairing for the transcript panel's richer,
/// turn-folded rendering (Plan 4's replacement for `TranscriptFeed`'s flat
/// row list). Line-for-line semantics port of `crates/rupu-cp/web/src/
/// components/transcript/transcriptView.ts` (cited by line number below),
/// with ONE deliberate deviation called out in "Turn folding" below.
///
/// No I/O, no clock, no SwiftUI/Observation dependency — a deterministic
/// function over an event array, directly `@Test`-able.
///
/// ## Pairing rules (ported)
/// - `tool_result` pairs to its `tool_call` by `call_id` (`transcriptView.ts`
///   L343-357). **Deviation from the web**: the web silently drops an
///   unpaired `tool_result` ("An unpaired result carries no tool_call to
///   render against; ignore.", L355). This port instead surfaces it as a
///   standalone entry (empty `tool` name, `.generic` kind) so a truncated
///   snapshot's tail result is never silently lost — the controller's
///   explicit instruction for this task.
/// - `file_edit` pairs by ADJACENCY onto the nearest preceding unpaired
///   `write_file`/`edit_file` (`.diff`-kind) call (L326-328, L389-399).
/// - `command_run` pairs by ADJACENCY onto the nearest preceding unpaired
///   `bash` (`.terminal`-kind) call (L328, L401-413).
/// - `tool_audit` is matched by a FIFO queue keyed on the tool NAME
///   (L258-268, L329-337, L359-387) — deliberately NOT adjacency: a single
///   turn can carry more than one `tool_use` block, and `run_agent` writes
///   every `tool_call` for the turn before dispatching any of them, so on
///   disk the order is `call A, call B, audit A, result A, audit B,
///   result B`. An adjacency/last-call-wins scheme would misattach A's
///   audit to B. Matching by name against a FIFO queue gets each audit onto
///   the correct call even when the same tool is called twice in one turn.
///   When the queue for that name is empty (an action-node call has no
///   `tool_call`/`tool_result` shape at all), the audit surfaces as a
///   standalone entry (L369-385) so it's never silently dropped.
/// - `action_emitted` (the action-node shape, `{kind, payload}`) is stashed
///   in a FIFO queue keyed by `kind` (the tool name) and merged onto the
///   standalone entry its matching `tool_audit` creates (L437-458) — one
///   merged entry, never two. Ported here as a dedicated `actionPayload`
///   field on `ToolEntry` (kept separate from `input`, which stays `.null`
///   for a call-less action node) rather than overwriting `input` the way
///   the web does, per the brief's explicit field contract.
/// - `ToolKind` classification mirrors `classify(tool:)` (L214-237) by tool
///   name, verified against the source rather than paraphrased:
///   `report_finding` -> finding, `read_file` -> read, `grep` -> grep,
///   `glob` -> glob, `write_file`/`edit_file` -> diff, `bash` -> terminal,
///   `dispatch_agent`/`dispatch_agents_parallel` -> subrun, `ast_grep` ->
///   astGrep, a `coverage_`-prefixed name -> coverage, else generic.
///
/// ## Turn folding (the one deliberate deviation)
/// The web (`transcriptView.ts` L293-307) starts a new turn at every
/// `assistant_message` and ignores `turn_start`/`turn_end` entirely (its own
/// comment, L461-462: "turn_start / turn_end / ... carry no render
/// payload. ... ignored gracefully."). This port instead prefers the
/// explicit `turn_start`/`turn_end` boundaries when the transcript carries
/// them (a turn opens at `turn_start(idx)`, and closes — capturing
/// `tokens_in`/`tokens_out` — at the matching `turn_end`), falling back to
/// "each `assistant_message` opens a synthetic turn" only when the
/// transcript has NEITHER event at all. Tool-bearing events that arrive
/// while no turn is currently open in turn-bounded mode — before the
/// first `turn_start`, or in a second (or later) gap between one
/// `turn_end` and the next `turn_start` — fall into a "gap" turn with a
/// synthetic negative id, the same "leading turn with no assistant" shape
/// the web's own `ensureTurn()` produces for pre-first-assistant tools.
/// Each gap gets its OWN id (-1, -2, -3, ...) via a decrementing counter,
/// never a fixed `-1` reused across gaps — a review fix: a shared
/// sentinel produced duplicate ids across two non-adjacent gaps, which
/// breaks SwiftUI `ForEach` identity on the `Identifiable` result array.
///
/// Excluded from `TurnVM` entirely (per the task brief): `assistant_delta`
/// (the turn's consolidated `assistant_message` already carries the same
/// text), `net_flow`, `usage`, `run_start`/`run_complete` (header/footer
/// concerns), and `gate_requested` (a standalone feed row, Task 7's
/// concern) — none of these five ever open, close, or populate a turn.
/// `unknown(type:)` is likewise ignored.
///
/// The LAST turn in the returned array has `isOpenByDefault == true`; every
/// earlier turn has it `false`.
public enum ToolKind: String, Equatable, Sendable {
    case finding, read, grep, glob, diff, terminal, subrun, coverage, astGrep, generic
}

public struct ToolEntry: Identifiable, Equatable, Sendable {
    public struct FileEdit: Equatable, Sendable {
        public let path: String
        public let kind: String
        public let diff: String
    }

    public struct Command: Equatable, Sendable {
        public let argv: [String]
        public let cwd: String
        public let exitCode: Int32
    }

    public struct Audit: Equatable, Sendable {
        public let declared: Bool
        public let granted: Bool
        public let blocked: Bool
        public let restricted: Bool
    }

    /// `call_id` for a paired/orphaned tool_call or tool_result; synthesized
    /// (`"audit-<tool>-<n>"`) for a standalone `tool_audit` entry that never
    /// had a `tool_call` of its own.
    public let id: String
    public let tool: String
    public let kind: ToolKind
    public let input: JSONValue
    public let output: String?
    public let errorText: String?
    public let durationMS: UInt64?
    public let structured: JSONValue?
    /// Adjacency-paired from the nearest preceding unpaired `.diff`-kind call.
    public let fileEdit: FileEdit?
    /// Adjacency-paired from the nearest preceding unpaired `.terminal`-kind call.
    public let command: Command?
    public let audit: Audit?
    /// The rendered `with:` args from a matching `action_emitted`, merged
    /// onto a standalone audit entry only (`nil` for every paired call).
    public let actionPayload: JSONValue?

    public init(
        id: String,
        tool: String,
        kind: ToolKind,
        input: JSONValue,
        output: String? = nil,
        errorText: String? = nil,
        durationMS: UInt64? = nil,
        structured: JSONValue? = nil,
        fileEdit: FileEdit? = nil,
        command: Command? = nil,
        audit: Audit? = nil,
        actionPayload: JSONValue? = nil
    ) {
        self.id = id
        self.tool = tool
        self.kind = kind
        self.input = input
        self.output = output
        self.errorText = errorText
        self.durationMS = durationMS
        self.structured = structured
        self.fileEdit = fileEdit
        self.command = command
        self.audit = audit
        self.actionPayload = actionPayload
    }
}

public struct TurnVM: Identifiable, Equatable, Sendable {
    /// The real `turn_idx` when turn events bound this turn; a synthesized
    /// index (starting at 0, or negative for a leading pre-turn-start turn)
    /// otherwise. See the type's file-header doc comment for the fallback
    /// rule.
    public let id: Int
    public let assistantText: String?
    public let thinking: String?
    public let tools: [ToolEntry]
    public let findingCount: Int
    public let hasError: Bool
    public let isOpenByDefault: Bool
    public let tokensIn: UInt64?
    public let tokensOut: UInt64?

    public init(
        id: Int,
        assistantText: String?,
        thinking: String?,
        tools: [ToolEntry],
        findingCount: Int,
        hasError: Bool,
        isOpenByDefault: Bool,
        tokensIn: UInt64?,
        tokensOut: UInt64?
    ) {
        self.id = id
        self.assistantText = assistantText
        self.thinking = thinking
        self.tools = tools
        self.findingCount = findingCount
        self.hasError = hasError
        self.isOpenByDefault = isOpenByDefault
        self.tokensIn = tokensIn
        self.tokensOut = tokensOut
    }
}

// ---------------------------------------------------------------------------
// Classification (transcriptView.ts L214-237)
// ---------------------------------------------------------------------------

private func classify(_ tool: String) -> ToolKind {
    switch tool {
    case "report_finding": return .finding
    case "read_file": return .read
    case "grep": return .grep
    case "glob": return .glob
    case "write_file", "edit_file": return .diff
    case "bash": return .terminal
    case "dispatch_agent", "dispatch_agents_parallel": return .subrun
    case "ast_grep": return .astGrep
    default: return tool.hasPrefix("coverage_") ? .coverage : .generic
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Mutable working copy of a `ToolEntry` while later events (`tool_result`,
/// `file_edit`, `command_run`, `tool_audit`) are still arriving. Reference
/// type so the FIFO/adjacency queues below can hold a shared handle and
/// mutate it in place; converted to the immutable `ToolEntry` at the end.
private final class ToolBuilder {
    let id: String
    let tool: String
    let kind: ToolKind
    var input: JSONValue
    var output: String?
    var errorText: String?
    var durationMS: UInt64?
    var structured: JSONValue?
    var fileEdit: ToolEntry.FileEdit?
    var command: ToolEntry.Command?
    var audit: ToolEntry.Audit?
    var actionPayload: JSONValue?

    init(id: String, tool: String, kind: ToolKind, input: JSONValue) {
        self.id = id
        self.tool = tool
        self.kind = kind
        self.input = input
    }

    func build() -> ToolEntry {
        ToolEntry(
            id: id, tool: tool, kind: kind, input: input,
            output: output, errorText: errorText, durationMS: durationMS, structured: structured,
            fileEdit: fileEdit, command: command, audit: audit, actionPayload: actionPayload
        )
    }
}

private final class TurnBuilder {
    let id: Int
    var assistantText: String?
    var thinking: String?
    var tools: [ToolBuilder] = []
    var tokensIn: UInt64?
    var tokensOut: UInt64?

    init(id: Int) {
        self.id = id
    }
}

public func buildTranscriptViewModel(events: [TranscriptEvent]) -> [TurnVM] {
    let hasTurnEvents = events.contains {
        switch $0 {
        case .turnStart, .turnEnd: return true
        default: return false
        }
    }

    var turns: [TurnBuilder] = []
    var current: TurnBuilder?
    var nextSyntheticID = 0
    var standaloneAuditCounter = 0
    // Decrementing counter for "gap" turns — content (a standalone audit,
    // an orphan tool_result, a stray tool_call) that arrives while no turn
    // is open: before the first turn_start, or in a SECOND (or later) gap
    // between one turn_end and the next turn_start. A fixed `-1` sentinel
    // here previously collided across multiple gaps (review fix: two
    // non-adjacent gaps both produced `id == -1`, a duplicate id in the
    // `Identifiable` result array that breaks SwiftUI ForEach identity).
    // Each NEW gap turn now takes the next id down (-1, -2, -3, ...),
    // guaranteed distinct from every other gap turn and from any real
    // (>= 0) turn_idx.
    var nextGapID = -1

    func ensureTurn() -> TurnBuilder {
        if let current { return current }
        let t = TurnBuilder(id: nextGapID)
        nextGapID -= 1
        turns.append(t)
        current = t
        return t
    }

    var toolByCallID: [String: ToolBuilder] = [:]
    var pendingDiff: ToolBuilder?
    var pendingTerminal: ToolBuilder?
    var pendingAuditsByTool: [String: [ToolBuilder]] = [:]
    var pendingActionArgsByTool: [String: [JSONValue]] = [:]

    for event in events {
        switch event {
        // Excluded from turns entirely — see the file-header doc comment.
        case .runStart, .runComplete, .usage, .netFlow, .assistantDelta, .gateRequested, .unknown:
            continue

        case .turnStart(let turnIdx):
            guard hasTurnEvents else { continue }
            let t = TurnBuilder(id: Int(turnIdx))
            turns.append(t)
            current = t

        case .turnEnd(let turnIdx, let tokensIn, let tokensOut):
            guard hasTurnEvents else { continue }
            if let current, current.id == Int(turnIdx) {
                current.tokensIn = tokensIn
                current.tokensOut = tokensOut
            }
            // Reset so any stray event between this turn_end and the next
            // turn_start attaches to a fresh leading turn rather than
            // silently re-joining the just-closed one.
            current = nil

        case .assistantMessage(let content, let thinking):
            if hasTurnEvents {
                let t = ensureTurn()
                t.assistantText = content
                t.thinking = thinking
            } else {
                // Fallback mode: each assistant_message opens its own
                // synthetic turn (tools already queued in a leading turn,
                // if any, stay there — only a NEW turn starts here).
                let t = TurnBuilder(id: nextSyntheticID)
                nextSyntheticID += 1
                t.assistantText = content
                t.thinking = thinking
                turns.append(t)
                current = t
            }

        case .toolCall(let callID, let tool, let input):
            let kind = classify(tool)
            let tb = ToolBuilder(id: callID, tool: tool, kind: kind, input: input)
            toolByCallID[callID] = tb
            pendingDiff = kind == .diff ? tb : nil
            pendingTerminal = kind == .terminal ? tb : nil
            pendingAuditsByTool[tool, default: []].append(tb)
            ensureTurn().tools.append(tb)

        case .toolResult(let callID, let output, let error, let durationMS, let structured):
            if let tb = toolByCallID[callID] {
                tb.output = output
                tb.errorText = error
                tb.durationMS = durationMS
                tb.structured = structured
            } else {
                // Orphan result — deliberately surfaced as a standalone
                // entry rather than dropped; see the type doc comment's
                // "Deviation from the web" note.
                let tb = ToolBuilder(id: callID, tool: "", kind: .generic, input: .null)
                tb.output = output
                tb.errorText = error
                tb.durationMS = durationMS
                tb.structured = structured
                ensureTurn().tools.append(tb)
            }

        case .toolAudit(let tool, let declared, let granted, let blocked, let restricted):
            let audit = ToolEntry.Audit(declared: declared, granted: granted, blocked: blocked, restricted: restricted)
            if var queue = pendingAuditsByTool[tool], !queue.isEmpty {
                let target = queue.removeFirst()
                pendingAuditsByTool[tool] = queue
                target.audit = audit
            } else {
                var argQueue = pendingActionArgsByTool[tool] ?? []
                let payload = argQueue.isEmpty ? nil : argQueue.removeFirst()
                pendingActionArgsByTool[tool] = argQueue
                standaloneAuditCounter += 1
                let tb = ToolBuilder(id: "audit-\(tool)-\(standaloneAuditCounter)", tool: tool, kind: classify(tool), input: .null)
                tb.audit = audit
                tb.actionPayload = payload
                ensureTurn().tools.append(tb)
            }

        case .fileEdit(let path, let kind, let diff):
            if let pendingDiff {
                pendingDiff.fileEdit = ToolEntry.FileEdit(path: path, kind: kind, diff: diff)
            }
            pendingDiff = nil

        case .commandRun(let argv, let cwd, let exitCode, _, _):
            if let pendingTerminal {
                pendingTerminal.command = ToolEntry.Command(argv: argv, cwd: cwd, exitCode: exitCode)
            }
            pendingTerminal = nil

        case .actionEmitted(let data):
            guard case .object(let obj) = data,
                  case .string(let kind)? = obj["kind"],
                  let payload = obj["payload"]
            else { continue }
            pendingActionArgsByTool[kind, default: []].append(payload)
        }
    }

    return turns.enumerated().map { index, t in
        let entries = t.tools.map { $0.build() }
        let findingCount = entries.reduce(0) { $0 + ($1.kind == .finding ? 1 : 0) }
        let hasError = entries.contains { $0.errorText != nil }
        return TurnVM(
            id: t.id,
            assistantText: t.assistantText,
            thinking: t.thinking,
            tools: entries,
            findingCount: findingCount,
            hasError: hasError,
            isOpenByDefault: index == turns.count - 1,
            tokensIn: t.tokensIn,
            tokensOut: t.tokensOut
        )
    }
}

// ---------------------------------------------------------------------------
// Input summary (ToolCard.tsx:50-121)
// ---------------------------------------------------------------------------

private func truncated(_ s: String, limit: Int = 60) -> String {
    guard s.count > limit else { return s }
    return String(s.prefix(limit - 3)) + "…"
}

private func stringField(_ obj: [String: JSONValue], _ key: String) -> String? {
    if case .string(let s)? = obj[key] { return s }
    return nil
}

private func numberField(_ obj: [String: JSONValue], _ key: String) -> Double? {
    if case .number(let n)? = obj[key] { return n }
    return nil
}

private func nonEmpty(_ s: String?) -> String? {
    guard let s, !s.isEmpty else { return nil }
    return s
}

/// Derives a short (<= ~60 char) header-line summary for a tool's input,
/// per-kind, mirroring `ToolCard.tsx:50-121`'s `summarizeInput`. `nil` when
/// nothing useful can be extracted (the web's own empty-string return).
public func summarizeInput(tool: String, kind: ToolKind, input: JSONValue) -> String? {
    if case .string(let s) = input {
        return nonEmpty(truncated(s))
    }
    guard case .object(let rec) = input else { return nil }

    switch kind {
    case .read:
        guard let path = nonEmpty(stringField(rec, "path")) else { return nil }
        let start = numberField(rec, "start_line").map { Int($0) }
        let end = numberField(rec, "end_line").map { Int($0) }
        if let start, let end { return "\(path):\(start)-\(end)" }
        if let start { return "\(path):\(start)" }
        return path

    case .grep:
        let pattern = stringField(rec, "pattern") ?? ""
        let path = stringField(rec, "path") ?? ""
        if !pattern.isEmpty && !path.isEmpty { return "\(pattern)  \(path)" }
        return nonEmpty(pattern) ?? nonEmpty(path)

    case .glob:
        let pattern = stringField(rec, "pattern") ?? ""
        let path = stringField(rec, "path") ?? ""
        return nonEmpty(pattern) ?? nonEmpty(path)

    case .terminal:
        let cmd = stringField(rec, "command") ?? stringField(rec, "cmd") ?? ""
        return nonEmpty(truncated(cmd))

    case .diff:
        return nonEmpty(stringField(rec, "path"))

    case .astGrep:
        let pattern = stringField(rec, "pattern") ?? ""
        let lang = stringField(rec, "lang") ?? ""
        if !pattern.isEmpty && !lang.isEmpty { return "\(pattern) · \(lang)" }
        return nonEmpty(pattern) ?? nonEmpty(lang)

    case .finding, .generic, .coverage, .subrun:
        for key in ["path", "pattern", "query", "name", "description"] {
            if let v = nonEmpty(stringField(rec, key)) {
                return truncated(v)
            }
        }
        return nil
    }
}
