import Testing
import Foundation
@testable import RupuAPI

/// Compares two JSON payloads by parsed object equality rather than raw
/// bytes: `JSONEncoder`'s field order and the fixtures' pretty-printed
/// whitespace differ, so a byte-for-byte comparison would be brittle for no
/// benefit — the actual contract these tests protect is field names and
/// values, not formatting. `NSDictionary`/`NSArray` equality (via
/// `JSONSerialization`) checks exactly that, order-independently for object
/// keys.
private func assertJSONEqualsFixture<T: Encodable>(_ value: T, fixture: String) throws {
    let actualData = try JSONEncoder().encode(value)
    let actual = try JSONSerialization.jsonObject(with: actualData) as? NSDictionary
    let expected = try JSONSerialization.jsonObject(with: Fixtures.data("requests/\(fixture)")) as? NSDictionary
    #expect(actual == expected)
}

@Test func decodesAllThreeLaunchResponseVariants() throws {
    let responses = try JSONDecoder().decode([LaunchResponse].self, from: Fixtures.data("launch_responses.json"))
    #expect(responses.count == 3)

    #expect(responses[0].hostID == "local")
    #expect(responses[0].runID == "run-01")
    #expect(responses[0].sessionID == nil)
    #expect(responses[0].ok == nil)

    #expect(responses[1].hostID == "local")
    #expect(responses[1].runID == nil)
    #expect(responses[1].sessionID == "ses-01")
    #expect(responses[1].ok == nil)

    #expect(responses[2].hostID == "mini")
    #expect(responses[2].runID == nil)
    #expect(responses[2].sessionID == nil)
    #expect(responses[2].ok == true)
}

@Test func decodesRunControlResponseWithConfirmedCancelledStatus() throws {
    let response = try JSONDecoder().decode(RunControlResponse.self, from: Fixtures.data("run_control_response.json"))
    #expect(response.hostID == "local")
    #expect(response.confirmedStatus == "cancelled")
    #expect(response.run?.id == "run-01")
    #expect(response.run?.errorMessage == "Cancelled from control plane")
}

@Test func decodesRunControlResponseRemoteOkShapeWithoutRecord() throws {
    let json = Data(#"{"ok":true,"host_id":"mini"}"#.utf8)
    let response = try JSONDecoder().decode(RunControlResponse.self, from: json)
    #expect(response.ok == true)
    #expect(response.hostID == "mini")
    #expect(response.run == nil)
    #expect(response.confirmedStatus == nil)
}

@Test func decodesRunControlResponseArchiveShape() throws {
    let json = Data(#"{"ok":true,"id":"run-01","archived":true}"#.utf8)
    let response = try JSONDecoder().decode(RunControlResponse.self, from: json)
    #expect(response.ok == true)
    #expect(response.archived == true)
}

@Test func approveBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(ApproveBody(mode: "bypass"), fixture: "approve_body.json")
}

@Test func rejectBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(RejectBody(reason: "needs another look"), fixture: "reject_body.json")
}

@Test func rejectBodyWithNoReasonEncodesToEmptyObject() throws {
    let data = try JSONEncoder().encode(RejectBody())
    let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    #expect(object?.isEmpty == true)
}

@Test func cancelBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(CancelBody(reason: "operator cancelled"), fixture: "cancel_body.json")
}

@Test func agentLaunchBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(
        AgentLaunchBody(prompt: "investigate the failing test", mode: "bypass", target: "main", workingDir: "/tmp/project", host: "mini", scopeKind: "project", scopeID: "ws_a"),
        fixture: "agent_run_body.json"
    )
}

@Test func agentLaunchBodyEncodesToSessionStartFixtureWithLocalHostLiteral() throws {
    // `session_start_body.json` carries `"host":"local"` literally — the
    // body struct encodes whatever host value it is given verbatim; any
    // "local means omit" normalization is the caller's/CPClient's
    // responsibility for query-placed host params, not this struct's.
    try assertJSONEqualsFixture(
        AgentLaunchBody(prompt: "start reviewing", mode: "ask", target: "main", workingDir: "/tmp/project", host: "local"),
        fixture: "session_start_body.json"
    )
}

@Test func workflowLaunchBodyEncodesToFixtureOmittingUnsetOptionalFields() throws {
    try assertJSONEqualsFixture(
        WorkflowLaunchBody(inputs: ["branch": "main"], mode: "ask", host: "mini"),
        fixture: "workflow_launch_body.json"
    )
}

@Test func validateBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(
        ValidateBody(raw: "name: demo\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\n"),
        fixture: "validate_body.json"
    )
}

@Test func sendBodyEncodesToFixture() throws {
    try assertJSONEqualsFixture(SendBody(prompt: "hello"), fixture: "send_body.json")
}
