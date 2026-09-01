import Testing
import RupuAPI
@testable import RupuRunDetail

/// Pure tests for `TranscriptFeed.swift`'s two testable seams:
/// `flattenedTurnSnippet` (the collapsed header's ~100-char preview) and
/// `buildFeedRows` (turns re-merged against the standalone `gate_requested`/
/// `run_complete` events `buildTranscriptViewModel` excludes — see that
/// function's own doc comment for the exact positioning contract this
/// exercises). No SwiftUI/`@MainActor` needed — both are plain synchronous
/// functions over synthetic data.
@Suite
struct TranscriptFeedModelTests {

    // MARK: - flattenedTurnSnippet

    @Test func nilContentYieldsAnEmptySnippet() {
        #expect(flattenedTurnSnippet(nil) == "")
    }

    @Test func shortContentPassesThroughUnchanged() {
        #expect(flattenedTurnSnippet("Reading the config file.") == "Reading the config file.")
    }

    @Test func whitespaceRunsIncludingNewlinesCollapseToASingleSpaceAndTrim() {
        let content = "  Reading\nthe   config\tfile.  "
        #expect(flattenedTurnSnippet(content) == "Reading the config file.")
    }

    @Test func contentOver100CharsTruncatesToNinetyNineCharsPlusEllipsis() {
        let content = String(repeating: "a", count: 150)
        let snippet = flattenedTurnSnippet(content)
        #expect(snippet.count == 100, "99 kept chars + the ellipsis glyph = 100")
        #expect(snippet.hasSuffix("…"))
        #expect(snippet == String(repeating: "a", count: 99) + "…")
    }

    @Test func contentExactlyAtTheLimitIsNotTruncated() {
        let content = String(repeating: "a", count: 100)
        #expect(flattenedTurnSnippet(content) == content)
    }

    // MARK: - buildFeedRows: turns only (no gate/run_complete)

