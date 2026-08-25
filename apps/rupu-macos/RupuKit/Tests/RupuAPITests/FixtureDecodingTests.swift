import Testing
import Foundation
@testable import RupuAPI

@Test func decodesHostInfoFixture() throws {
    let info = try JSONDecoder().decode(HostInfo.self, from: Fixtures.data("host_info.json"))
    #expect(info.version == "0.71.0")
    #expect(info.capabilities.permissionModes == ["ask", "bypass", "readonly"])
}

@Test func decodesEveryEventFixtureVariant() throws {
    let events = try JSONDecoder().decode([CPEvent].self, from: Fixtures.data("events.json"))
    #expect(events.count >= 18)
    #expect(!events.contains { if case .unknown = $0 { true } else { false } })
    if case let .stepCompleted(runID, stepID, success, durationMS, host) = events[4] {
        #expect(runID == "run-01" && stepID == "plan" && success && durationMS == 4200 && host == "mini")
    } else { Issue.record("events[4] should be step_completed") }
}

@Test func unknownEventTypeDecodesAsUnknownNotError() throws {
    let json = Data(#"{"type":"future_thing","run_id":"r9"}"#.utf8)
    let ev = try JSONDecoder().decode(CPEvent.self, from: json)
    #expect(ev == .unknown(type: "future_thing", runID: "r9"))
}

@Test func decodesProjectsFixture() throws {
    let rows = try JSONDecoder().decode([APIProjectRow].self, from: Fixtures.data("projects.json"))
    #expect(!rows.isEmpty)
    #expect(rows[0].wsID == "ws-1")
    #expect(rows[0].name == "rupu")
    #expect(rows[0].runCount == 14)
    #expect(rows[0].lastRunAt == "2026-08-20T12:00:00Z")
    #expect(rows[0].path == "/Users/matt/Code/rupu")
    #expect(rows[0].repoHomeURL == "https://github.com/section9labs/rupu")
    #expect(rows[0].createdAt == "2026-08-01T09:00:00Z")
}

@Test func decodesDashboardFixture() throws {
    let dashboard = try JSONDecoder().decode(APIDashboardResponse.self, from: Fixtures.data("dashboard.json"))

    #expect(dashboard.hosts.count == 2)
    #expect(dashboard.hosts[0].hostID == "mini" && dashboard.hosts[0].state == "ok")
    #expect(dashboard.hosts[1].hostID == "kuki" && dashboard.hosts[1].state == "offline")
    #expect(dashboard.hosts[1].reason == "connection refused")

    // The poisoned field: findings_open is nil while findings_partial is true.
    #expect(dashboard.findingsOpen == nil)
    #expect(dashboard.findingsPartial == true)
    #expect(dashboard.cyclesPartial == false)
    #expect(dashboard.fleetPartial == false)

    #expect(dashboard.active.running == 3)
    #expect(dashboard.activeLongest?.runID == "run-01")
    #expect(dashboard.activeLongest?.ageMs == 120_000)

    #expect(dashboard.terminalBuckets.count == 2)
    #expect(dashboard.terminalBuckets[0].completed == 4)
    #expect(dashboard.throughputBuckets.count == 2)
    #expect(dashboard.throughputBuckets[0].cron == 3)

    #expect(dashboard.cycles.total == 10)
    #expect(dashboard.cycles.clean == 8)

    #expect(dashboard.fleet.issuesCapped == true)
    #expect(dashboard.fleet.issuesOpen == 120)
}
