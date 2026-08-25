import Testing
import Foundation
import RupuAPI
import RupuDesign
@testable import RupuSituation

// Ported test table from `crates/rupu-cp/web/src/lib/situationRoom/
// roster.test.ts` (verified full read, 169 lines) — same inputs, same
// expected outputs, adapted to CPEvent's typed cases and to
// `RupuAPI.APIProjectRow`/`CPEventRow` as the Swift stand-ins for the web's
// `ProjectRow`/`{ts, event}` item shape.

private func project(
    ws: String, name: String, branch: String? = "main", lastActive: String? = nil
) -> APIProjectRow {
    APIProjectRow(
        wsID: ws, name: name, runCount: 0, lastRunAt: nil,
        usage: APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0),
        branch: branch, lastActive: lastActive
    )
}

private func finding(ws: String, sev: String) -> APIFinding {
    APIFinding(
        id: "\(ws)-\(sev)-\(UUID().uuidString)", summary: "s", severity: sev, scope: "",
        filePath: nil, lineRange: nil, wsID: ws, project: ws, targetID: "t",
        workflowName: nil, permalink: nil, rationale: "r", declaredAt: "2026-07-21T00:00:00Z"
    )
}

private func act(_ runID: String, _ state: RunActivity.State, _ ts: Int64, _ action: String? = nil) -> RunActivity {
    RunActivity(runID: runID, state: state, action: action, ts: ts)
}

private func item(_ ts: Int64, _ event: CPEvent) -> CPEventRow {
    CPEventRow(event: event, ts: ts, pos: 0)
}

// MARK: - findingsByWorkspace — roster.test.ts lines 38-44

@Test func findingsByWorkspaceBucketsCountsPerWorkspaceAndSeverity() {
    let m = findingsByWorkspace([
        finding(ws: "ws1", sev: "high"), finding(ws: "ws1", sev: "high"),
        finding(ws: "ws1", sev: "low"), finding(ws: "ws2", sev: "critical"),
    ])
    #expect(m["ws1"]?.high == 2)
    #expect(m["ws1"]?.low == 1)
    #expect(m["ws1"]?.total == 3)
    #expect(m["ws2"]?.critical == 1)
    #expect(m["ws2"]?.total == 1)
}

// MARK: - buildRoster (foldRoster) — roster.test.ts lines 46-73

private let rosterProjects = [
    project(ws: "ws1", name: "billing-api"), project(ws: "ws2", name: "notes-svc"), project(ws: "ws3", name: "idle-app"),
]
private let rosterRunToWs = ["rA": "ws1", "rB": "ws2"]

@Test func buildRosterOrdersAwaitingRunningIdleAndAttributesFindings() {
    let activity: [String: RunActivity] = [
        "rA": act("rA", .running, 100, "oracle-sec · audit"),
        "rB": act("rB", .awaiting, 200),
    ]
    let findings = [finding(ws: "ws1", sev: "high"), finding(ws: "ws2", sev: "critical")]
    let roster = foldRoster(projects: rosterProjects, runToWs: rosterRunToWs, activity: activity, findings: findings)

    #expect(roster.map(\.name) == ["notes-svc", "billing-api", "idle-app"])
    #expect(roster[0].status == .await_)
    #expect(roster[1].status == .running)
    #expect(roster[1].action == "oracle-sec · audit")
    #expect(roster[2].status == .idle)
    #expect(roster[1].findings.high == 1)
    #expect(roster[0].findings.critical == 1)
    #expect(projectsLive(roster) == 2)
}

@Test func aRunWithNoKnownWorkspaceDoesNotAttributeToAnyProject() {
    let activity: [String: RunActivity] = ["orphan": act("orphan", .running, 100)]
    let roster = foldRoster(projects: rosterProjects, runToWs: [:], activity: activity, findings: [])
    #expect(roster.allSatisfy { $0.status == .idle })
}

// MARK: - deriveActivity — roster.test.ts lines 75-114

@Test func deriveActivityFoldsTheNewestEventPerRunAMidStepRunReadsRunning() {
    let m = deriveActivity([
        item(300, .stepWorking(runID: "r1", stepID: "audit", note: "scanning", transcriptPath: nil)),
        item(200, .stepStarted(runID: "r1", stepID: "audit", kind: "linear", agent: "sec", host: nil)),
    ])
    #expect(m["r1"]?.state == .running)
    #expect(m["r1"]?.action == "scanning")
}

