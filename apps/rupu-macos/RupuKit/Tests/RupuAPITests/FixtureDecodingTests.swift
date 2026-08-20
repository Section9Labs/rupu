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
