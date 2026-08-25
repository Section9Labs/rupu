import Foundation
import RupuAPI
import RupuDesign

// Situation Room — pure mapping from typed wire objects (`CPEvent`,
// `APIFinding`) to `StreamCard` view models. Line-by-line Swift port of
// `crates/rupu-cp/web/src/lib/situationRoom/cards.ts` (verified full read,
// 211 lines) — every switch arm below cites the web line(s) it mirrors so a
// future diff against the web file is a straight line-number lookup.
//
// Fix round 1 correction: an earlier pass here had `cardForEvent` return
// `nil` for `.unknown`/`.dispatchStarted`/`.dispatchCompleted` believing the
// task-6 brief's "returns nil" paraphrase over the verified `cards.ts`
// source. Task review confirmed the web source (not the paraphrase) is
// authoritative: `cards.ts` lines 99-111 special-case
// `!isKnownRunEvent(ev)` (which — per `web/src/lib/api.ts`'s
// `KNOWN_EVENT_TYPES`, lines 311-328 — covers `.unknown` AND
// `dispatch_started`/`dispatch_completed`, none of which that set lists) by
// STILL rendering a degraded activity card rather than dropping the event:
// `form`/`group` = `activity`, `accent` = `brand`, `badge` = the raw type
// with underscores replaced by spaces, `title` = the raw `step_id` when the
// raw object carries a string one, else the raw type verbatim, `runId`
// carried through. That fallback is now ported below for all three cases.
//
// One remaining, honest divergence: the web's title fallback is
// conditional on the raw JS object actually having a string `step_id` key.
// `CPEvent.unknown(type:runID:)` and the two `dispatch*` cases
// (`RupuAPI/CPEvent.swift`) never carry a `step_id` field at all — the
// Swift decoder doesn't retain the rest of the payload the way the raw JS
// object does — so for these three cases the title always takes the
// `ev.type` branch of that ternary; the `step_id`-present branch is simply
// unreachable here. Not a behavior gap in practice (no known instance of
// these event kinds carries a `step_id` on the wire today), documented for
// honesty.

/// Which filter chip a card answers to. Port of `cards.ts` line 24's
/// `CardGroup` union. `await_` (trailing underscore) sidesteps the `await`
/// keyword — same convention the task brief's own interface sketch uses.
public enum CardGroup: Equatable, Sendable {
    case finding, await_, error, activity
}

/// The editorial form the card renders as. Port of `cards.ts` lines 28-35's
/// `CardForm` union.
public enum CardForm: Equatable, Sendable {
    case activity, panel, lifecycle, complete, finding, await_, error
}

/// Left-stripe / badge color key. Port of `cards.ts` line 39's `CardAccent`
/// union (`FindingSeverity | 'brand' | 'await' | 'error'`) — the
/// `FindingSeverity` half is modeled as `.severity(Severity)` using
/// `RupuDesign`'s existing severity type rather than re-deriving a parallel
/// string enum.
public enum CardAccent: Equatable, Sendable {
    case severity(Severity)
    case brand
    case await_
    case error
}

/// Present on `await` cards — the run + reason an approval can act on. Port
/// of `cards.ts` line 74's `approvable` field shape.
public struct Approvable: Equatable, Sendable {
    public let runID: String
    public let stepID: String?
    public let reason: String

    public init(runID: String, stepID: String?, reason: String) {
        self.runID = runID
        self.stepID = stepID
        self.reason = reason
    }
}

/// Port of `cards.ts` lines 41-75's `StreamCard` interface.
public struct StreamCard: Equatable, Sendable {
    /// Stable identity — dedup key (see `mergeStream`) and view identity.
    public let key: String
    /// Ordering key, ms since epoch, newest-first in the stream.
    public let ts: Int64
    public let form: CardForm
    public let group: CardGroup
    public let accent: CardAccent
    public let badge: String
    public let title: String
    public let detail: String?
    public let runID: String?
    public let projectName: String?
    public let stepID: String?
    public let agent: String?
    public let severity: Severity?
    public let fileRef: String?
    public let code: String?
    public let wsID: String?
    public let filePath: String?
    public let fileLine: Int?
    public let permalink: String?
    public let approvable: Approvable?

