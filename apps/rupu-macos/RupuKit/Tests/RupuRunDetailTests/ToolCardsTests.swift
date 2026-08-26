import Testing
import RupuAPI
@testable import RupuRunDetail

/// `parseSubrunPayload` — pure JSON-field extraction off a `dispatch_agent`/
/// `dispatch_agents_parallel` result string, verified against the actual
/// Rust wire shape (`crates/rupu-tools/src/dispatch_agent.rs:156-168`,
/// `dispatch_agents_parallel.rs:235-247`) rather than the web's
/// `status`/`total_tokens` reads — see `SubrunPayload`'s doc comment for why.

@Test func parsesARealDispatchAgentResultBody() {
    let json = """
    {"ok": true, "agent": "reviewer", "output": "done", "findings": [], "tokens_used": 1234, "duration_ms": 500, "transcript_path": "/tmp/sub_TEST/transcript.jsonl", "sub_run_id": "sub_TEST"}
    """
    let payload = parseSubrunPayload(json)
    #expect(payload?.ok == true)
    #expect(payload?.tokensUsed == 1234)
    #expect(payload?.transcriptPath == "/tmp/sub_TEST/transcript.jsonl")
    #expect(payload?.subRunID == "sub_TEST")
    #expect(payload?.hasResolvableTarget == true)
}

@Test func parsesAFailedDispatchResult() {
    let json = #"{"ok": false, "agent": "x", "output": "boom", "findings": [], "tokens_used": 0, "duration_ms": 10, "transcript_path": "/tmp/x.jsonl", "sub_run_id": "sub_x"}"#
    let payload = parseSubrunPayload(json)
    #expect(payload?.ok == false)
    #expect(payload?.tokensUsed == 0)
}

@Test func returnsNilForNilOutput() {
    #expect(parseSubrunPayload(nil) == nil)
}

@Test func returnsNilForNonJSONOutput() {
    #expect(parseSubrunPayload("not json at all") == nil)
}

@Test func returnsNilForAJSONArrayRatherThanAnObject() {
    #expect(parseSubrunPayload("[1, 2, 3]") == nil)
}

@Test func returnsNilWhenNoneOfTheFourFieldsArePresent() {
    // Mirrors a `dispatch_agents_parallel` result: keyed by request id, so
    // none of `ok`/`tokens_used`/`transcript_path`/`sub_run_id` sit at the
    // JSON's own top level.
    let json = #"{"reviewer": {"ok": true, "agent": "reviewer"}}"#
    #expect(parseSubrunPayload(json) == nil)
}

@Test func hasResolvableTargetIsFalseWithoutATranscriptPathOrSubRunID() {
    let json = #"{"ok": true, "tokens_used": 5}"#
    let payload = parseSubrunPayload(json)
    #expect(payload != nil)
    #expect(payload?.hasResolvableTarget == false)
}

// ---------------------------------------------------------------------------
// `bodyValue(for:)` — `.coverage`/`.generic`'s `StructuredView` input
// precedence (fix round 2, finding 1: a standalone action-node entry's
// `actionPayload` was produced but never read, so it fell straight through
// to `.null` and rendered the literal text "null").
// ---------------------------------------------------------------------------

private func makeEntry(
    input: JSONValue = .null,
    structured: JSONValue? = nil,
    actionPayload: JSONValue? = nil
) -> ToolEntry {
    ToolEntry(id: "e1", tool: "some_action", kind: .generic, input: input, structured: structured, actionPayload: actionPayload)
}

@Test func bodyValueFallsBackToActionPayloadWhenStructuredIsAbsent() {
    // The standalone action-node shape this fix targets: no tool_call/
    // tool_result, so `input == .null` and `structured == nil`, but
    // `buildTranscriptViewModel` merged a real `with:` payload on.
    let payload: JSONValue = .object(["path": .string("a.rs")])
    let entry = makeEntry(input: .null, structured: nil, actionPayload: payload)

    #expect(bodyValue(for: entry) == payload, "actionPayload must win over the null input, not render as the literal 'null'")
}

@Test func bodyValuePrefersStructuredOverActionPayloadWhenBothArePresent() {
    let structured: JSONValue = .object(["result": .string("structured")])
    let actionPayload: JSONValue = .object(["result": .string("action")])
    let entry = makeEntry(input: .null, structured: structured, actionPayload: actionPayload)

    #expect(bodyValue(for: entry) == structured)
}

@Test func bodyValueFallsBackToInputWhenNeitherStructuredNorActionPayloadIsPresent() {
    let input: JSONValue = .object(["path": .string("b.rs")])
    let entry = makeEntry(input: input, structured: nil, actionPayload: nil)

    #expect(bodyValue(for: entry) == input)
}
