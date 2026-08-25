import Testing
import Foundation
import RupuAPI
import RupuDesign
@testable import RupuSituation

// Ported test table from `crates/rupu-cp/web/src/lib/situationRoom/
// cards.test.ts` (verified full read, 99 lines) — same inputs, same
// expected outputs, adapted to `CPEvent`'s typed cases (each event
// constructed directly as the matching `CPEvent` case instead of a raw
// `RunEvent` object literal) and to `cardForEvent`'s Swift signature (no
// caller-supplied `key`; `ts` is a `Date?` instead of a raw ms number).

private let ts1000 = Date(timeIntervalSince1970: 1) // 1000ms since epoch

@Test func anAwaitingApprovalStepIsAnApprovableAwaitCard() {
    // cards.test.ts lines 19-28
    let ev = CPEvent.stepAwaitingApproval(runID: "r1", stepID: "deploy", reason: "ship it?")
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.group == .await_)
    #expect(c.accent == .await_)
    #expect(c.approvable == Approvable(runID: "r1", stepID: "deploy", reason: "ship it?"))
    #expect(c.detail == "ship it?")
}

@Test func aStepFailureIsAnErrorGroupCardCarryingTheErrorText() {
    // cards.test.ts lines 30-36
    let ev = CPEvent.stepFailed(runID: "r1", stepID: "checkout", error: "clone timed out")
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.group == .error)
    #expect(c.accent == .error)
    #expect(c.detail == "clone timed out")
}

@Test func anAgentStepStartedIsAScanningActivityCardAttributedToTheAgent() {
    // cards.test.ts lines 38-45
    let ev = CPEvent.stepStarted(runID: "r1", stepID: "audit", kind: "agent", agent: "oracle-sec", host: nil)
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.group == .activity)
    #expect(c.badge == "Scanning")
    #expect(c.agent == "oracle-sec")
    #expect(c.title.contains("oracle-sec"))
}

@Test func aNoteLessStepWorkingHeartbeatIsDroppedNotAnEmptyRow() {
    // cards.test.ts lines 47-50
    let ev = CPEvent.stepWorking(runID: "r1", stepID: "audit", note: nil, transcriptPath: nil)
    #expect(cardForEvent(ev, ts: ts1000) == nil)
}

@Test func aStepWorkingWithANoteRendersTheNoteAsDetail() {
    // cards.test.ts lines 52-56
    let ev = CPEvent.stepWorking(runID: "r1", stepID: "audit", note: "reading routes", transcriptPath: nil)
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.detail == "reading routes")
}

@Test func aPanelRoundSurfacesTheRoundCounterAndMaxSeverityRemaining() {
    // cards.test.ts lines 58-66
    let ev = CPEvent.panelRound(
        runID: "r1", stepID: "panel", round: 2, maxIterations: 4, maxSeverityRemaining: "high"
    )
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.form == .panel)
    #expect(c.title.contains("round 2/4"))
    #expect(c.detail?.contains("high") == true)
}

// MARK: - unknown / not-in-cards.ts event kinds → the web's degraded
// fallback card (cards.ts lines 99-111), not nil. Fix round 1, finding 1:
// task review found the earlier "returns nil" behavior contradicted the
// verified web source; inverted to match cards.ts.

@Test func anUnknownEventTypeGetsTheWebsFallbackCardNotNil() {
    let ev = CPEvent.unknown(type: "future_thing", runID: "r9")
    let c = cardForEvent(ev, ts: ts1000)!
    #expect(c.form == .activity)
    #expect(c.group == .activity)
    #expect(c.accent == .brand)
    #expect(c.badge == "future thing") // underscores → spaces
    #expect(c.title == "future_thing") // no step_id on `.unknown` — falls back to the raw type
    #expect(c.runID == "r9")
}

