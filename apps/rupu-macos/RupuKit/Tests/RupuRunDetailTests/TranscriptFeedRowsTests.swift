import Testing
import RupuAPI
@testable import RupuRunDetail

/// Transcript-fidelity v2 (Plan 3, Task 2): pure tests for `buildFeedRows`'s
/// six new standalone row kinds (`thinking`/`userMessage`/`seed`/`notice`/
/// `compaction`/`unknownEvent`) plus the new `turnSeparator` row — the
/// extension of `TranscriptFeedModelTests`'s existing `buildFeedRows`
/// coverage (kept in a separate file per the task brief, since this is a
/// distinct feature arc from that file's turn/gate/run_complete merging
/// coverage). No SwiftUI/`@MainActor` needed — `buildFeedRows` is a plain
/// synchronous function over synthetic data, same as its sibling suite.
@Suite
struct TranscriptFeedRowsTests {

    // MARK: - no event may vanish: every v2 narrative kind produces a row

    @Test func thinkingUserSeedNoticeCompactionUnknownAllProduceRows() {
        let rows = buildFeedRows(events: [
            .seed(messageCount: 4, sourceTranscript: "/w/turn1.jsonl"),
            .userMessage(content: "do it"),
            .thinking(text: "pick", provider: "anthropic", model: "m"),
            .notice(kind: "context_trim", message: "trimmed"),
            .compaction(seq: 1, summarizedMessages: 9, backupPath: nil),
            .unknown(type: "hologram_projection"),
        ])
        #expect(rows.count == 6, "no event may vanish: \(rows)")
    }

    @Test func seedRowCarriesMessageCountAndSourceTranscript() {
        let rows = buildFeedRows(events: [.seed(messageCount: 4, sourceTranscript: "/w/turn1.jsonl")])
        guard case .seed(let messageCount, let sourceTranscript, _) = rows[0] else {
            Issue.record("expected .seed, got \(rows)"); return
        }
        #expect(messageCount == 4)
        #expect(sourceTranscript == "/w/turn1.jsonl")
    }

    @Test func userMessageRowCarriesItsContent() {
        let rows = buildFeedRows(events: [.userMessage(content: "do it")])
        guard case .userMessage(let content, _) = rows[0] else {
            Issue.record("expected .userMessage, got \(rows)"); return
        }
        #expect(content == "do it")
    }

    @Test func thinkingRowCarriesItsTextIncludingNilForRedacted() {
        let rows = buildFeedRows(events: [
            .thinking(text: "pick", provider: "anthropic", model: "m"),
            .thinking(text: nil, provider: "anthropic", model: "m"),
        ])
        guard case .thinking(let text0, _) = rows[0] else { Issue.record("expected .thinking, got \(rows)"); return }
        #expect(text0 == "pick")
        guard case .thinking(let text1, _) = rows[1] else { Issue.record("expected .thinking, got \(rows)"); return }
        #expect(text1 == nil)
    }

    @Test func noticeRowCarriesKindAndMessage() {
        let rows = buildFeedRows(events: [.notice(kind: "context_trim", message: "trimmed")])
        guard case .notice(let kind, let message, _) = rows[0] else {
            Issue.record("expected .notice, got \(rows)"); return
        }
        #expect(kind == "context_trim")
        #expect(message == "trimmed")
    }

    @Test func compactionRowCarriesSeqAndSummarizedMessages() {
        let rows = buildFeedRows(events: [.compaction(seq: 1, summarizedMessages: 9, backupPath: "/tmp/backup.jsonl")])
        guard case .compaction(let seq, let summarizedMessages, _) = rows[0] else {
            Issue.record("expected .compaction, got \(rows)"); return
        }
        #expect(seq == 1)
        #expect(summarizedMessages == 9)
    }

    @Test func unknownEventRowCarriesTheRawTypeString() {
        let rows = buildFeedRows(events: [.unknown(type: "hologram_projection")])
        guard case .unknownEvent(let type, _) = rows[0] else {
            Issue.record("expected .unknownEvent, got \(rows)"); return
        }
        #expect(type == "hologram_projection")
    }

    // MARK: - thinking_delta stays hidden (consolidated-event convention)

    @Test func thinkingDeltaNeverProducesARowEvenAloneInTheTranscript() {
        #expect(buildFeedRows(events: [.thinkingDelta(content: "partial reasoning...")]).isEmpty)
    }

    // MARK: - turn_end becomes a turnSeparator, positioned after its own turn

    @Test func turnEndBecomesASeparatorAndDeltasStayHidden() {
        let rows = buildFeedRows(events: [
            .turnStart(turnIdx: 0),
            .assistantDelta(content: "x"),
            .thinkingDelta(content: "y"),
            .assistantMessage(content: "done", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: 11, tokensOut: 7),
        ])
        #expect(rows.count == 2)
        guard case .turn(let turn) = rows[0] else { Issue.record("expected a .turn row, got \(rows)"); return }
        #expect(turn.assistantText == "done")
        guard case .turnSeparator(let turnIdx, let tokensIn, let tokensOut, _) = rows[1] else {
            Issue.record("expected a .turnSeparator row, got \(rows)"); return
        }
        #expect(turnIdx == 0)
        #expect(tokensIn == 11)
        #expect(tokensOut == 7)
    }

    // MARK: - a v2 narrative row never opens a gap turn of its own

    @Test func aStandaloneNoticeBeforeAnyTurnDoesNotOpenAGapTurn() {
        let rows = buildFeedRows(events: [
            .notice(kind: "provider_retry", message: "retrying"),
            .assistantMessage(content: "hi", thinking: nil),
        ])
        #expect(rows.count == 2)
        guard case .notice = rows[0] else { Issue.record("expected the notice first, got \(rows)"); return }
        guard case .turn(let turn) = rows[1] else { Issue.record("expected the fallback-mode turn second, got \(rows)"); return }
        #expect(turn.assistantText == "hi")
    }

    // MARK: - FeedRow.id stays unique across two otherwise-identical rows

    @Test func twoIdenticalNoticesGetDistinctRowIDsFromTheirPosition() {
        let rows = buildFeedRows(events: [
            .notice(kind: "provider_retry", message: "retrying"),
            .notice(kind: "provider_retry", message: "retrying"),
        ])
        #expect(rows.count == 2)
        #expect(rows[0].id != rows[1].id, "identical payloads must still get distinct, position-derived ids")
    }
}
