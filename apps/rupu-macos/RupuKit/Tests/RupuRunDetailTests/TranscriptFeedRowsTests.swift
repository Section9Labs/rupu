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

    // MARK: - Review fix (critical): a narrative event arriving while its
    // enclosing turn is still open must render BEFORE that turn's card, not
    // after. This is the NORMAL case, not an edge case — the runner's
    // emission-order contract writes `Thinking` right after `turn_start`
    // and before that turn's own `assistant_message`/`tool_call`. Before
    // this fix, `flushTurns(upTo: turnsOpenedSoFar)` treated the
    // still-open turn as flushable (`turnsOpenedSoFar` counts a turn from
    // its `turn_start`, not its `turn_end`), so the fully-built turn card
    // rendered ahead of the very reasoning that produced it.

    @Test func thinkingInsideAnOpenTurnRendersBeforeThatTurnsCardNotAfter() {
        let rows = buildFeedRows(events: [
            .turnStart(turnIdx: 0),
            .thinking(text: "pick", provider: "anthropic", model: "m"),
            .assistantMessage(content: "done", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
        ])
        #expect(rows.count == 3, "thinking + the turn card + its turnSeparator")
        guard case .thinking(let text, _) = rows[0] else {
            Issue.record("expected .thinking FIRST — reasoning in true position, ahead of the turn it produced, got \(rows)")
            return
        }
        #expect(text == "pick")
        guard case .turn(let turn) = rows[1] else { Issue.record("expected the turn card second, got \(rows)"); return }
        #expect(turn.assistantText == "done")
        guard case .turnSeparator = rows[2] else { Issue.record("expected the turnSeparator last, got \(rows)"); return }
    }

    @Test func noticeInsideAnOpenTurnRendersBeforeThatTurnsCardNotAfter() {
        let rows = buildFeedRows(events: [
            .turnStart(turnIdx: 0),
            .toolCall(callID: "c1", tool: "bash", input: .null),
            .notice(kind: "context_trim", message: "trimmed"),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 1, structured: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
        ])
        #expect(rows.count == 3, "the notice + the turn card + its turnSeparator")
        guard case .notice(let kind, let message, _) = rows[0] else {
            Issue.record("expected .notice FIRST — mid-turn, ahead of that turn's own card, got \(rows)")
            return
        }
        #expect(kind == "context_trim")
        #expect(message == "trimmed")
        guard case .turn(let turn) = rows[1] else { Issue.record("expected the turn card second, got \(rows)"); return }
        #expect(turn.tools.count == 1)
        guard case .turnSeparator = rows[2] else { Issue.record("expected the turnSeparator last, got \(rows)"); return }
    }

    /// A narrative event AFTER a turn has already closed (a real boundary,
    /// not a still-open one) must still render after that turn's card —
    /// the fix must not defer every narrative row unconditionally, only
    /// ones arriving while a turn is genuinely still open.
    @Test func noticeAfterATurnHasAlreadyClosedStillRendersAfterItsCard() {
        let rows = buildFeedRows(events: [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "done", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
            .notice(kind: "provider_retry", message: "retrying"),
        ])
        #expect(rows.count == 3, "the turn card + its turnSeparator + the notice")
        guard case .turn = rows[0] else { Issue.record("expected the turn card first, got \(rows)"); return }
        guard case .turnSeparator = rows[1] else { Issue.record("expected the turnSeparator second, got \(rows)"); return }
        guard case .notice = rows[2] else { Issue.record("expected the notice last — it arrived after the turn closed, got \(rows)"); return }
    }

    /// Two turns, each with its own mid-turn thinking — every narrative row
    /// stays paired with its OWN enclosing turn, never leaking into the
    /// wrong turn's position.
    @Test func thinkingInEachOfTwoTurnsStaysPairedWithItsOwnTurnNotTheOther() {
        let rows = buildFeedRows(events: [
            .turnStart(turnIdx: 0),
            .thinking(text: "first", provider: "anthropic", model: "m"),
            .assistantMessage(content: "one", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
            .turnStart(turnIdx: 1),
            .thinking(text: "second", provider: "anthropic", model: "m"),
            .assistantMessage(content: "two", thinking: nil),
            .turnEnd(turnIdx: 1, tokensIn: nil, tokensOut: nil),
        ])
        #expect(rows.count == 6)
        guard case .thinking(let firstText, _) = rows[0] else { Issue.record("row 0 should be the first thinking, got \(rows)"); return }
        #expect(firstText == "first")
        guard case .turn(let firstTurn) = rows[1] else { Issue.record("row 1 should be turn 0, got \(rows)"); return }
        #expect(firstTurn.assistantText == "one")
        guard case .turnSeparator = rows[2] else { Issue.record("row 2 should be turn 0's separator, got \(rows)"); return }
        guard case .thinking(let secondText, _) = rows[3] else { Issue.record("row 3 should be the second thinking, got \(rows)"); return }
        #expect(secondText == "second")
        guard case .turn(let secondTurn) = rows[4] else { Issue.record("row 4 should be turn 1, got \(rows)"); return }
        #expect(secondTurn.assistantText == "two")
        guard case .turnSeparator = rows[5] else { Issue.record("row 5 should be turn 1's separator, got \(rows)"); return }
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