@Test func dispatchEventsNotInCardsTsKnownEventTypesAlsoGetTheFallbackCard() {
    let started = CPEvent.dispatchStarted(runID: "r1", subRunID: "sr1", agent: "sec", transcriptPath: "t.jsonl")
    let completed = CPEvent.dispatchCompleted(runID: "r1", subRunID: "sr1", success: true, tokensIn: 1, tokensOut: 2)

    let startedCard = cardForEvent(started, ts: ts1000)!
    #expect(startedCard.form == .activity)
    #expect(startedCard.accent == .brand)
    #expect(startedCard.badge == "dispatch started")
    #expect(startedCard.title == "dispatch_started")
    #expect(startedCard.runID == "r1")

    let completedCard = cardForEvent(completed, ts: ts1000)!
    #expect(completedCard.badge == "dispatch completed")
    #expect(completedCard.title == "dispatch_completed")
    #expect(completedCard.runID == "r1")
}

// MARK: - cardForFinding — ported from cards.test.ts lines 69-99.

private func baseFinding(
    severity: String = "HIGH", // cards.test.ts line 74's exact literal — see note below
    filePath: String? = "src/routes/billing.ts",
    lineRange: [UInt32]? = [16, 21]
) -> APIFinding {
    // cards.test.ts's base fixture uses uppercase `'HIGH'` specifically to
    // exercise `normFindingSeverity`'s `raw.toLowerCase()`. Fix round 1,
    // finding 2: `cardForFinding` now lowercases the wire string before
    // calling `Severity(wireString:)` (see that call site's comment), so
    // this restores the web's exact uppercase input instead of the
    // lowercase workaround an earlier pass used.
    APIFinding(
        id: "f-1", summary: "Broken org-scoping on GET /invoice/:id", severity: severity, scope: "line",
        filePath: filePath, lineRange: lineRange, wsID: "ws1", project: "billing-api", targetID: "t1",
        workflowName: nil, permalink: nil,
        rationale: "orgId is checked for truthiness, not against the caller",
        codeExcerpt: "if (invoice.orgId) {",
        declaredAt: "2026-07-21T10:00:00Z"
    )
}

@Test func normalizesSeverityBuildsAFileLineRefAndKeepsTheRealCodeExcerpt() {
    // cards.test.ts lines 78-92
    let c = cardForFinding(baseFinding())
    #expect(c.group == .finding)
    #expect(c.severity == .high)
    #expect(c.accent == .severity(.high))
    #expect(c.badge == "High")
    #expect(c.fileRef == "src/routes/billing.ts:16-21")
    #expect(c.code == "if (invoice.orgId) {")
    #expect(c.detail?.contains("truthiness") == true)

    // Independent oracle for the ts field — computed via Calendar/
    // DateComponents rather than reusing `rfc3339ToMS` (the production
    // parser), so this is a genuine check, not a tautology.
    var comps = DateComponents()
    comps.year = 2026; comps.month = 7; comps.day = 21; comps.hour = 10; comps.minute = 0; comps.second = 0
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "UTC")!
    let expected = Int64((cal.date(from: comps)!.timeIntervalSince1970 * 1000).rounded())
    #expect(c.ts == expected)

    // provenance for the Code-viewer deep link
    #expect(c.wsID == "ws1")
    #expect(c.filePath == "src/routes/billing.ts")
    #expect(c.fileLine == 16)
}

@Test func anUnknownSeverityFallsBackToInfoAndAMissingFileHasNoRef() {
    // cards.test.ts lines 94-98
    let c = cardForFinding(baseFinding(severity: "bogus", filePath: nil, lineRange: nil))
    #expect(c.severity == .info)
    #expect(c.fileRef == nil)
}

// MARK: - mergeStream — no direct web counterpart (see StreamCards.swift's
// `mergeStream` doc comment for the two web locations this synthesizes:
// `Events.tsx`'s newest-first sort + its upstream `identityOf` dedup).

@Test func mergeStreamOrdersNewestFirst() {
    let a = cardForEvent(.runPaused(runID: "r1"), ts: Date(timeIntervalSince1970: 1))!
    let b = cardForEvent(.runResumed(runID: "r2"), ts: Date(timeIntervalSince1970: 3))!
    let c = cardForEvent(.runPaused(runID: "r3"), ts: Date(timeIntervalSince1970: 2))!
    let out = mergeStream([a, b, c], max: 10)
    #expect(out.map(\.runID) == ["r2", "r3", "r1"])
}

