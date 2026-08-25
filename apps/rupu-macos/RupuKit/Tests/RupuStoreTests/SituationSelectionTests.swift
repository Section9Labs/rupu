import Testing
import Foundation
import RupuAPI
@testable import RupuStore

// Table tests for `SituationSelection.swift`'s pure functions — review fix
// round 1, ruling 5 (`selectNewestFirst`/`sparkTick`) and round 2, ruling 1
// (`mergeSortKey`). Plain `@Test func`s: every function here is fully pure
// (no networking, no timers, no `@MainActor` state), so nothing needs
// `@MainActor`.

@Suite
struct SelectNewestFirstTests {
    @Test func emptyCandidatesYieldsEmptySelection() {
        #expect(selectNewestFirst(candidates: [:], budget: 12) { _ in true } == [])
    }

    @Test func zeroBudgetYieldsEmptySelectionEvenWithEligibleCandidates() {
        let picked = selectNewestFirst(candidates: ["a": 100], budget: 0) { _ in true }
        #expect(picked == [])
    }

    @Test func picksNewestFirstByDescendingTs() {
        let candidates = ["old": Int64(100), "mid": Int64(200), "new": Int64(300)]
        let picked = selectNewestFirst(candidates: candidates, budget: 3) { _ in true }
        #expect(picked == ["new", "mid", "old"])
    }

    @Test func tiedTsBreaksByAscendingRunIDForDeterminism() {
        let candidates = ["z": Int64(100), "a": Int64(100), "m": Int64(100)]
        let picked = selectNewestFirst(candidates: candidates, budget: 3) { _ in true }
        #expect(picked == ["a", "m", "z"], "a ts tie must not depend on Dictionary's unstable iteration order")
    }

    @Test func budgetTruncatesEvenWhenMoreEligibleCandidatesExist() {
        let candidates = ["a": Int64(300), "b": Int64(200), "c": Int64(100)]
        let picked = selectNewestFirst(candidates: candidates, budget: 2) { _ in true }
        #expect(picked == ["a", "b"])
    }

    @Test func ineligibleCandidatesAreSkippedWithoutConsumingBudget() {
        // "newer" (ts 300) is ineligible; it must not displace or block
        // "older" (ts 100) from being picked, and must not itself count
        // against the budget.
        let candidates = ["newer": Int64(300), "older": Int64(100)]
        let picked = selectNewestFirst(candidates: candidates, budget: 1) { runID in runID == "older" }
        #expect(picked == ["older"])
    }

    @Test func onlyEligibleEntriesEverAppearInTheResult() {
        let candidates = ["a": Int64(300), "b": Int64(200), "c": Int64(100)]
        let picked = selectNewestFirst(candidates: candidates, budget: 3) { runID in runID != "b" }
        #expect(picked == ["a", "c"])
    }
}

@Suite
struct SparkTickTests {
    @Test func pushesTheNewSampleOntoTheRingAndDropsTheOldest() {
        let result = sparkTick(current: [1, 2, 3, 4], eventsInWindow: 9, windowMS: 5_000)
        #expect(result.spark == [2, 3, 4, 9])
    }

    @Test func ringLengthStaysConstant() {
        let result = sparkTick(current: Array(repeating: 0, count: 16), eventsInWindow: 5, windowMS: 5_000)
        #expect(result.spark.count == 16)
    }

    @Test func rateIsRoundedEventsPerMinuteAtTheGivenWindow() {
        // Exact port of Events.tsx line 174: Math.round((n * 60_000) / SPARK_TICK_MS).
        let result = sparkTick(current: [0], eventsInWindow: 3, windowMS: 5_000)
        #expect(result.eventsPerMin == 36) // 3 events / 5s -> 36/min
    }

    @Test func zeroEventsInTheWindowYieldsZeroRateAndPushesZero() {
        let result = sparkTick(current: [7, 8], eventsInWindow: 0, windowMS: 5_000)
        #expect(result.spark == [8, 0])
        #expect(result.eventsPerMin == 0)
    }

