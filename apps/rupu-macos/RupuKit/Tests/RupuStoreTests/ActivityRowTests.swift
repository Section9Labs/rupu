import Testing
import Foundation
@testable import RupuStore
import RupuAPI
import RupuDesign

@Test func mapsRunListRowFixtureToActivityRow() throws {
    let rows = try JSONDecoder().decode([APIRunListRow].self, from: Fixtures.data("run_list_row.json"))
    #expect(rows.count == 2)

    let running = ActivityRow(rows[0])
    #expect(running.id == "run-01")
    #expect(running.kind == .workflow)
    #expect(running.subject == "nightly-health")
    #expect(running.host == "local")
    #expect(running.trigger == "cron")
    #expect(running.status == .running)
    #expect(running.costUSD == 0.12)
    #expect(running.navigation == .run(id: "run-01", host: "local"))
    #expect(running.startedAt != nil)

    let awaiting = ActivityRow(rows[1])
    #expect(awaiting.id == "run-02")
    #expect(awaiting.host == "mini")
    #expect(awaiting.status == .awaiting)
    #expect(awaiting.durationMS == nil)
    #expect(awaiting.navigation == .run(id: "run-02", host: "mini"))
}

@Test func mapsAgentRunRowFixtureToActivityRow() throws {
    let rows = try JSONDecoder().decode([APIAgentRunRow].self, from: Fixtures.data("agent_run_rows.json"))
    #expect(rows.count == 2)

    let named = ActivityRow(rows[0])
    #expect(named.id == "run-10")
    #expect(named.kind == .agent)
    #expect(named.subject == "rupuso")
    #expect(named.host == "local")
    #expect(named.trigger == "session_turn")
    #expect(named.status == .running)
    #expect(named.navigation == .run(id: "run-10", host: "local"))

    // No `agent`/`host_id` on this row — subject falls back to "agent
    // run", host falls back to "local", and a nil `status` normalizes to
    // the explicit unknown placeholder rather than a guessed real case.
    let unnamed = ActivityRow(rows[1])
    #expect(unnamed.id == "run-11")
    #expect(unnamed.subject == "agent run")
    #expect(unnamed.host == "local")
    #expect(unnamed.status == .unknown("—"))
    #expect(unnamed.startedAt == nil)
    #expect(unnamed.navigation == .run(id: "run-11", host: nil))
}

@Test func mapsAutoflowEventRowFixtureToActivityRow() throws {
    let rows = try JSONDecoder().decode([APIAutoflowEventRow].self, from: Fixtures.data("autoflow_event_rows.json"))
    #expect(rows.count == 2)

    let launched = ActivityRow(rows[0])
    #expect(launched.id == "evt-1")
    #expect(launched.kind == .autoflow)
    #expect(launched.subject == "nightly-health")
    #expect(launched.status == .running)
    #expect(launched.navigation == .run(id: "run-20", host: "local"))

    // `cycle_failed` carries no `run_id` — subject falls back to nothing
    // needed here since `workflow` is still present, but navigation must
    // be `.none` (nothing to navigate to) rather than a run link with a
    // missing id.
    let cycleFailed = ActivityRow(rows[1])
    #expect(cycleFailed.id == "evt-2")
    #expect(cycleFailed.subject == "nightly-health")
    #expect(cycleFailed.navigation == .none)
    #expect(cycleFailed.status == .unknown("—"))
}

@Test func autoflowSubjectFallsBackToKindWhenWorkflowIsNil() {
    let row = APIAutoflowEventRow(
        eventID: "evt-9",
        cycleID: "cycle-9",
        at: "2026-08-20T12:00:00Z",
        kind: "worker_started",
        workflow: nil,
        issueDisplayRef: nil,
        runID: nil,
        status: nil,
        workerName: "worker-b",
        detail: nil,
        usage: APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0),
        turns: nil,
        durationMS: nil,
        hostID: nil
    )
    #expect(ActivityRow(row).subject == "worker_started")
}

@Test func mapsSessionRowFixtureToActivityRow() throws {
    let rows = try JSONDecoder().decode([APISessionRow].self, from: Fixtures.data("session_rows.json"))
    #expect(rows.count == 1)

    let row = ActivityRow(rows[0])
    #expect(row.id == "sess-1")
    #expect(row.kind == .session)
    #expect(row.subject == "rupuso")
    #expect(row.project == "ws-1")
    #expect(row.host == "local")
    #expect(row.navigation == .session(id: "sess-1"))
    // activeRunID present -> .running, regardless of lastError.
    #expect(row.status == .running)
    #expect(row.startedAt != nil)
}

@Test func sessionRowStatusFallsBackToFailedThenCompleted() {
    let base = APISessionRow(
        sessionID: "sess-2",
        agentName: "rupuso",
        model: "claude-sonnet-4-6",
        providerName: "anthropic",
        totalTurns: 1,
        totalTokensIn: 0,
        totalTokensOut: 0,
        totalTokensCached: 0,
        createdAt: "2026-08-20T11:00:00Z",
        updatedAt: "2026-08-20T11:00:00Z",
        activeRunID: nil,
        lastError: "boom",
        target: nil,
        workspaceID: "ws-2",
        scope: "active",
        usage: nil,
        hostID: nil
    )
    #expect(ActivityRow(base).status == .failed)

    var completedNoError = base
    completedNoError = APISessionRow(
        sessionID: base.sessionID, agentName: base.agentName, model: base.model, providerName: base.providerName,
        totalTurns: base.totalTurns, totalTokensIn: base.totalTokensIn, totalTokensOut: base.totalTokensOut,
        totalTokensCached: base.totalTokensCached, createdAt: base.createdAt, updatedAt: base.updatedAt,
        activeRunID: nil, lastError: nil, target: base.target, workspaceID: base.workspaceID,
        scope: base.scope, usage: base.usage, hostID: base.hostID
    )
    #expect(ActivityRow(completedNoError).status == .completed)
}

@Test func parseISOHandlesFractionalAndPlainSeconds() {
    #expect(ActivityRow.parseISO(nil) == nil)
    #expect(ActivityRow.parseISO("2026-08-20T12:00:00Z") != nil)
    #expect(ActivityRow.parseISO("2026-08-20T12:00:00.123Z") != nil)
    #expect(ActivityRow.parseISO("not-a-date") == nil)
}

@Test func normalizeMapsAllKnownRawStatuses() {
    let table: [(String?, ActivityStatus)] = [
        ("pending", .pending),
        ("running", .running),
        ("completed", .completed),
        ("failed", .failed),
        ("awaiting_approval", .awaiting),
        ("rejected", .rejected),
        ("cancelled", .cancelled),
        ("paused", .paused),
        ("ok", .completed),
        ("error", .failed),
        ("aborted", .cancelled),
        (nil, .unknown("—")),
        ("some_weird_status", .unknown("some_weird_status")),
    ]
    for (raw, expected) in table {
        #expect(ActivityStatus.normalize(raw) == expected, "normalize(\(String(describing: raw)))")
    }
}

@Test func toneMapsEveryStatusCase() {
    let table: [(ActivityStatus, RunTone)] = [
        (.running, .run),
        (.completed, .done),
        (.failed, .fail),
        (.rejected, .fail),
        (.awaiting, .waiting),
        (.paused, .pause),
        (.cancelled, .pause),
        (.pending, .pause),
        (.unknown("x"), .pause),
    ]
    for (status, expected) in table {
        #expect(status.tone == expected, "tone(\(status))")
    }
}
