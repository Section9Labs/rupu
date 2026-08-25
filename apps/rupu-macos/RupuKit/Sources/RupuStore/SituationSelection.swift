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
func mergeSortKey(_ event: CPEvent) -> String {
    let runID = event.runID ?? ""
    switch event {
    case let .runStarted(_, workflowPath, startedAt):
        return "runStarted|\(runID)|\(workflowPath)|\(startedAt)"
    case let .stepStarted(_, stepID, kind, agent, host):
        return "stepStarted|\(runID)|\(stepID)|\(kind)|\(agent ?? "")|\(host ?? "")"
    case let .stepWorking(_, stepID, note, transcriptPath):
        return "stepWorking|\(runID)|\(stepID)|\(note ?? "")|\(transcriptPath ?? "")"
    case let .stepAwaitingApproval(_, stepID, reason):
        return "stepAwaitingApproval|\(runID)|\(stepID)|\(reason)"
    case let .stepCompleted(_, stepID, success, durationMS, host):
        return "stepCompleted|\(runID)|\(stepID)|\(success)|\(durationMS)|\(host ?? "")"
    case let .stepFailed(_, stepID, error):
        return "stepFailed|\(runID)|\(stepID)|\(error)"
    case let .stepSkipped(_, stepID, reason):
        return "stepSkipped|\(runID)|\(stepID)|\(reason)"
    case let .unitStarted(_, stepID, index, unitKey, agent, transcriptPath, host):
        return "unitStarted|\(runID)|\(stepID)|\(index)|\(unitKey)|\(agent ?? "")|\(transcriptPath)|\(host ?? "")"
    case let .unitCompleted(_, stepID, index, unitKey, success, tokensIn, tokensOut, host):
        return "unitCompleted|\(runID)|\(stepID)|\(index)|\(unitKey)|\(success)|\(tokensIn)|\(tokensOut)|\(host ?? "")"
    case let .panelRound(_, stepID, round, maxIterations, maxSeverityRemaining):
        return "panelRound|\(runID)|\(stepID)|\(round)|\(maxIterations)|\(maxSeverityRemaining ?? "")"
    case let .runCompleted(_, status, finishedAt):
        return "runCompleted|\(runID)|\(status)|\(finishedAt)"
    case let .runFailed(_, error, finishedAt):
        return "runFailed|\(runID)|\(error)|\(finishedAt)"
    case .runPaused:
        return "runPaused|\(runID)"
    case .runResumed:
        return "runResumed|\(runID)"
    case let .stepPaused(_, stepID):
        return "stepPaused|\(runID)|\(stepID)"
    case let .stepResumed(_, stepID):
        return "stepResumed|\(runID)|\(stepID)"
    case let .dispatchStarted(_, subRunID, agent, transcriptPath):
        return "dispatchStarted|\(runID)|\(subRunID)|\(agent ?? "")|\(transcriptPath)"
    case let .dispatchCompleted(_, subRunID, success, tokensIn, tokensOut):
        return "dispatchCompleted|\(runID)|\(subRunID)|\(success)|\(tokensIn)|\(tokensOut)"
    case let .unknown(type, _):
        return "unknown|\(runID)|\(type)"
    }
}