    @Test func rateRoundsToNearestNotTruncates() {
        // 1 event / 5s = 12/min exactly; 2 events / 5s = 24/min exactly —
        // pick a window that forces a genuine .5 rounding case: 1 event in
        // a 4s window = 15/min exactly (sanity), 1 event in a 4.8s window
        // rounds rather than truncates.
        let result = sparkTick(current: [0], eventsInWindow: 1, windowMS: 4_800)
        #expect(result.eventsPerMin == 13, "round(60_000 / 4_800) == round(12.5) == 13")
    }
}

@Suite
struct FoldFreshMarksTests {
    private let epoch = Date(timeIntervalSince1970: 1_000_000)

    @Test func arrivalsAreMarkedFreshUntilNowPlusDuration() {
        let next = foldFreshMarks([:], arrivals: ["a"], now: epoch, duration: 2.5)
        #expect(next["a"] == epoch.addingTimeInterval(2.5))
    }

    @Test func multipleArrivalsInOneFoldAllGetTheSameExpiry() {
        let next = foldFreshMarks([:], arrivals: ["a", "b", "c"], now: epoch, duration: 2.5)
        #expect(next.keys.sorted() == ["a", "b", "c"])
        #expect(Set(next.values) == [epoch.addingTimeInterval(2.5)])
    }

    @Test func anEntryStillWithinItsWindowSurvivesAFoldWithNoNewArrivals() {
        let marked = foldFreshMarks([:], arrivals: ["a"], now: epoch, duration: 2.5)
        // 1s later — well before the 2.5s window closes.
        let pruned = foldFreshMarks(marked, arrivals: [], now: epoch.addingTimeInterval(1), duration: 2.5)
        #expect(pruned["a"] != nil, "a) must still be fresh 1s into a 2.5s window")
    }

    @Test func anEntryPastItsExpiryIsPrunedEvenWithNoNewArrivals() {
        let marked = foldFreshMarks([:], arrivals: ["a"], now: epoch, duration: 2.5)
        // 2.5s later — exactly AT expiry: `> now` (not `>=`) is what
        // survives a fold, so the boundary itself is already gone.
        let pruned = foldFreshMarks(marked, arrivals: [], now: epoch.addingTimeInterval(2.5), duration: 2.5)
        #expect(pruned["a"] == nil, "expiry is exclusive — the exact expiry instant is no longer fresh")
    }

    @Test func aKeyPastExpiryIsPrunedEvenAsOtherKeysRemainFresh() {
        let firstMark = foldFreshMarks([:], arrivals: ["old"], now: epoch, duration: 2.5)
        // 2s later: "old" (marked at epoch) has 0.5s left; "new" is marked
        // fresh right now for its own full 2.5s window.
        let mixed = foldFreshMarks(firstMark, arrivals: ["new"], now: epoch.addingTimeInterval(2), duration: 2.5)
        #expect(mixed["old"] != nil, "old still has 0.5s left")
        #expect(mixed["new"] != nil)

        // 1s further (3s total since epoch): "old"'s window (epoch + 2.5s)
        // has closed; "new"'s (epoch+2s + 2.5s = epoch+4.5s) has not.
        let after = foldFreshMarks(mixed, arrivals: [], now: epoch.addingTimeInterval(3), duration: 2.5)
        #expect(after["old"] == nil, "old must be pruned once its own window closes")
        #expect(after["new"] != nil, "new's independent, later-started window must not be affected")
    }

    @Test func aKeyThatArrivesAgainBeforeExpiryRefreshesItsWindowFromNow() {
        let firstMark = foldFreshMarks([:], arrivals: ["a"], now: epoch, duration: 2.5)
        let refreshed = foldFreshMarks(firstMark, arrivals: ["a"], now: epoch.addingTimeInterval(2), duration: 2.5)
        #expect(refreshed["a"] == epoch.addingTimeInterval(2).addingTimeInterval(2.5), "re-arriving resets the expiry to duration from THIS now, not the original")
    }

    @Test func productionDurationMatchesEventsTsFreshMsAsSeconds() {
        // `Events.tsx` line 37: `FRESH_MS = 2500` (milliseconds) = 2.5s.
        #expect(freshHighlightSeconds == 2.5)
    }
}

