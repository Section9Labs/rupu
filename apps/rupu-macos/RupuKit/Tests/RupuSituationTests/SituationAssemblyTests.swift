import Testing
import Foundation
import RupuAPI
@testable import RupuSituation

// Tests for `assembleSituation`/`resolveCardProject` — the glue this module
// needs `RupuStore` for (see `SituationAssembly.swift`'s file header). Plain
// `@Test func`s, no `@MainActor` needed: every input here is a plain
// `Sendable` value type built by hand, not read off a live `SituationStore`.

private func finding(id: String, declaredAt: String, project: String = "acme") -> APIFinding {
    APIFinding(
        id: id, summary: "s-\(id)", severity: "high", scope: "target",
        filePath: nil, lineRange: nil, wsID: "ws1", project: project, targetID: "t1",
        workflowName: nil, permalink: nil, rationale: "r", declaredAt: declaredAt
    )
}

private func project(wsID: String, name: String) -> APIProjectRow {
    APIProjectRow(
        wsID: wsID, name: name, runCount: 0, lastRunAt: nil,
        usage: APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0)
    )
}

@Test func findingCardsInTheMergedStreamAreOrderedNewestDeclaredAtFirst() {
    let older = finding(id: "old", declaredAt: "2026-01-01T00:00:00Z")
    let newer = finding(id: "new", declaredAt: "2026-06-01T00:00:00Z")

    // Passed in ARRIVAL order (older first) — the assembly must still sort
    // findings newest-`declared_at`-first before merging into the stream
    // (`Events.tsx` lines 264-267's `.sort((a, b) => Date.parse(b.declared_at) - Date.parse(a.declared_at))`).
    let snapshot = assembleSituation(
        eventRows: [], findings: [older, newer], findingsSummary: nil, projects: [],
        runToWorkspace: [:], runTerminalStatus: [:], dashboard: nil, eventsPerMin: 0
    )

    #expect(snapshot.cards.map(\.key) == ["finding:new", "finding:old"])
}

@Test func mergedCardCapIsEnforced() {
    let events = (0..<10).map { i in
        CPEventRow(event: .stepFailed(runID: "r", stepID: "s\(i)", error: "e\(i)"), ts: Int64(i), pos: i)
    }

    let snapshot = assembleSituation(
        eventRows: events, findings: [], findingsSummary: nil, projects: [],
        runToWorkspace: [:], runTerminalStatus: [:], dashboard: nil, eventsPerMin: 0,
        cardCap: 3
    )

    #expect(snapshot.cards.count == 3)
}

@Test func aContentIdenticalDuplicateEventRowCollapsesToOneCardInTheAssembledStream() {
    // Same `CPEvent` twice (a history row and a live replay of it, in
    // arrival order) — `ts` differs, content doesn't.
    let historyRow = CPEventRow(event: .runPaused(runID: "run-1"), ts: 1_000, pos: 0)
    let replayRow = CPEventRow(event: .runPaused(runID: "run-1"), ts: 9_000, pos: -1)

    let snapshot = assembleSituation(
        eventRows: [historyRow, replayRow], findings: [], findingsSummary: nil, projects: [],
        runToWorkspace: [:], runTerminalStatus: [:], dashboard: nil, eventsPerMin: 0
    )

    #expect(snapshot.cards.count == 1)
    #expect(snapshot.cards.first?.ts == 1_000, "first-arrived (the real historical ts) must win, not the replay's")
}

@Test func vitalsProjectsLiveDelegatesToTheFoldedRosterNotAnIndependentCount() {
    let awaitingEvent = CPEventRow(
        event: .stepAwaitingApproval(runID: "run-1", stepID: "gate", reason: "ok?"), ts: 1_000, pos: 0
    )
    let snapshot = assembleSituation(
        eventRows: [awaitingEvent], findings: [], findingsSummary: nil,
        projects: [project(wsID: "ws1", name: "Acme")],
        runToWorkspace: ["run-1": "ws1"], runTerminalStatus: [:], dashboard: nil, eventsPerMin: 0
    )

    #expect(snapshot.roster.count == 1)
    #expect(snapshot.roster.first?.status == .await_)
    #expect(snapshot.vitals.projectsLive == 1)
    #expect(snapshot.vitals.projectsTotal == 1)
}

@Test func errorsVitalCountsTheMergedStreamsErrorGroupCards() {
    let events = [
        CPEventRow(event: .stepFailed(runID: "r", stepID: "s1", error: "boom"), ts: 1_000, pos: 0),
        CPEventRow(event: .runPaused(runID: "r"), ts: 900, pos: 1),
    ]
    let snapshot = assembleSituation(
        eventRows: events, findings: [], findingsSummary: nil, projects: [],
        runToWorkspace: [:], runTerminalStatus: [:], dashboard: nil, eventsPerMin: 0
    )
    #expect(snapshot.vitals.errors == 1)
}

// MARK: - resolveCardProject

@Test func resolveCardProjectPrefersTheCardsOwnProjectNameOverRunResolution() {
    let card = cardForFinding(finding(id: "f1", declaredAt: "2026-01-01T00:00:00Z", project: "direct-project"))
    let result = resolveCardProject(card, runToWorkspace: [:], projectsByWorkspace: [:])
    #expect(result.label == "direct-project")
}

@Test func resolveCardProjectFallsBackThroughRunIDToWorkspaceToProjectRow() {
    let card = cardForEvent(.runPaused(runID: "run-1"), ts: nil)!
    let result = resolveCardProject(
        card,
        runToWorkspace: ["run-1": "ws1"],
        projectsByWorkspace: ["ws1": project(wsID: "ws1", name: "Acme")]
    )
    #expect(result.label == "Acme")
}

@Test func resolveCardProjectReturnsNilWhenTheRunIsNotYetResolved() {
    let card = cardForEvent(.runPaused(runID: "run-unresolved"), ts: nil)!
    let result = resolveCardProject(card, runToWorkspace: [:], projectsByWorkspace: [:])
    #expect(result.label == nil)
}
