import Foundation
import RupuAPI
import RupuDesign

// Situation Room — pure mapping from typed wire objects (`CPEvent`,
// `APIFinding`) to `StreamCard` view models. Line-by-line Swift port of
// `crates/rupu-cp/web/src/lib/situationRoom/cards.ts` (verified full read,
// 211 lines) — every switch arm below cites the web line(s) it mirrors so a
// future diff against the web file is a straight line-number lookup.
//
// Two deliberate, cited divergences from the web source (both forced by
// what `CPEvent`/`APIFinding` — this task's fixed inputs — actually carry;
// neither is a "fix" to the web's behavior, both are documented here and in
// the task-6 report):
//
// 1. Unknown/forward-compat events. `cards.ts` lines 99-111 special-case
//    `!isKnownRunEvent(ev)` by still rendering a fallback activity card
//    (badge = the raw type with underscores replaced, title falling back to
//    the raw `step_id` when present). `CPEvent.unknown(type:runID:)`
//    (`RupuAPI/CPEvent.swift` lines 12,31,53) carries only `type`/`runID` —
//    no `step_id` — because the Swift decoder never retains the rest of the
//    raw JSON object the JS `identityOf`/fallback path reads from. Per this
//    task's brief (`task-6-brief.md` line 23: "Where `CPEvent.unknown`
//    arrives, the card mapping returns nil"), `cardForEvent` returns `nil`
//    for `.unknown` rather than fabricating a lower-fidelity fallback card
//    from partial data.
// 2. `CPEvent.swift`'s `knownTypeTags` (lines 226-232) additionally decodes
//    `dispatch_started`/`dispatch_completed` into typed cases — event kinds
//    `cards.ts`'s own `KNOWN_EVENT_TYPES` (`web/src/lib/api.ts` lines
//    311-328) does NOT include, so on the web these are themselves
//    `isKnownRunEvent`-false and take the unknown-event fallback path. To
//    stay consistent with divergence (1) above, `cardForEvent`'s switch below
//    covers exactly the 16 event kinds `cards.ts`'s `KNOWN_EVENT_TYPES` set
//    lists (matching `cardFromEvent`'s 16 explicit `case` arms, `cards.ts`
//    lines 116-175) and falls through to `nil` for `.dispatchStarted`,
//    `.dispatchCompleted`, and `.unknown` alike.

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

/// Map one event to a `StreamCard`, or `nil` when the event carries nothing
/// worth a row (a note-less `step_working` heartbeat, an unknown/
/// forward-compat event type, or a `dispatch_started`/`dispatch_completed`
/// event `cards.ts`'s own `KNOWN_EVENT_TYPES` doesn't include — see the file
/// header for both cited divergences).
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
/// parameter) from `CPEvent`'s own case + associated values — since
/// `CPEventRow` already separates `ts`/`pos` from the decoded `CPEvent`
/// (`RupuAPI/Models.swift` lines 37-52), two decoded events are content-
/// identical (`Equatable`) exactly when they'd produce the same web
/// `identityOf` (`web/src/pages/Events.tsx` lines 48-68) content-hash key —
/// `String(describing:)` on the enum gives a stable, deterministic string
/// for that same identity without re-deriving a stable-stringify routine.
public func cardForEvent(_ e: CPEvent, ts: Date?) -> StreamCard? {
    let tsMS = ts.map { Int64(($0.timeIntervalSince1970 * 1000).rounded()) } ?? 0
    let key = String(describing: e)

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
    case .dispatchStarted, .dispatchCompleted, .unknown:
        // Not in cards.ts's KNOWN_EVENT_TYPES (dispatch_*) or the web's
        // isKnownRunEvent gate (.unknown) — see file header divergence (1)/(2).
        return nil
    }
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
    let sev = Severity(wireString: f.severity)
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
/// line-by-line — the web assembles its stream in two places instead:
/// `Events.tsx` lines 271-274 does the newest-first `sort`, uncapped
/// (`[...eventCards, ...findingCards].sort((a, b) => b.ts - a.ts)`); the
/// content-identity dedup EXCLUDING `ts`/`pos` that this task's brief calls
/// out lives one layer upstream, on the raw `RunEvent` before it ever
/// becomes a card — `Events.tsx`'s `identityOf` (lines 48-68, a sorted-key
/// stable-stringify of the event object minus `ts`/`pos`) feeds the
/// `seenRef` `Set` that `cardFromEvent`'s own caller-supplied `key`
/// parameter is built from (line 253's `cardFromEvent(event, ts, key)`).
/// `cardForEvent` above already reproduces that same content identity as
/// `StreamCard.key` (see its doc comment) — a card built from a duplicate
/// replay of the same event (same content, different `ts`) carries the same
/// `key`. So the port here is: newest-first sort, then dedup by `key`
/// keeping the first (i.e. newest) occurrence — the same net effect as the
/// web's upstream `seenRef` gate applied post-sort instead of pre-map — then
/// cap at `max` (`Events.tsx`'s `MAX_EVENTS`/`STREAM_FINDINGS_CAP`-style
/// caps, generalized to one caller-supplied bound).
public func mergeStream(_ cards: [StreamCard], max: Int) -> [StreamCard] {
    guard max > 0 else { return [] }
    let sorted = cards.sorted { $0.ts > $1.ts }
    var seen = Set<String>()
    var out: [StreamCard] = []
    out.reserveCapacity(Swift.min(sorted.count, max))
    for card in sorted {
        if seen.contains(card.key) { continue }
        seen.insert(card.key)
        out.append(card)
        if out.count >= max { break }
    }
    return out
}
