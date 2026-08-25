import Foundation
import RupuAPI

// Situation Room (Phase 6B, Task 7 review fix round 1, ruling 5) — pure,
// testable logic `SituationStore` wires its budgeted run-resolution and
// events/min sampling through, extracted so both can be table-tested
// without any networking, timers, or `@MainActor` state.

/// Newest-first (`ts` descending; `runID` ascending as a deterministic
/// tie-break — `[String: Int64]`'s iteration order is not stable across
/// runs), budgeted selection over a `[runID: ts]` snapshot, filtered by
/// `isEligible`. Shared shape for both of `SituationStore.resolveRuns`'s
/// passes — the "never resolved" pass and the "stale, needs recheck" pass —
/// which differ only in their `isEligible` predicate and share one 12-slot
/// budget across both (ruling 4: the STALE pass gets whatever budget the
/// UNRESOLVED pass didn't spend, both newest-first).
///
/// `isEligible` is checked only up to `budget` PICKS, not up to `budget`
/// CANDIDATES scanned — an ineligible newer run never displaces an eligible
/// older one from the walk, it's just skipped, matching
/// `Events.tsx`'s own `for (...) { if (budget === 0) return; if (!eligible)
/// continue; ...; budget -= 1 }` loop shape (lines 205-213 / 214-223).
func selectNewestFirst(
    candidates: [String: Int64],
    budget: Int,
    isEligible: (String) -> Bool
) -> [String] {
    guard budget > 0 else { return [] }
    let ordered = candidates.sorted { a, b in
        a.value != b.value ? a.value > b.value : a.key < b.key
    }
    var picked: [String] = []
    for (runID, _) in ordered {
        guard picked.count < budget else { break }
        guard isEligible(runID) else { continue }
        picked.append(runID)
    }
    return picked
}

/// One events/min sampling tick. Exact port of `Events.tsx`'s
/// `SPARK_TICK_MS`-interval sampler (lines 169-177):
/// `setSpark(prev => [...prev.slice(1), n])` (the ring buffer push) and
/// `Math.round((n * 60_000) / SPARK_TICK_MS)` (the rate arithmetic).
/// `windowMS` is the real tick interval as milliseconds (`5_000.0` in
/// production, matching `SPARK_TICK_MS`) — a parameter, not a baked-in
/// constant, so this stays testable at whatever cadence a test wants to
/// assert on.
func sparkTick(current: [Int], eventsInWindow: Int, windowMS: Double) -> (spark: [Int], eventsPerMin: Int) {
    let nextSpark = Array(current.dropFirst()) + [eventsInWindow]
    let rate = Int((Double(eventsInWindow) * 60_000 / windowMS).rounded())
    return (nextSpark, rate)
}

/// `Events.tsx` line 37's `FRESH_MS = 2500` (milliseconds), as seconds —
/// the fresh-arrival highlight window `foldFreshMarks` below marks a key
/// for.
let freshHighlightSeconds: TimeInterval = 2.5

