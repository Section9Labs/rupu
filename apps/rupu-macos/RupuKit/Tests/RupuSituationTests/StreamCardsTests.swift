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

// MARK: - unknown / not-in-cards.ts event kinds → nil (see StreamCards.swift
// file header for why this is a deliberate, cited divergence from cards.ts's
// own fallback-card behavior for these two categories).

@Test func anUnknownEventTypeMapsToNilRatherThanAFallbackCard() {
    let ev = CPEvent.unknown(type: "future_thing", runID: "r9")
    #expect(cardForEvent(ev, ts: ts1000) == nil)
}

@Test func aDispatchEventNotInCardsTsKnownEventTypesAlsoMapsToNil() {
    let started = CPEvent.dispatchStarted(runID: "r1", subRunID: "sr1", agent: "sec", transcriptPath: "t.jsonl")
    let completed = CPEvent.dispatchCompleted(runID: "r1", subRunID: "sr1", success: true, tokensIn: 1, tokensOut: 2)
    #expect(cardForEvent(started, ts: ts1000) == nil)
    #expect(cardForEvent(completed, ts: ts1000) == nil)
}

// MARK: - cardForFinding — ported from cards.test.ts lines 69-99.

private func baseFinding(
    severity: String = "high", // adapted from cards.test.ts's 'HIGH' — see note below
    filePath: String? = "src/routes/billing.ts",
    lineRange: [UInt32]? = [16, 21]
) -> APIFinding {
    // `Severity(wireString:)` (`RupuDesign/Tokens.swift`) is a case-sensitive
    // exact match — unlike the web's `normFindingSeverity`
    // (`raw.toLowerCase()` first), it does not lowercase before matching, so
    // it never sees an uppercase wire value in practice (rupu-coverage's
    // `Severity` enum is `#[serde(rename_all = "lowercase")]`, confirmed by
    // both `findings_global.json`'s fixture values and
    // `SeverityWireMappingTests`, which only pins lowercase inputs). The web
    // test's literal `'HIGH'` input exercises normFindingSeverity's
    // case-insensitivity specifically — a behavior `Severity(wireString:)`
    // doesn't have and this task doesn't touch (RupuDesign is out of this
    // task's file scope) — so this port uses the real wire casing ('high')
    // instead of asserting a false pass. Flagged in the task-6 report.
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

@Test func mergeStreamDedupsByContentIdentityExcludingTsKeepingTheNewestOccurrence() {
    // Same event content (same CPEvent case + associated values), replayed
    // at two different timestamps — mirrors the web's history↔live replay
    // scenario `identityOf` guards against (`Events.tsx` lines 57-68).
    let older = cardForEvent(.runPaused(runID: "r1"), ts: Date(timeIntervalSince1970: 1))!
    let newer = cardForEvent(.runPaused(runID: "r1"), ts: Date(timeIntervalSince1970: 5))!
    let distinct = cardForEvent(.runResumed(runID: "r2"), ts: Date(timeIntervalSince1970: 3))!
    let out = mergeStream([older, newer, distinct], max: 10)
    #expect(out.count == 2)
    #expect(out[0].runID == "r1")
    #expect(out[0].ts == 5000) // the newest occurrence survived, not the oldest
    #expect(out[1].runID == "r2")
}

@Test func mergeStreamCapsAtMax() {
    let cards = (0..<5).map { i in
        cardForEvent(.runPaused(runID: "r\(i)"), ts: Date(timeIntervalSince1970: Double(i)))!
    }
    #expect(mergeStream(cards, max: 2).count == 2)
    #expect(mergeStream(cards, max: 0).isEmpty)
}
