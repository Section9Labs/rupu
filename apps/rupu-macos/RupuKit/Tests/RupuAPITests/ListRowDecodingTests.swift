import Testing
import Foundation
@testable import RupuAPI

@Test func decodesRunListRowFixture() throws {
    let rows = try JSONDecoder().decode([APIRunListRow].self, from: Fixtures.data("run_list_row.json"))
    #expect(rows.count == 2)
    #expect(rows[1].status == "awaiting_approval")
    #expect(rows[1].hostID == "mini")
    #expect(rows[0].workflowName == "nightly-health")
    #expect(rows[0].usage.totalTokens == 1200)
    #expect(rows[1].finishedAt == nil)
    #expect(rows[1].durationMS == nil)
}

@Test func decodesAgentRunRowsFixture() throws {
    let rows = try JSONDecoder().decode([APIAgentRunRow].self, from: Fixtures.data("agent_run_rows.json"))
    #expect(rows.count == 2)
    #expect(rows[0].source == "session")
    #expect(rows[0].runID == "run-10")
    #expect(rows[0].sessionID == "sess-1")
    #expect(rows[1].source == "standalone")
    #expect(rows[1].agent == nil)
    #expect(rows[1].status == nil)
}

@Test func decodesAutoflowEventRowsFixture() throws {
    let rows = try JSONDecoder().decode([APIAutoflowEventRow].self, from: Fixtures.data("autoflow_event_rows.json"))
    #expect(rows.count == 2)
    #expect(rows[0].eventID == "evt-1")
    #expect(rows[0].kind == "run_launched")
    let cycleFailed = rows[1]
    #expect(cycleFailed.kind == "cycle_failed")
    #expect(cycleFailed.runID == nil)
    #expect(cycleFailed.detail != nil)
    #expect(cycleFailed.turns == nil)
}

/// `autoflow_cycle_rows.json` is hand-authored (not machine-regenerated via
/// `make macos-fixtures`'s `check_fixture` mechanism) — no Rust-side
/// `..._fixture_is_current` test exists for `AutoflowCycleRow` yet, since
/// this Swift-side `APIAutoflowCycleRow` is new (Task 4b: the autoflow
/// Cycles sub-table) but the Rust struct it mirrors (`crates/rupu-cp/src/
/// api/run_streams.rs`'s `AutoflowCycleRow`) already existed, unchanged —
/// no RupuAPI serde type actually changed shape, so there's no drift for
/// the cargo gate to catch. Field names/shapes here were verified directly
/// against that struct's `#[derive(Serialize)]` output.
@Test func decodesAutoflowCycleRowsFixture() throws {
    let rows = try JSONDecoder().decode([APIAutoflowCycleRow].self, from: Fixtures.data("autoflow_cycle_rows.json"))
    #expect(rows.count == 2)
    #expect(rows[0].cycleID == "cycle-1")
    #expect(rows[0].mode == "tick")
    #expect(rows[0].workerName == "worker-a")
    #expect(rows[0].runIDs == ["run-20", "run-21"])
    #expect(rows[0].hostID == "local")
    #expect(rows[0].usage.totalTokens == 490)
    #expect(rows[1].workerName == nil)
    #expect(rows[1].runIDs == [])
    #expect(rows[1].failedCycles == 1)
    #expect(rows[1].hostID == nil)
    #expect(rows[1].id == "cycle-2")
}

@Test func decodesSessionRowsFixture() throws {
    let rows = try JSONDecoder().decode([APISessionRow].self, from: Fixtures.data("session_rows.json"))
    #expect(rows.count == 1)
    #expect(rows[0].scope == "active")
    #expect(rows[0].sessionID == "sess-1")
    #expect(rows[0].agentName == "rupuso")
    #expect(rows[0].workspaceID == "ws-1")
    #expect(rows[0].activeRunID == "run-30")
}

@Test func decodesProjectRunsFixtureWithNoHostIDInjected() throws {
    // Project routes are local-only — unlike run_list_row.json, this
    // fixture carries no `host_id` key at all (not even `"local"`).
    let rows = try JSONDecoder().decode([APIRunListRow].self, from: Fixtures.data("project_runs.json"))
    #expect(rows.count == 1)
    #expect(rows[0].id == "run-01")
    #expect(rows[0].hostID == nil)
    #expect(rows[0].usage.totalTokens == 1200)
    #expect(rows[0].durationMS == 360_000)
}

@Test func decodesProjectSessionsFixtureWithScopeAndUsageNoHostID() throws {
    let rows = try JSONDecoder().decode([APISessionRow].self, from: Fixtures.data("project_sessions.json"))
    #expect(rows.count == 2)
    #expect(rows[0].scope == "active")
    #expect(rows[0].sessionID == "ses-01")
    #expect(rows[0].workspaceID == "ws-1")
    #expect(rows[0].usage?.priced == true)
    #expect(rows[0].hostID == nil)

    #expect(rows[1].scope == "archived")
    #expect(rows[1].activeRunID == nil)
    #expect(rows[1].usage?.priced == false)
    #expect(rows[1].usage?.costUSD == nil)
}

@Test func decodesEventRowsFixtureWithNonOptionalTsAndPos() throws {
    let rows = try JSONDecoder().decode([CPEventRow].self, from: Fixtures.data("event_rows.json"))
    #expect(rows.count == 3)
    #expect(rows[0].ts == 1_755_691_200_000)
    #expect(rows[0].pos == 0)
    #expect(rows[1].pos == 1)
    #expect(rows[2].pos == 2)
}

@Test func eventRowMissingTsThrowsOnDecode() throws {
    let json = Data(#"{"pos": 0, "run_id": "run-01", "started_at": "2026-08-20T12:00:00Z", "type": "run_started", "workflow_path": "wf/x.yaml"}"#.utf8)
    #expect(throws: (any Error).self) {
        _ = try JSONDecoder().decode(CPEventRow.self, from: json)
    }
}

@Test func eventRowMissingPosThrowsOnDecode() throws {
    let json = Data(#"{"ts": 1755691200000, "run_id": "run-01", "started_at": "2026-08-20T12:00:00Z", "type": "run_started", "workflow_path": "wf/x.yaml"}"#.utf8)
    #expect(throws: (any Error).self) {
        _ = try JSONDecoder().decode(CPEventRow.self, from: json)
    }
}
