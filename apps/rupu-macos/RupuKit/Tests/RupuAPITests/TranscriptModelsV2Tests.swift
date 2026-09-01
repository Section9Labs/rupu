import Testing
import Foundation
@testable import RupuAPI

@Suite struct TranscriptModelsV2Tests {
    private func decode(_ json: String) throws -> TranscriptEvent {
        try JSONDecoder().decode(TranscriptEvent.self, from: Data(json.utf8))
    }

    @Test func thinkingDecodesWithAndWithoutText() throws {
        let e = try decode(#"{"type":"thinking","data":{"text":"pick a tool","provider":"anthropic","model":"m","raw":{"signature":"sig"}}}"#)
        #expect(e == .thinking(text: "pick a tool", provider: "anthropic", model: "m"))
        let redacted = try decode(#"{"type":"thinking","data":{"provider":"anthropic","model":"m","raw":{}}}"#)
        #expect(redacted == .thinking(text: nil, provider: "anthropic", model: "m"))
    }

    @Test func newVariantsDecode() throws {
        #expect(try decode(#"{"type":"thinking_delta","data":{"content":"c"}}"#) == .thinkingDelta(content: "c"))
        #expect(try decode(#"{"type":"user_message","data":{"content":"do it"}}"#) == .userMessage(content: "do it"))
        #expect(try decode(#"{"type":"seed","data":{"message_count":4,"sha256":"aa","messages":[]}}"#) == .seed(messageCount: 4, sourceTranscript: nil))
        #expect(try decode(#"{"type":"seed","data":{"message_count":7,"sha256":"bb","source_transcript":"/w/turn1.jsonl"}}"#) == .seed(messageCount: 7, sourceTranscript: "/w/turn1.jsonl"))
        #expect(try decode(#"{"type":"notice","data":{"kind":"context_trim","message":"trimmed"}}"#) == .notice(kind: "context_trim", message: "trimmed"))
        #expect(try decode(#"{"type":"compaction","data":{"seq":1,"summarized_messages":9,"backup_path":"/b","messages":[]}}"#) == .compaction(seq: 1, summarizedMessages: 9, backupPath: "/b"))
    }

    @Test func toolAuditAndActionEmittedCarryTheirPayloadsNow() throws {
        let audit = try decode(#"{"type":"tool_audit","data":{"tool":"issues.create","declared":true,"granted":true,"blocked":false,"restricted":true}}"#)
        #expect(audit == .toolAudit(tool: "issues.create", declared: true, granted: true, blocked: false, restricted: true))
        let action = try decode(#"{"type":"action_emitted","data":{"kind":"issues.create","payload":{"title":"x"},"allowed":true,"applied":true}}"#)
        #expect(action == .actionEmitted(kind: "issues.create", payload: .object(["title": .string("x")]), allowed: true, applied: true, reason: nil))
    }

    @Test func unknownTagStillFallsThrough() throws {
        #expect(try decode(#"{"type":"hologram_projection","data":{}}"#) == .unknown(type: "hologram_projection"))
    }
}