@Suite
struct MergeSortKeyTests {
    @Test func isDeterministicAcrossRepeatedCallsOnEqualValues() {
        let a = CPEvent.stepFailed(runID: "r1", stepID: "s1", error: "boom")
        let b = CPEvent.stepFailed(runID: "r1", stepID: "s1", error: "boom")
        #expect(mergeSortKey(a) == mergeSortKey(b))
    }

    @Test func distinguishesEventsThatDifferOnlyByOneField() {
        let a = CPEvent.stepFailed(runID: "r1", stepID: "s1", error: "boom")
        let b = CPEvent.stepFailed(runID: "r1", stepID: "s1", error: "bang")
        #expect(mergeSortKey(a) != mergeSortKey(b))
    }

    @Test func distinguishesDifferentCasesThatShareARunID() {
        let a = CPEvent.runPaused(runID: "r1")
        let b = CPEvent.runResumed(runID: "r1")
        #expect(mergeSortKey(a) != mergeSortKey(b))
    }

    @Test func producesAStableTotalOrderForATsTieInTheMergeSort() {
        // Mirrors how `SituationStore.mergeIncoming` actually sorts:
        // `a.ts != b.ts ? a.ts > b.ts : mergeSortKey(a.event) < mergeSortKey(b.event)`.
        // Two distinct events sharing a `ts` must sort the same way every
        // time — the whole point of round 2's ruling 1 fix.
        let rows = [
            CPEventRow(event: .runPaused(runID: "z"), ts: 1_000, pos: 0),
            CPEventRow(event: .runPaused(runID: "a"), ts: 1_000, pos: 1),
            CPEventRow(event: .runPaused(runID: "m"), ts: 1_000, pos: 2),
        ]
        let sortedOnce = rows.sorted { a, b in
            a.ts != b.ts ? a.ts > b.ts : mergeSortKey(a.event) < mergeSortKey(b.event)
        }
        let sortedAgain = rows.reversed().sorted { a, b in
            a.ts != b.ts ? a.ts > b.ts : mergeSortKey(a.event) < mergeSortKey(b.event)
        }
        let runIDs = sortedOnce.map { $0.event.runID }
        #expect(runIDs == ["a", "m", "z"], "the runPaused keys for a/m/z order a < m < z lexicographically")
        #expect(
            sortedAgain.map { $0.event.runID } == runIDs,
            "the same input set, given to the sort in a different starting order, must land in the identical order"
        )
    }

    // MARK: - Final-review fix, item 4: delimiter-safe composition

    /// The shifted-boundary collision a bare `joined(separator: "|")` allows:
    /// two DIFFERENT events whose field values shift the delimiter across a
    /// boundary produced the identical key (`"stepFailed|r1|s|t|u"` both
    /// ways). Two distinct events comparing EQUAL under the tie-break key
    /// hands their relative order back to the unstable sort — exactly the
    /// non-determinism round 2's ruling 1 set out to remove.
    @Test func doesNotCollideOnAShiftedDelimiterBoundary() {
        let a = CPEvent.stepFailed(runID: "r1", stepID: "s|t", error: "u")
        let b = CPEvent.stepFailed(runID: "r1", stepID: "s", error: "t|u")
        #expect(mergeSortKey(a) != mergeSortKey(b))
    }

    /// The same hazard on an optional field: `nil` must stay distinguishable
    /// from the empty string (and from a value that happens to contain the
    /// delimiter).
    @Test func distinguishesANilOptionalFromAnEmptyStringAndFromADelimiterValue() {
        let nilAgent = CPEvent.stepStarted(runID: "r1", stepID: "s", kind: "agent", agent: nil, host: "h")
        let emptyAgent = CPEvent.stepStarted(runID: "r1", stepID: "s", kind: "agent", agent: "", host: "h")
        let shifted = CPEvent.stepStarted(runID: "r1", stepID: "s", kind: "agent|h", agent: nil, host: nil)
        #expect(mergeSortKey(nilAgent) != mergeSortKey(emptyAgent))
        #expect(mergeSortKey(nilAgent) != mergeSortKey(shifted))
        #expect(mergeSortKey(emptyAgent) != mergeSortKey(shifted))
    }
}