    @Test func turnOnlyEventsProduceOneTurnRowPerTurn() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "hi", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 2, "the turn row plus its own turnSeparator row")
        guard case .turn(let turn) = rows[0] else {
            Issue.record("expected a .turn row")
            return
        }
        #expect(turn.id == 0)
        guard case .turnSeparator(let turnIdx, _, _, _) = rows[1] else {
            Issue.record("expected a .turnSeparator row right after its turn")
            return
        }
        #expect(turnIdx == 0)
    }

    // MARK: - buildFeedRows: turn_end becomes its own turnSeparator row

    @Test func turnEndCarryingTokensSurfacesThemOnTheTurnSeparatorRow() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "hi", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: 11, tokensOut: 7),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 2)
        guard case .turnSeparator(let turnIdx, let tokensIn, let tokensOut, _) = rows[1] else {
            Issue.record("expected a .turnSeparator row, got \(rows)")
            return
        }
        #expect(turnIdx == 0)
        #expect(tokensIn == 11)
        #expect(tokensOut == 7)
    }

    // MARK: - buildFeedRows: run_complete is always last

    @Test func runCompleteIsAlwaysTheLastRow() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "hi", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
            .runComplete(runID: "r1", status: "ok", totalTokens: 10, durationMS: 100, error: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 3, "turn + its turnSeparator + run_complete")
        guard case .runComplete(let runID, _, _, _, _) = rows.last else {
            Issue.record("expected the last row to be .runComplete")
            return
        }
        #expect(runID == "r1")
    }

    // MARK: - buildFeedRows: gate_requested positioned between turns

    @Test func gateRequestedBetweenTwoTurnStartBoundedTurnsLandsBetweenThem() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "first", thinking: nil),
            .toolCall(callID: "c1", tool: "read_file", input: .null),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 1, structured: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
            .gateRequested(gateID: "g1", prompt: "continue?", decision: nil, decidedBy: nil),
            .turnStart(turnIdx: 1),
            .assistantMessage(content: "second", thinking: nil),
            .turnEnd(turnIdx: 1, tokensIn: nil, tokensOut: nil),
            .runComplete(runID: "r1", status: "ok", totalTokens: 10, durationMS: 100, error: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 6, "2 turns + their 2 turnSeparators + the gate + run_complete")

        guard case .turn(let first) = rows[0] else { Issue.record("row 0 should be a turn"); return }
        #expect(first.id == 0)

        guard case .turnSeparator(let firstIdx, _, _, _) = rows[1] else { Issue.record("row 1 should be the first turn's separator"); return }
        #expect(firstIdx == 0)

        guard case .gate(let gateID, let prompt, _, _) = rows[2] else { Issue.record("row 2 should be the gate"); return }
        #expect(gateID == "g1")
        #expect(prompt == "continue?")

        guard case .turn(let second) = rows[3] else { Issue.record("row 3 should be a turn"); return }
        #expect(second.id == 1)

        guard case .turnSeparator(let secondIdx, _, _, _) = rows[4] else { Issue.record("row 4 should be the second turn's separator"); return }
        #expect(secondIdx == 1)

        guard case .runComplete = rows[5] else { Issue.record("row 5 should be run_complete"); return }
    }

    @Test func gateRequestedBeforeAnyTurnStartsLandsFirst() {
        let events: [TranscriptEvent] = [
            .gateRequested(gateID: "g0", prompt: "proceed?", decision: "approved", decidedBy: "matt"),
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "go", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 3, "the leading gate + the turn + its turnSeparator")
        guard case .gate(let gateID, _, let decision, let decidedBy) = rows[0] else {
            Issue.record("row 0 should be the leading gate")
            return
        }
        #expect(gateID == "g0")
        #expect(decision == "approved")
        #expect(decidedBy == "matt")
        guard case .turn = rows[1] else { Issue.record("row 1 should be the turn"); return }
        guard case .turnSeparator = rows[2] else { Issue.record("row 2 should be the turn's separator"); return }
    }

    // MARK: - buildFeedRows: fallback mode (no turn_start/turn_end at all)

    @Test func fallbackModeGapTurnBeforeTheFirstAssistantMessageStillGetsItsOwnRowAheadOfAMidStreamGate() {
        // No turn_start/turn_end anywhere — `buildTranscriptViewModel` falls
        // back to "each assistant_message opens its own synthetic turn",
        // with any tool activity ahead of the first one landing in a gap
        // turn (negative id). A gate arriving between the gap turn's tool
        // activity and the first assistant_message should flush exactly
        // that one gap turn first.
        let events: [TranscriptEvent] = [
            .toolCall(callID: "c0", tool: "bash", input: .null),
            .toolResult(callID: "c0", output: "ok", error: nil, durationMS: 1, structured: nil),
            .gateRequested(gateID: "g1", prompt: "confirm?", decision: nil, decidedBy: nil),
            .assistantMessage(content: "first reply", thinking: nil),
            .toolCall(callID: "c1", tool: "grep", input: .null),
            .toolResult(callID: "c1", output: "found", error: nil, durationMS: 2, structured: nil),
            .runComplete(runID: "r1", status: "ok", totalTokens: 10, durationMS: 100, error: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.count == 4)

        guard case .turn(let gapTurn) = rows[0] else { Issue.record("row 0 should be the gap turn"); return }
        #expect(gapTurn.id < 0)
        #expect(gapTurn.tools.count == 1)
        #expect(gapTurn.tools[0].id == "c0")

        guard case .gate(let gateID, _, _, _) = rows[1] else { Issue.record("row 1 should be the gate"); return }
        #expect(gateID == "g1")

        guard case .turn(let secondTurn) = rows[2] else { Issue.record("row 2 should be the assistant turn"); return }
        #expect(secondTurn.assistantText == "first reply")
        #expect(secondTurn.tools.count == 1)
        #expect(secondTurn.tools[0].id == "c1")

        guard case .runComplete = rows[3] else { Issue.record("row 3 should be run_complete"); return }
    }

    // MARK: - buildFeedRows: no standalone events at all

    @Test func emptyEventsProduceEmptyRows() {
        #expect(buildFeedRows(events: []).isEmpty)
    }

    // MARK: - FeedRow.id (perf & interaction arc, Plan 5 Task 3: `ForEach`
    // now keys on this instead of positional `\.offset`)

    @Test func turnRowIDIsStableAcrossEqualTurnsIncludingNegativeGapTurnIDs() {
        let events: [TranscriptEvent] = [
            .toolCall(callID: "c0", tool: "bash", input: .null),
            .toolResult(callID: "c0", output: "ok", error: nil, durationMS: 1, structured: nil),
            .assistantMessage(content: "hi", thinking: nil),
        ]
        let firstPass = buildFeedRows(events: events)
        let secondPass = buildFeedRows(events: events)
        #expect(firstPass.map(\.id) == secondPass.map(\.id), "identical input must always produce identical row ids")

        guard case .turn(let gapTurn) = firstPass[0] else { Issue.record("expected the gap turn first"); return }
        #expect(gapTurn.id < 0, "the gap turn's id is negative — this is the case the brief calls out by name")
        #expect(firstPass[0].id == "turn:\(gapTurn.id)")
    }

    @Test func gateAndRunCompleteRowIDsAreKindPrefixedSoTheyCanNeverCollideWithATurn() {
        let events: [TranscriptEvent] = [
            .gateRequested(gateID: "42", prompt: "continue?", decision: nil, decidedBy: nil),
            .runComplete(runID: "42", status: "ok", totalTokens: 1, durationMS: 1, error: nil),
        ]
        let rows = buildFeedRows(events: events)
        #expect(rows.map(\.id) == ["gate:42", "runComplete:42"], "same underlying id string, but kind-prefixed so they never collide with each other or a turn")
    }

    // MARK: - TranscriptFeed's `computeRows` seam (perf & interaction arc,
    // Plan 5 Task 3): `rows`/`sawRunComplete` are computed exactly once at
    // init (the initial mount) via the injectable `computeRows` closure —
    // this is the one part of the "computed once per event, not once per
    // body pass" contract this test target can assert directly without a
    // SwiftUI view-hosting harness (which it has none of); the ongoing
    // "recomputed once per event, not once per body pass" half of that
    // contract rests on `.onChange(of: events.count)`'s own documented
    // "fires only when the tracked value changed" behavior.
    @Test @MainActor func initComputesRowsAndSawRunCompleteExactlyOnceViaTheInjectedSeam() {
        final class CallCounter {
            var count = 0
        }
        let counter = CallCounter()
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "hi", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: nil, tokensOut: nil),
            .runComplete(runID: "r1", status: "ok", totalTokens: 1, durationMS: 1, error: nil),
        ]
        let feed = TranscriptFeed(
            events: events, runID: nil, host: nil, sourcePreviewStore: nil,
            computeRows: { evts in
                counter.count += 1
                return buildFeedRows(events: evts)
            }
        )

        #expect(counter.count == 1, "init must call computeRows exactly once, not twice (the old computed-property bug)")
        #expect(feed.rows.count == 3, "one turn row + its turnSeparator + the run_complete row")
        #expect(feed.sawRunComplete == true)
    }
}
