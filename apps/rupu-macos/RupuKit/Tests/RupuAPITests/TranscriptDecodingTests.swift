import Testing
import Foundation
@testable import RupuAPI

@Test func decodesEveryTranscriptEventFixtureVariantNoneUnknown() throws {
    let events = try JSONDecoder().decode([TranscriptEvent].self, from: Fixtures.data("transcript_events.json"))
    #expect(events.count >= 15)
    #expect(!events.contains { if case .unknown = $0 { true } else { false } })

    guard case let .runStart(runID, workspaceID, agent, provider, model, startedAt, mode) = events[0] else {
        Issue.record("events[0] should be run_start"); return
    }
    #expect(runID == "run-01" && workspaceID == "ws-1" && agent == "rupuso")
    #expect(provider == "anthropic" && model == "claude-sonnet-4-6")
    #expect(startedAt == "2026-08-20T12:00:00Z" && mode == "ask")

    guard case let .assistantMessage(content, thinking) = events[3] else {
        Issue.record("events[3] should be assistant_message"); return
    }
    #expect(content == "Here is the plan")
    #expect(thinking == "reasoning trace")

    guard case let .toolResult(callID, output, error, durationMS, structured) = events[6] else {
        Issue.record("events[6] should be tool_result"); return
    }
    #expect(callID == "call-2" && output == "error output" && error == "boom" && durationMS == 5)
    #expect(structured != nil)

    guard case let .gateRequested(gateID, prompt, decision, decidedBy) = events[12] else {
        Issue.record("events[12] should be gate_requested"); return
    }
    #expect(gateID == "gate-1" && prompt == "Deploy to prod?")
    #expect(decision == "approved" && decidedBy == "matt")

    guard case .netFlow = events[23] else {
        Issue.record("events[23] should be net_flow"); return
    }

    // The fixture's trailing run is v2-only events (thinking / thinking_delta
    // / user_message / seed / compaction / notice), added after net_flow —
    // none of them decode as .unknown, which the top-of-test assertion above
    // already covers.
    let last = events[events.count - 1]
    guard case let .notice(kind, message) = last else {
        Issue.record("last event should be notice"); return
    }
    #expect(kind == "provider_retry" && message == "Retrying after a 529 overloaded response")
}

@Test func actionEmittedPreservesPayloadAsJSONValue() throws {
    let events = try JSONDecoder().decode([TranscriptEvent].self, from: Fixtures.data("transcript_events.json"))

    guard case let .actionEmitted(kind, payload, allowed, applied, reason) = events[10] else {
        Issue.record("events[10] should be action_emitted"); return
    }
    #expect(kind == "issues.create")
    #expect(allowed)
    #expect(applied)
    #expect(reason == "auto-approved")
    guard case let .object(payloadFields) = payload else {
        Issue.record("action_emitted payload should decode as a JSON object"); return
    }
    #expect(payloadFields["title"] == .string("x"))

    guard case let .actionEmitted(_, _, secondAllowed, secondApplied, secondReason) = events[11] else {
        Issue.record("events[11] should be action_emitted"); return
    }
    #expect(!secondAllowed)
    #expect(!secondApplied)
    #expect(secondReason == nil)
}

@Test func toolAuditDecodesAllFieldsIncludingBlockedTrue() throws {
    let events = try JSONDecoder().decode([TranscriptEvent].self, from: Fixtures.data("transcript_events.json"))

    guard case let .toolAudit(tool, declared, granted, blocked, restricted) = events[20] else {
        Issue.record("events[20] should be tool_audit"); return
    }
    #expect(tool == "issues.create")
    #expect(declared && granted && !blocked && restricted)

    guard case let .toolAudit(tool2, declared2, granted2, blocked2, restricted2) = events[21] else {
        Issue.record("events[21] should be tool_audit"); return
    }
    #expect(tool2 == "bash")
    #expect(!declared2 && !granted2 && blocked2 && restricted2)
}

@Test func unknownTranscriptEventTypeDecodesAsUnknownNotError() throws {
    let json = Data(#"{"type":"future","data":{}}"#.utf8)
    let event = try JSONDecoder().decode(TranscriptEvent.self, from: json)
    #expect(event == .unknown(type: "future"))
}

@Test func sseParserFrameDecodesAsTranscriptEventThroughGenericPath() throws {
    var parser = SSELineParser()
    let frameLine = #"data: {"type":"assistant_message","data":{"content":"hi"}}"#
    #expect(parser.feed(line: frameLine) == nil)
    let dispatched = parser.feed(line: "")
    let frame = try #require(dispatched)

    let data = try #require(frame.data.data(using: .utf8))
    let event = try JSONDecoder().decode(TranscriptEvent.self, from: data)
    #expect(event == .assistantMessage(content: "hi", thinking: nil))
}

@Test func transcriptPageDecodesPartialWhenPresentAndNilOtherwise() throws {
    let with = try JSONDecoder().decode(APITranscriptPage.self, from: Data(#"{"events":[],"summary":null,"partial":true}"#.utf8))
    #expect(with.partial == true)
    let without = try JSONDecoder().decode(APITranscriptPage.self, from: Data(#"{"events":[],"summary":null}"#.utf8))
    #expect(without.partial == nil)
}
