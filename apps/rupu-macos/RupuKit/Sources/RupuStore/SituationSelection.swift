import Foundation

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