/// Fresh-arrival tracking — port of `Events.tsx`'s `freshKeys: Set<string>`
/// plus its per-key `setTimeout(() => ..., FRESH_MS)` removal (lines 87,
/// 139-144), verified against the actual `subscribeEvents` callback (not
/// the initial `getEvents` history-load effect, lines 109-125, which never
/// touches `freshKeys` at all — only a genuinely NEW live event marks one).
///
/// **Lives here (`RupuStore`), not in `RupuSituation` alongside
/// `isFollowing`/`planStreamRender`** (this task's brief sketch put both
/// pure pieces in `RupuSituation`) — a deliberate, reasoned divergence, not
/// an oversight. The web's "only a genuinely new LIVE event, never the
/// history backfill or a reconnect's re-fetched page" trigger is not
/// something the RENDER layer can reconstruct after the fact: by the time
/// `RupuSituation`'s `assembleSituation` sees `eventRows`, a freshly-merged
/// history row and a freshly-applied live row are indistinguishable arrays
/// of the same `CPEventRow` shape. `SituationStore.applyLive(_:)` is the
/// ONE place in this port that already knows the difference (it's the
/// event-arrival function itself — `mergeIncoming(_:)`, used by both the
/// initial load and reconnect backfill, never calls into this), so marking
/// freshness has to happen there, store-side, to stay correct — matching
/// the "verified web source overrides the brief's own paraphrase" precedent
/// `StreamCards.swift`'s file header and `EventStreamColumn.swift`'s file
/// header both already establish for this exact task/module pair.
///
/// Modeled as an explicit per-key EXPIRY timestamp map, not one real
/// `Task.sleep`/timer per key (the direct transliteration of the web's
/// per-key `setTimeout`) — so the fold is a pure function of `(current,
/// arrivals, now)`, table-testable with an injected `now` and no real
/// sleeps bracketing the assertions (#611). `SituationStore` advances this
/// with `Date()` on every live arrival plus a light periodic real-time
/// prune tick, so a highlight still clears on schedule even when no
/// further events land to trigger another fold.
///
/// Prunes every entry whose expiry has already passed `now`, then
/// (re)marks every key in `arrivals` fresh for `duration` starting at
/// `now`. Generic over `Key` rather than fixed to `String`/`CPEvent` — the
/// identity this module tracks freshness by is `CPEvent` itself (this
/// file's sibling `SituationStore.seenEventKeys` already keys the same
/// dedup ledger on it), not the `StreamCard.key` string `RupuSituation`
/// computes downstream from it.
func foldFreshMarks<Key: Hashable>(
    _ current: [Key: Date],
    arrivals: [Key],
    now: Date,
    duration: TimeInterval = freshHighlightSeconds
) -> [Key: Date] {
    var next = current.filter { $0.value > now }
    for key in arrivals {
        next[key] = now.addingTimeInterval(duration)
    }
    return next
}