@Test func mergeStreamDedupsByContentIdentityExcludingTsKeepingTheFirstArrivedOccurrence() {
    // Same event content (same CPEvent case + associated values), replayed
    // at two different timestamps — mirrors the web's history↔live replay
    // scenario `identityOf` guards against (`Events.tsx` lines 57-68). Fix
    // round 1, finding 5: the web's `seenRef` gate keeps whichever copy is
    // *ingested first* — here, `older` (passed first in the input array) —
    // regardless of which one carries the "newer" `ts`; an earlier pass here
    // sorted before deduping and so kept `newer` instead. Inverted.
    let older = cardForEvent(.runPaused(runID: "r1"), ts: Date(timeIntervalSince1970: 1))!
    let newer = cardForEvent(.runPaused(runID: "r1"), ts: Date(timeIntervalSince1970: 5))!
    let distinct = cardForEvent(.runResumed(runID: "r2"), ts: Date(timeIntervalSince1970: 3))!
    let out = mergeStream([older, newer, distinct], max: 10)
    #expect(out.count == 2)
    // Newest-first sort still applies to the deduped set: distinct (ts=3000)
    // sorts ahead of the surviving older duplicate (ts=1000).
    #expect(out[0].runID == "r2")
    #expect(out[1].runID == "r1")
    #expect(out[1].ts == 1000) // the first-arrived occurrence survived, not the newest
}

@Test func mergeStreamCapsAtMax() {
    let cards = (0..<5).map { i in
        cardForEvent(.runPaused(runID: "r\(i)"), ts: Date(timeIntervalSince1970: Double(i)))!
    }
    #expect(mergeStream(cards, max: 2).count == 2)
    #expect(mergeStream(cards, max: 0).isEmpty)
}

@Test func mergeStreamBreaksATsTieDeterministicallyByKey() {
    // Fix round 1, finding 4: two distinct cards sharing a `ts` must order
    // the same way regardless of input order — proof the sort doesn't ride
    // Swift's incidental input-order stability alone.
    let a = cardForEvent(.runPaused(runID: "r1"), ts: ts1000)!
    let b = cardForEvent(.runPaused(runID: "r2"), ts: ts1000)!
    let forward = mergeStream([a, b], max: 10)
    let reversed = mergeStream([b, a], max: 10)
    #expect(forward.map(\.key) == reversed.map(\.key))
    #expect(forward.map(\.key) == forward.map(\.key).sorted()) // key-ascending on a ts tie
}

// MARK: - Final-review fix, item 4: `contentIdentityKey` is delimiter-safe

/// `StreamCard.key` IS `contentIdentityKey`'s output and is what
/// `mergeStream` dedups on, so a key collision silently collapses two
/// genuinely distinct rows into one on the live wall. A bare
/// `joined(separator: "|")` allowed exactly that whenever a free-form field
/// (`error`, `reason`, `note`, paths) contained the delimiter: the shifted
/// boundary below produced the identical string both ways.
@Test func contentIdentityKeyDoesNotCollideOnAShiftedDelimiterBoundary() {
    let a = cardForEvent(.stepFailed(runID: "r1", stepID: "s|t", error: "u"), ts: ts1000)!
    let b = cardForEvent(.stepFailed(runID: "r1", stepID: "s", error: "t|u"), ts: ts1000)!
    #expect(a.key != b.key)
    // ...and the dedup they feed must therefore keep both rows.
    #expect(mergeStream([a, b], max: 10).count == 2)
}

/// The same hazard on an optional field: `nil` must stay distinguishable
/// from the empty string and from a neighbouring value carrying the
/// delimiter.
@Test func contentIdentityKeyDistinguishesNilFromEmptyAndFromADelimiterValue() {
    let nilAgent = cardForEvent(.stepStarted(runID: "r1", stepID: "s", kind: "agent", agent: nil, host: "h"), ts: ts1000)!
    let emptyAgent = cardForEvent(.stepStarted(runID: "r1", stepID: "s", kind: "agent", agent: "", host: "h"), ts: ts1000)!
    let shifted = cardForEvent(.stepStarted(runID: "r1", stepID: "s", kind: "agent|h", agent: nil, host: nil), ts: ts1000)!
    #expect(nilAgent.key != emptyAgent.key)
    #expect(nilAgent.key != shifted.key)
    #expect(emptyAgent.key != shifted.key)
}