    public init(
        key: String,
        ts: Int64,
        form: CardForm,
        group: CardGroup,
        accent: CardAccent,
        badge: String,
        title: String,
        detail: String? = nil,
        runID: String? = nil,
        projectName: String? = nil,
        stepID: String? = nil,
        agent: String? = nil,
        severity: Severity? = nil,
        fileRef: String? = nil,
        code: String? = nil,
        wsID: String? = nil,
        filePath: String? = nil,
        fileLine: Int? = nil,
        permalink: String? = nil,
        approvable: Approvable? = nil
    ) {
        self.key = key
        self.ts = ts
        self.form = form
        self.group = group
        self.accent = accent
        self.badge = badge
        self.title = title
        self.detail = detail
        self.runID = runID
        self.projectName = projectName
        self.stepID = stepID
        self.agent = agent
        self.severity = severity
        self.fileRef = fileRef
        self.code = code
        self.wsID = wsID
        self.filePath = filePath
        self.fileLine = fileLine
        self.permalink = permalink
        self.approvable = approvable
    }
}

// MARK: - cardForEvent

/// Short, human step label — strips noise, keeps the id readable. Port of
/// `cards.ts` lines 86-88's `stepLabel`.
private func stepLabel(_ stepID: String?) -> String {
    (stepID ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
}

/// JS truthiness for an optional string: `nil` and `""` are both falsy.
/// Several `cards.ts`/`roster.ts` branches use `k.agent ? … : …` /
/// `event.note?.trim() || event.step_id` — this mirrors that check.
private func isTruthy(_ s: String?) -> Bool {
    guard let s else { return false }
    return !s.isEmpty
}

/// Port of `cards.ts` line 147's
/// `` `${k.success ? 'ok' : 'failed'} · ${Math.round(k.duration_ms / 100) / 10}s` ``
/// tenths-of-a-second formatting — `Math.round(ms / 100) / 10` then JS's
/// default number-to-string (no trailing `.0` for a whole number).
private func formatTenthsSeconds(_ durationMS: UInt64) -> String {
    let tenths = (Double(durationMS) / 100).rounded() / 10
    if tenths.truncatingRemainder(dividingBy: 1) == 0 {
        return String(Int(tenths))
    }
    return String(tenths)
}

/// Explicit, auditable content-identity key — one string per `CPEvent`
/// case, composed from every field the case carries (i.e. everything
/// `CPEventRow` keeps on `CPEvent` once the server-injected `ts`/`pos` are
/// stripped out — see `RupuAPI/Models.swift` lines 37-52). This mirrors what
/// the web's `identityOf` (`web/src/pages/Events.tsx` lines 48-68) covers —
/// every remaining key of the raw event object after `ts`/`pos` — but as an
/// explicit, stable composition rather than `String(describing:)`
/// reflection, whose output format Swift does not document as stable across
/// compiler versions (fix round 1, finding 6).
///
/// **Delimiter-safe composition** (final-review fix, item 4). A plain
/// `joined(separator: "|")` is NOT injective once a component can itself
/// contain the delimiter — and these components are free-form server text
/// (`error`, `reason`, `note`, `unit_key`, paths), so a `|` in one is
/// entirely possible. `stepFailed(stepID: "s|t", error: "u")` and
/// `stepFailed(stepID: "s", error: "t|u")` are DIFFERENT events that joined
/// to the identical string, which would make the stream's dedup (this key
/// is `StreamCard.key`) collapse two genuinely distinct rows into one. The
/// web's `identityOf` doesn't have the problem because it stable-stringifies
/// a JS object, which is delimiter-safe by construction.
///
/// Each component is therefore LENGTH-PREFIXED as `"<count>:<value>"`
/// (`count` in `Character`s) before joining, with `nil` encoded as the
/// marker `"~"` — a prefix no length-prefixed component can produce, since
/// those always start with a digit. That makes the concatenation uniquely
/// decodable left-to-right (read digits to `:`, take exactly that many
/// characters, expect `|` or end), i.e. injective, so distinct field tuples
/// can never share a key regardless of what the fields contain. `mergeSortKey`
/// (`RupuStore/SituationSelection.swift`) applies the same encoding for the
/// same reason — separately, per the module-boundary note in that function's
/// own doc comment; the two need not stay byte-identical, only both safe.
private func contentIdentityKey(_ e: CPEvent) -> String {
    func key(_ parts: String?...) -> String {
        parts.map { part in
            guard let part else { return "~" }
            return "\(part.count):\(part)"
        }.joined(separator: "|")
    }
    switch e {
    case let .runStarted(runID, workflowPath, startedAt):
        return key("run_started", runID, workflowPath, startedAt)
    case let .stepStarted(runID, stepID, kind, agent, host):
        return key("step_started", runID, stepID, kind, agent, host)
    case let .stepWorking(runID, stepID, note, transcriptPath):
        return key("step_working", runID, stepID, note, transcriptPath)
    case let .stepAwaitingApproval(runID, stepID, reason):
        return key("step_awaiting_approval", runID, stepID, reason)
    case let .stepCompleted(runID, stepID, success, durationMS, host):
        return key("step_completed", runID, stepID, String(success), String(durationMS), host)
    case let .stepFailed(runID, stepID, error):
        return key("step_failed", runID, stepID, error)
    case let .stepSkipped(runID, stepID, reason):
        return key("step_skipped", runID, stepID, reason)
    case let .unitStarted(runID, stepID, index, unitKey, agent, transcriptPath, host):
        return key("unit_started", runID, stepID, String(index), unitKey, agent, transcriptPath, host)
    case let .unitCompleted(runID, stepID, index, unitKey, success, tokensIn, tokensOut, host):
        return key(
            "unit_completed", runID, stepID, String(index), unitKey, String(success),
            String(tokensIn), String(tokensOut), host
        )
    case let .panelRound(runID, stepID, round, maxIterations, maxSeverityRemaining):
        return key("panel_round", runID, stepID, String(round), String(maxIterations), maxSeverityRemaining)
    case let .runCompleted(runID, status, finishedAt):
        return key("run_completed", runID, status, finishedAt)
    case let .runFailed(runID, error, finishedAt):
        return key("run_failed", runID, error, finishedAt)
    case let .runPaused(runID):
        return key("run_paused", runID)
    case let .runResumed(runID):
        return key("run_resumed", runID)
    case let .stepPaused(runID, stepID):
        return key("step_paused", runID, stepID)
    case let .stepResumed(runID, stepID):
        return key("step_resumed", runID, stepID)
    case let .dispatchStarted(runID, subRunID, agent, transcriptPath):
        return key("dispatch_started", runID, subRunID, agent, transcriptPath)
    case let .dispatchCompleted(runID, subRunID, success, tokensIn, tokensOut):
        return key("dispatch_completed", runID, subRunID, String(success), String(tokensIn), String(tokensOut))
    case let .unknown(type, runID):
        return key("unknown", type, runID)
    }
}

/// Map one event to a `StreamCard`. `nil` only for a note-less
/// `step_working` heartbeat (`cards.ts` lines 131-136) — every other event
/// kind, known or not, renders a card (the unknown/dispatch fallback below
/// included), matching `cardFromEvent`'s only `null`-returning path on the
/// web.
///
/// `ts` is caller-supplied (mirrors `cards.ts` line 99's `ts: number`
/// parameter, adapted to `Date?` per the task-6 brief so this pure layer
/// never parses a timestamp itself): history rows carry their own
/// timestamp; live SSE frames should be stamped with arrival time by the
/// caller. A `nil` `ts` maps to `0`, matching `cardFromFinding`'s own
/// `Number.isNaN(ts) ? 0 : ts` fallback (`cards.ts` line 194) for an
/// unparseable/missing timestamp.
///
/// `key` is derived internally (unlike `cards.ts`'s caller-supplied `key`
/// parameter) via `contentIdentityKey` below — an explicit, per-case
/// composition of every field `CPEventRow` keeps on `CPEvent` (i.e.
/// everything except the server-injected `ts`/`pos`, which `CPEventRow`
/// already keeps separate — `RupuAPI/Models.swift` lines 37-52), mirroring
/// what the web's `identityOf` (`web/src/pages/Events.tsx` lines 48-68)
/// covers for the same fields.
public func cardForEvent(_ e: CPEvent, ts: Date?) -> StreamCard? {
    let tsMS = ts.map { Int64(($0.timeIntervalSince1970 * 1000).rounded()) } ?? 0
    let key = contentIdentityKey(e)

    switch e {
    case let .runStarted(runID, workflowPath, _):
        // cards.ts lines 116-118
        return StreamCard(
            key: key, ts: tsMS, form: .lifecycle, group: .activity, accent: .brand,
            badge: "Run started", title: "Workflow run started", detail: workflowPath, runID: runID
        )
    case let .runCompleted(runID, status, _):
        // cards.ts lines 119-122
        let accent: CardAccent = status == "completed" ? .brand : status == "failed" ? .error : .brand
        return StreamCard(
            key: key, ts: tsMS, form: .complete, group: .activity, accent: accent,
            badge: "Run " + status, title: "Run \(status)", runID: runID
        )
    case let .runFailed(runID, error, _):
        // cards.ts lines 123-125
        return StreamCard(
            key: key, ts: tsMS, form: .error, group: .error, accent: .error,
            badge: "Run failed", title: "Workflow run failed", detail: error, runID: runID
        )
    case let .stepStarted(runID, stepID, kind, agent, _):
        // cards.ts lines 126-130
        let agentTruthy = isTruthy(agent)
        return StreamCard(
            key: key, ts: tsMS, form: .activity, group: .activity, accent: .brand,
            badge: agentTruthy ? "Scanning" : "Step",
            title: agentTruthy ? "\(agent!) · \(stepLabel(stepID))" : stepLabel(stepID),
            detail: agentTruthy ? nil : kind,
            runID: runID, stepID: stepID, agent: agent
        )
    case let .stepWorking(runID, stepID, note, _):
        // cards.ts lines 131-136
        let trimmed = note?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let trimmed, !trimmed.isEmpty else { return nil }
        return StreamCard(
            key: key, ts: tsMS, form: .activity, group: .activity, accent: .brand,
            badge: "Working", title: stepLabel(stepID), detail: trimmed,
            runID: runID, stepID: stepID
        )
    case let .stepAwaitingApproval(runID, stepID, reason):
        // cards.ts lines 137-141
        return StreamCard(
            key: key, ts: tsMS, form: .await_, group: .await_, accent: .await_,
            badge: "Awaiting you", title: "Approval needed · \(stepLabel(stepID))", detail: reason,
            runID: runID, stepID: stepID,
            approvable: Approvable(runID: runID, stepID: stepID, reason: reason)
        )
    case let .stepCompleted(runID, stepID, success, durationMS, _):
        // cards.ts lines 142-147
        return StreamCard(
            key: key, ts: tsMS, form: .complete, group: .activity,
            accent: success ? .brand : .error,
            badge: success ? "Step done" : "Step failed",
            title: stepLabel(stepID),
            detail: "\(success ? "ok" : "failed") · \(formatTenthsSeconds(durationMS))s",
            runID: runID, stepID: stepID
        )
    case let .stepFailed(runID, stepID, error):
        // cards.ts lines 148-150
        return StreamCard(
            key: key, ts: tsMS, form: .error, group: .error, accent: .error,
            badge: "Error", title: "\(stepLabel(stepID)) failed", detail: error,
            runID: runID, stepID: stepID
        )
    case let .stepSkipped(runID, stepID, reason):
        // cards.ts lines 151-153
        return StreamCard(
            key: key, ts: tsMS, form: .activity, group: .activity, accent: .brand,
            badge: "Skipped", title: "\(stepLabel(stepID)) skipped", detail: reason,
            runID: runID, stepID: stepID
        )
    case let .unitStarted(runID, stepID, _, unitKey, agent, _, _):
        // cards.ts lines 154-157
        return StreamCard(
            key: key, ts: tsMS, form: .activity, group: .activity, accent: .brand,
            badge: "Fan-out", title: "\(stepLabel(stepID)) · \(unitKey)",
            detail: isTruthy(agent) ? "agent \(agent!)" : nil,
            runID: runID, stepID: stepID, agent: agent
        )
    case let .unitCompleted(runID, stepID, _, unitKey, success, tokensIn, tokensOut, _):
        // cards.ts lines 158-162
        return StreamCard(
            key: key, ts: tsMS, form: .complete, group: .activity,
            accent: success ? .brand : .error,
            badge: success ? "Unit done" : "Unit failed",
            title: "\(stepLabel(stepID)) · \(unitKey)",
            detail: "\(success ? "ok" : "failed") · \(tokensIn)→\(tokensOut) tok",
            runID: runID, stepID: stepID
        )
    case let .panelRound(runID, stepID, round, maxIterations, maxSeverityRemaining):
        // cards.ts lines 163-167
        return StreamCard(
            key: key, ts: tsMS, form: .panel, group: .activity, accent: .brand,
            badge: "Panel round", title: "\(stepLabel(stepID)) · round \(round)/\(maxIterations)",
            detail: isTruthy(maxSeverityRemaining) ? "max severity remaining: \(maxSeverityRemaining!)" : nil,
            runID: runID, stepID: stepID
        )
    case let .runPaused(runID):
        // cards.ts line 168
        return StreamCard(
            key: key, ts: tsMS, form: .lifecycle, group: .activity, accent: .await_,
            badge: "Paused", title: "Run paused", runID: runID
        )
    case let .runResumed(runID):
        // cards.ts line 170
        return StreamCard(
            key: key, ts: tsMS, form: .lifecycle, group: .activity, accent: .brand,
            badge: "Resumed", title: "Run resumed", runID: runID
        )
    case let .stepPaused(runID, stepID):
        // cards.ts line 172
        return StreamCard(
            key: key, ts: tsMS, form: .lifecycle, group: .activity, accent: .await_,
            badge: "Paused", title: "\(stepLabel(stepID)) paused", runID: runID, stepID: stepID
        )
    case let .stepResumed(runID, stepID):
        // cards.ts line 174
        return StreamCard(
            key: key, ts: tsMS, form: .lifecycle, group: .activity, accent: .brand,
            badge: "Resumed", title: "\(stepLabel(stepID)) resumed", runID: runID, stepID: stepID
        )
    case let .unknown(type, runID):
        // cards.ts lines 100-111 — isKnownRunEvent-false fallback. No
        // step_id field on this case (see file header), so title always
        // takes the `ev.type` branch of the web's ternary.
        return fallbackCard(key: key, ts: tsMS, rawType: type, runID: runID)
    case let .dispatchStarted(runID, _, _, _):
        // Not in cards.ts's KNOWN_EVENT_TYPES (web/src/lib/api.ts lines
        // 311-328 omits dispatch_started/dispatch_completed) — same
        // isKnownRunEvent-false fallback path as `.unknown` above. The wire
        // type tag is hardcoded here since CPEvent's typed dispatch cases
        // (unlike `.unknown`) don't carry their own type string back.
        return fallbackCard(key: key, ts: tsMS, rawType: "dispatch_started", runID: runID)
    case let .dispatchCompleted(runID, _, _, _, _):
        return fallbackCard(key: key, ts: tsMS, rawType: "dispatch_completed", runID: runID)
    }
}

/// Port of `cards.ts` lines 100-111's `isKnownRunEvent`-false fallback card.
/// Never carries a `stepId` (the web's returned object literal doesn't set
/// one either) even though `title` may equal the raw type string.
private func fallbackCard(key: String, ts: Int64, rawType: String, runID: String?) -> StreamCard {
    StreamCard(
        key: key, ts: ts, form: .activity, group: .activity, accent: .brand,
        badge: rawType.replacingOccurrences(of: "_", with: " "),
        title: rawType,
        runID: runID
    )
}

// MARK: - cardForFinding

private let sevBadgeText: [Severity: String] = [
    .crit: "Critical", .high: "High", .med: "Medium", .low: "Low", .info: "Info",
]

/// Best-effort RFC-3339 → ms-since-epoch, mirroring `Date.parse`'s `NaN`
/// fallback: an unparseable string yields `0`, matching `cards.ts` line
/// 194's `Number.isNaN(ts) ? 0 : ts`. Shared with `Roster.swift`'s
/// `lastActive` parsing (`roster.ts` line 160's `Date.parse(p.last_active)`
/// / `NaN` fallback). Formatters are created per call (not cached as global
/// state) — `ISO8601DateFormatter` isn't `Sendable`, and this is a pure,
/// infrequently-called leaf function, not a hot loop.
func rfc3339ToMS(_ s: String) -> Int64 {
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = fractional.date(from: s) {
        return Int64((d.timeIntervalSince1970 * 1000).rounded())
    }
    if let d = ISO8601DateFormatter().date(from: s) {
        return Int64((d.timeIntervalSince1970 * 1000).rounded())
    }
    return 0
}

/// Map one finding to a `StreamCard`. Port of `cards.ts` lines 184-211's
/// `cardFromFinding` — severity accent, a `file:line` reference, the
/// evidence rationale as `detail`, and the real `code_excerpt` (never
/// fabricated: `nil` when the finding doesn't carry one, never a
/// placeholder). Unlike `cardForEvent`, this takes no `ts` parameter (per
/// the task-6 brief's interface sketch) — the finding's own `declared_at`
/// IS the ordering timestamp on the web (`cards.ts` line 186's
/// `Date.parse(f.declared_at)`), so parsing it happens inside this pure
/// function rather than at a caller boundary.
public func cardForFinding(_ f: APIFinding) -> StreamCard {
    // cards.ts line 185: `normFindingSeverity(f.severity)` lowercases the
    // raw wire string before matching (`raw.toLowerCase()`) — `Severity
    // (wireString:)` (RupuDesign) does an exact-match switch with no
    // lowercasing of its own, so this port lowercases at the call site to
    // stay parity with the web (fix round 1, finding 2).
    let sev = Severity(wireString: f.severity.lowercased())
    let ts = rfc3339ToMS(f.declaredAt)
    let fileRef: String?
    if let path = f.filePath {
        if let range = f.lineRange, range.count == 2 {
            fileRef = "\(path):\(range[0])-\(range[1])"
        } else {
            fileRef = path
        }
    } else {
        fileRef = nil
    }
    return StreamCard(
        key: "finding:\(f.id)",
        ts: ts,
        form: .finding,
        group: .finding,
        accent: .severity(sev),
        badge: sevBadgeText[sev] ?? "Info",
        title: f.summary,
        detail: f.rationale,
        runID: nil,
        projectName: f.project,
        severity: sev,
        fileRef: fileRef,
        code: f.codeExcerpt,
        wsID: f.wsID,
        filePath: f.filePath,
        fileLine: f.lineRange?.first.map { Int($0) },
        permalink: f.permalink
    )
}

// MARK: - mergeStream

/// Merge event cards + finding cards into one newest-first stream, capped at
/// `max`.
///
/// There is no single `mergeStream` function on the web side to port
/// line-by-line — the web assembles its stream across two places instead,
/// and each contributes one half of what this does:
///
/// 1. **Dedup, in ingestion order, keeping the first-arrived duplicate.**
///    `Events.tsx`'s `identityOf` (lines 48-68, a sorted-key
///    stable-stringify of the event object minus `ts`/`pos`) feeds the
///    `seenRef` `Set` that gates BOTH the history loader and the live SSE
///    handler (lines 116-119 / 133-137) — whichever copy of an event
///    (history row or live replay) is *ingested first* wins; a
///    later-arriving duplicate is dropped outright, regardless of which one
///    carries the "newer" `ts`. `cardForEvent`'s `key` (via
///    `contentIdentityKey`) reproduces that same content identity, so
///    deduping `cards` in the order the caller passes them in, keeping the
///    first occurrence of each `key`, is the direct port of that gate.
///    (Fix round 1, finding 5 — an earlier pass here sorted before deduping
///    and so kept the newest-`ts` duplicate instead; inverted.)
/// 2. **Newest-first sort.** `Events.tsx` lines 271-274:
///    `[...eventCards, ...findingCards].sort((a, b) => b.ts - a.ts)`,
///    applied (on the web) to the already-deduped `items`/`findings`
///    state. JS's `Array.prototype.sort` has been a stable sort since
///    ES2019, so a `ts` tie there resolves to array order, which is itself
///    ingestion order — an implicit tiebreak this port makes explicit
///    instead of relying on `sorted`'s incidental stability surviving a
///    future refactor: ties break on `key` (fix round 1, finding 4).
///
/// Then capped at `max` (`Events.tsx`'s `MAX_EVENTS`/`STREAM_FINDINGS_CAP`-
/// style caps, generalized to one caller-supplied bound).
public func mergeStream(_ cards: [StreamCard], max: Int) -> [StreamCard] {
    guard max > 0 else { return [] }

    var seen = Set<String>()
    var deduped: [StreamCard] = []
    deduped.reserveCapacity(cards.count)
    for card in cards {
        if seen.contains(card.key) { continue } // first-arrived wins
        seen.insert(card.key)
        deduped.append(card)
    }

    let sorted = deduped.sorted { a, b in
        a.ts != b.ts ? a.ts > b.ts : a.key < b.key
    }
    return Array(sorted.prefix(max))
}
