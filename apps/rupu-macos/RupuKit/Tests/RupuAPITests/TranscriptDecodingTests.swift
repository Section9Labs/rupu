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

    let last = events[events.count - 1]
    guard case .netFlow = last else {
        Issue.record("last event should be net_flow"); return
    }
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