/// Deterministic secondary sort key for `SituationStore.mergeIncoming(_:)`'s
/// merge sort — review fix round 2, ruling 1: `Array.sort(by:)` is NOT a
/// stability-guaranteed API (an earlier doc comment on this file's sole
/// caller wrongly claimed it was), and two rows sharing a `ts` (the common
/// case — a burst of events from one poll/backfill page frequently shares
/// a millisecond, and multiple live events can share the "arrival time"
/// stamp `SituationStore.applyLive(_:)` gives them) is exactly the shape
/// where an unstable sort's tie order is allowed to differ between calls.
/// This composes every field a case carries (minus the server-injected
/// `ts`/`pos`, which is what `CPEventRow` already keeps separate from
/// `CPEvent`) into one string, so `merged.sort { a, b in a.ts != b.ts ?
/// a.ts > b.ts : mergeSortKey(a.event) < mergeSortKey(b.event) }` is a full,
/// deterministic total order — same "value descending, key ascending"
/// shape `selectNewestFirst` above already uses, with a row's own content
/// standing in for that function's `runID` key.
///
/// Deliberately a SEPARATE, scoped-down composition from `RupuSituation`'s
/// own `contentIdentityKey` (`StreamCards.swift`) — same "cannot import
/// `RupuSituation`" module-boundary reason as everywhere else in this
/// module (see `SituationStore`'s own doc comment) — not a shared
/// implementation reused across the boundary. This one only needs to be a
/// deterministic function of an event's content for ONE purpose (breaking
/// a sort tie inside a single process's memory); it never needs to match a
/// key computed by different code, so nothing here needs to stay in
/// lockstep with that other function's exact composition.
///
/// **Delimiter-safe composition** (final-review fix, item 4). An earlier
/// revision joined raw field values with a bare `|`, which is not injective
/// once a component can itself contain a `|` — and these are free-form
/// server strings (`error`, `reason`, `note`, `unit_key`, paths). Two
/// DIFFERENT events could then produce the identical key (classic shifted
/// boundary: `stepFailed(stepID: "s|t", error: "u")` vs
/// `stepFailed(stepID: "s", error: "t|u")`), which for a tie-break
/// comparator means two distinct rows compare EQUAL and the sort's order
/// between them falls back to whatever the unstable sort happens to do —
/// re-introducing exactly the non-determinism round 2's ruling 1 fixed.
/// Every component is therefore length-prefixed as `"<count>:<value>"`
/// (`count` in `Character`s) before joining, with `nil` encoded as `"~"` (a
/// marker no length-prefixed component can produce — those always start with
/// a digit), making the concatenation uniquely decodable and hence
/// injective. `RupuSituation`'s `contentIdentityKey` applies the same
/// encoding for the same reason.
func mergeSortKey(_ event: CPEvent) -> String {
    let runID = event.runID ?? ""
    switch event {
    case let .runStarted(_, workflowPath, startedAt):
        return joinKey("runStarted", runID, workflowPath, startedAt)
    case let .stepStarted(_, stepID, kind, agent, host):
        return joinKey("stepStarted", runID, stepID, kind, agent, host)
    case let .stepWorking(_, stepID, note, transcriptPath):
        return joinKey("stepWorking", runID, stepID, note, transcriptPath)
    case let .stepAwaitingApproval(_, stepID, reason):
        return joinKey("stepAwaitingApproval", runID, stepID, reason)
    case let .stepCompleted(_, stepID, success, durationMS, host):
        return joinKey("stepCompleted", runID, stepID, String(success), String(durationMS), host)
    case let .stepFailed(_, stepID, error):
        return joinKey("stepFailed", runID, stepID, error)
    case let .stepSkipped(_, stepID, reason):
        return joinKey("stepSkipped", runID, stepID, reason)
    case let .unitStarted(_, stepID, index, unitKey, agent, transcriptPath, host):
        return joinKey(
            "unitStarted", runID, stepID, String(index), unitKey, agent, transcriptPath, host
        )
    case let .unitCompleted(_, stepID, index, unitKey, success, tokensIn, tokensOut, host):
        return joinKey(
            "unitCompleted", runID, stepID, String(index), unitKey, String(success),
            String(tokensIn), String(tokensOut), host
        )
    case let .panelRound(_, stepID, round, maxIterations, maxSeverityRemaining):
        return joinKey(
            "panelRound", runID, stepID, String(round), String(maxIterations), maxSeverityRemaining
        )
    case let .runCompleted(_, status, finishedAt):
        return joinKey("runCompleted", runID, status, finishedAt)
    case let .runFailed(_, error, finishedAt):
        return joinKey("runFailed", runID, error, finishedAt)
    case .runPaused:
        return joinKey("runPaused", runID)
    case .runResumed:
        return joinKey("runResumed", runID)
    case let .stepPaused(_, stepID):
        return joinKey("stepPaused", runID, stepID)
    case let .stepResumed(_, stepID):
        return joinKey("stepResumed", runID, stepID)
    case let .dispatchStarted(_, subRunID, agent, transcriptPath):
        return joinKey("dispatchStarted", runID, subRunID, agent, transcriptPath)
    case let .dispatchCompleted(_, subRunID, success, tokensIn, tokensOut):
        return joinKey(
            "dispatchCompleted", runID, subRunID, String(success), String(tokensIn), String(tokensOut)
        )
    case let .unknown(type, _):
        return joinKey("unknown", runID, type)
    }
}

/// Length-prefixed, delimiter-safe join — see `mergeSortKey`'s
/// "Delimiter-safe composition" section for why a bare `|` join is not
/// injective here. `nil` encodes as `"~"`, distinct from every
/// length-prefixed value (which always begins with a digit) and therefore
/// also distinct from the empty string (`"0:"`).
private func joinKey(_ parts: String?...) -> String {
    parts.map { part in
        guard let part else { return "~" }
        return "\(part.count):\(part)"
    }.joined(separator: "|")
}
