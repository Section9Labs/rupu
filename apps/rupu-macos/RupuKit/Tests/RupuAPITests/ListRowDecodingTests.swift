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

@Test func decodesSessionRowsFixture() throws {
    let rows = try JSONDecoder().decode([APISessionRow].self, from: Fixtures.data("session_rows.json"))
    #expect(rows.count == 1)
    #expect(rows[0].scope == "active")
    #expect(rows[0].sessionID == "sess-1")
    #expect(rows[0].agentName == "rupuso")
    #expect(rows[0].workspaceID == "ws-1")
    #expect(rows[0].activeRunID == "run-30")
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