@Test func runCompletedWithACancelledStatusEndsTheRunNoForeverSpinner() {
    let m = deriveActivity([
        item(400, .runCompleted(runID: "r1", status: "cancelled", finishedAt: "x")),
        item(300, .stepWorking(runID: "r1", stepID: "audit", note: nil, transcriptPath: nil)),
    ])
    #expect(m["r1"]?.state == .done)
}

@Test func pausedRunsReadPausedNotRunningAResumeFlipsThemBack() {
    let paused = deriveActivity([
        item(300, .runPaused(runID: "r1")),
        item(200, .stepWorking(runID: "r1", stepID: "audit", note: nil, transcriptPath: nil)),
    ])
    #expect(paused["r1"]?.state == .paused)

    let stepPaused = deriveActivity([item(300, .stepPaused(runID: "r2", stepID: "audit"))])
    #expect(stepPaused["r2"]?.state == .paused)

    // run_resumed is NOT one of deriveActivity's explicitly-handled event
    // kinds on the web either (roster.ts lines 42-54 has no `run_resumed`
    // case — only `step_resumed`) — it falls to `default`, leaving the
    // initial `state = 'running'` untouched. Ported as-is, not "fixed" to a
    // symmetric run_paused/run_resumed pair.
    let resumed = deriveActivity([
        item(400, .runResumed(runID: "r1")),
        item(300, .runPaused(runID: "r1")),
    ])
    #expect(resumed["r1"]?.state == .running)
}

// MARK: - reconcileActivity — roster.test.ts lines 116-158

@Test func reconcileActivityOverridesALiveLookingActivityWhenThePersistedStatusIsTerminal() {
    let activity: [String: RunActivity] = [
        "r1": act("r1", .running, 100, "audit"),
        "r2": act("r2", .awaiting, 200),
        "r3": act("r3", .running, 300, "build"),
    ]
    let out = reconcileActivity(activity, persistedStatus: ["r1": "cancelled", "r2": "rejected"])
    #expect(out["r1"]?.state == .done)
    #expect(out["r2"]?.state == .failed)
    #expect(out["r3"]?.state == .running)
    #expect(out["r3"]?.action == "build")
}

@Test func reconcileActivityNeverDowngradesAnAlreadyTerminalActivity() {
    let activity: [String: RunActivity] = ["r1": act("r1", .done, 100)]
    let out = reconcileActivity(activity, persistedStatus: ["r1": "failed"])
    #expect(out["r1"]?.state == .done)
}

@Test func reconcileActivityPausedActivityWithATerminalPersistedStatusAlsoCloses() {
    let activity: [String: RunActivity] = ["r1": act("r1", .paused, 100)]
    let out = reconcileActivity(activity, persistedStatus: ["r1": "failed"])
    #expect(out["r1"]?.state == .failed)
}

@Test func rosterPausedRunsDoNotCountAsLive() {
    let roster = foldRoster(
        projects: [project(ws: "ws1", name: "billing-api")],
        runToWs: ["rA": "ws1"],
        activity: ["rA": act("rA", .paused, 100)],
        findings: []
    )
    #expect(roster[0].status == .idle)
}

// MARK: - buildVitals — roster.test.ts lines 160-169

@Test func buildVitalsDegradesMissingSourcesToZerosNeverFabricates() {
    let v = buildVitals(
        activeRuns: nil, awaiting: nil, findings: nil,
        projectsLive: 2, projectsTotal: 5, errors: 1, eventsPerMin: 12
    )
    #expect(v.activeRuns == 0)
    #expect(v.awaiting == 0)
    #expect(v.findings.total == 0)
    #expect(v.projectsLive == 2)
    #expect(v.eventsPerMin == 12)
}

// MARK: - EventRateRing / eventsPerMinute — no direct web function to port
// (see Vitals.swift's file header). Exercises the Events.tsx line-174
// arithmetic + the ring buffer's fixed-length newest-last behavior.

@Test func eventsPerMinuteMatchesTheWebsLine174Arithmetic() {
    // Math.round((12 * 60_000) / 5_000) == 144
    #expect(eventsPerMinute(12, windowMS: 5_000) == 144)
    #expect(eventsPerMinute(0, windowMS: 5_000) == 0)
}

@Test func eventRateRingKeepsAFixedLengthNewestLastWindow() {
    var ring = EventRateRing(capacity: 3)
    #expect(ring.samples == [0, 0, 0])
    ring.tick(5)
    ring.tick(7)
    ring.tick(9)
    #expect(ring.samples == [5, 7, 9])
    ring.tick(11) // oldest (5) drops off
    #expect(ring.samples == [7, 9, 11])
}
