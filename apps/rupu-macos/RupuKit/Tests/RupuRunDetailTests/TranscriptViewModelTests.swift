import Testing
import RupuAPI
@testable import RupuRunDetail

/// Table-driven pure tests for `buildTranscriptViewModel`/`summarizeInput` —
/// a line-for-line port of `crates/rupu-cp/web/src/components/transcript/
/// transcriptView.ts`'s pairing/classification semantics (deviations noted
/// in `TranscriptViewModel.swift`'s file-header doc comment). No I/O, no
/// `@MainActor` — these are plain synchronous `@Test`s over synthetic event
/// arrays.
@Suite
struct TranscriptViewModelTests {

    // MARK: - call/result pairing by call_id

    @Test func toolResultPairsOntoItsToolCallByCallID() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "doing a thing", thinking: nil),
            .toolCall(callID: "c1", tool: "read_file", input: .object(["path": .string("a.rs")])),
            .toolResult(callID: "c1", output: "contents", error: nil, durationMS: 12, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 1)
        #expect(turns[0].tools.count == 1)
        let entry = turns[0].tools[0]
        #expect(entry.id == "c1")
        #expect(entry.tool == "read_file")
        #expect(entry.kind == .read)
        #expect(entry.output == "contents")
        #expect(entry.durationMS == 12)
        #expect(entry.errorText == nil)
    }

    // MARK: - orphan tool_result becomes a standalone entry (deviation from the web)

    @Test func orphanToolResultBecomesAStandaloneEntryRatherThanBeingDropped() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "hi", thinking: nil),
            .toolResult(callID: "ghost", output: "late output", error: nil, durationMS: 5, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 1)
        #expect(turns[0].tools.count == 1, "an unpaired tool_result must still surface as a row, not vanish")
        let entry = turns[0].tools[0]
        #expect(entry.id == "ghost")
        #expect(entry.tool == "")
        #expect(entry.kind == .generic)
        #expect(entry.output == "late output")
    }

    // MARK: - file_edit adjacency

    @Test func fileEditPairsByAdjacencyOntoThePrecedingUnpairedDiffCall() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "editing", thinking: nil),
            .toolCall(callID: "c1", tool: "write_file", input: .object(["path": .string("a.rs")])),
            .fileEdit(path: "a.rs", kind: "modified", diff: "-old\n+new"),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 3, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        let entry = turns[0].tools[0]
        #expect(entry.kind == .diff)
        #expect(entry.fileEdit?.path == "a.rs")
        #expect(entry.fileEdit?.kind == "modified")
        #expect(entry.fileEdit?.diff == "-old\n+new")
    }

    @Test func fileEditWithNoPendingDiffCallIsDroppedRatherThanMisattached() {
        // No preceding diff-kind call armed — the file_edit has nothing to
        // pair onto and must not create a phantom entry or crash.
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "nothing to edit yet", thinking: nil),
            .fileEdit(path: "a.rs", kind: "modified", diff: "-old\n+new"),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools.isEmpty)
    }

    // MARK: - command_run adjacency

    @Test func commandRunPairsByAdjacencyOntoThePrecedingUnpairedBashCall() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "running", thinking: nil),
            .toolCall(callID: "c1", tool: "bash", input: .object(["command": .string("ls")])),
            .commandRun(argv: ["/bin/sh", "-c", "ls"], cwd: "/tmp", exitCode: 0, stdoutBytes: 10, stderrBytes: 0),
            .toolResult(callID: "c1", output: "a.txt", error: nil, durationMS: 4, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        let entry = turns[0].tools[0]
        #expect(entry.kind == .terminal)
        #expect(entry.command?.argv == ["/bin/sh", "-c", "ls"])
        #expect(entry.command?.cwd == "/tmp")
        #expect(entry.command?.exitCode == 0)
    }

    // MARK: - tool_audit FIFO-per-name, NOT adjacency (out-of-order two-same-name-calls)

    /// Mirrors the exact on-disk order `transcriptView.ts`'s module doc
    /// describes for a turn with >1 `tool_use` of the SAME tool name:
    /// `call A, call B, audit A, result A, audit B, result B`. An
    /// adjacency/"last call wins" scheme would misattach A's audit to B;
    /// FIFO-by-name must get each audit onto the call it actually belongs
    /// to.
    @Test func toolAuditFIFOAttachesEachAuditToTheCorrectCallEvenOutOfResultOrder() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "two bash calls", thinking: nil),
            .toolCall(callID: "A", tool: "bash", input: .object(["command": .string("one")])),
            .toolCall(callID: "B", tool: "bash", input: .object(["command": .string("two")])),
            .toolAudit(tool: "bash", declared: true, granted: true, blocked: false, restricted: true),
            .toolResult(callID: "A", output: "1", error: nil, durationMS: 1, structured: nil),
            .toolAudit(tool: "bash", declared: false, granted: true, blocked: true, restricted: true),
            .toolResult(callID: "B", output: "2", error: nil, durationMS: 1, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools.count == 2)
        let entryA = turns[0].tools.first { $0.id == "A" }!
        let entryB = turns[0].tools.first { $0.id == "B" }!
        #expect(entryA.audit == ToolEntry.Audit(declared: true, granted: true, blocked: false, restricted: true))
        #expect(entryB.audit == ToolEntry.Audit(declared: false, granted: true, blocked: true, restricted: true))
    }

    // MARK: - standalone audit (no queued call of that name)

    @Test func toolAuditWithNoQueuedCallOfThatNameBecomesAStandaloneEntry() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "action node ran", thinking: nil),
            .toolAudit(tool: "notify_slack", declared: true, granted: true, blocked: false, restricted: true),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools.count == 1)
        let entry = turns[0].tools[0]
        #expect(entry.tool == "notify_slack")
        #expect(entry.kind == classifyForTest("notify_slack"))
        #expect(entry.audit == ToolEntry.Audit(declared: true, granted: true, blocked: false, restricted: true))
        #expect(entry.actionPayload == nil)
    }

    // MARK: - action_emitted merges onto the matching standalone audit entry

    @Test func actionEmittedPayloadMergesOntoTheMatchingStandaloneAuditEntry() {
        let payload = JSONValue.object(["channel": .string("#eng")])
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "action node ran", thinking: nil),
            .actionEmitted(kind: "notify_slack", payload: payload, allowed: true, applied: true, reason: nil),
            .toolAudit(tool: "notify_slack", declared: true, granted: true, blocked: false, restricted: true),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools.count == 1, "action_emitted must never produce its own row — only the merged tool_audit entry")
        let entry = turns[0].tools[0]
        #expect(entry.tool == "notify_slack")
        #expect(entry.actionPayload == payload)
        #expect(entry.audit != nil)
    }

    @Test func actionEmittedIsQueuedFIFOPerKindForTwoActionCallsOfTheSameTool() {
        let payloadA = JSONValue.object(["channel": .string("#a")])
        let payloadB = JSONValue.object(["channel": .string("#b")])
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "two action calls", thinking: nil),
            .actionEmitted(kind: "notify_slack", payload: payloadA, allowed: true, applied: true, reason: nil),
            .toolAudit(tool: "notify_slack", declared: true, granted: true, blocked: false, restricted: true),
            .actionEmitted(kind: "notify_slack", payload: payloadB, allowed: true, applied: true, reason: nil),
            .toolAudit(tool: "notify_slack", declared: true, granted: true, blocked: false, restricted: true),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools.count == 2)
        #expect(turns[0].tools[0].actionPayload == payloadA)
        #expect(turns[0].tools[1].actionPayload == payloadB)
    }

    // MARK: - ToolKind classification table (verified against transcriptView.ts:214-237)

    @Test(arguments: [
        ("report_finding", ToolKind.finding),
        ("read_file", ToolKind.read),
        ("grep", ToolKind.grep),
        ("glob", ToolKind.glob),
        ("write_file", ToolKind.diff),
        ("edit_file", ToolKind.diff),
        ("bash", ToolKind.terminal),
        ("dispatch_agent", ToolKind.subrun),
        ("dispatch_agents_parallel", ToolKind.subrun),
        ("ast_grep", ToolKind.astGrep),
        ("coverage_summary", ToolKind.coverage),
        ("coverage_detail", ToolKind.coverage),
        ("some_other_tool", ToolKind.generic),
    ])
    func classificationMatchesTheWebsTable(tool: String, expectedKind: ToolKind) {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "x", thinking: nil),
            .toolCall(callID: "c1", tool: tool, input: .null),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].tools[0].kind == expectedKind, "\(tool) must classify as \(expectedKind)")
    }

    // MARK: - synthetic turns fallback (no turn_start/turn_end at all)

    @Test func eachAssistantMessageOpensASyntheticTurnWhenNoTurnEventsArePresent() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "first", thinking: nil),
            .toolCall(callID: "c1", tool: "read_file", input: .null),
            .assistantMessage(content: "second", thinking: nil),
            .toolCall(callID: "c2", tool: "read_file", input: .null),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 2)
        #expect(turns[0].assistantText == "first")
        #expect(turns[0].tools.map(\.id) == ["c1"])
        #expect(turns[1].assistantText == "second")
        #expect(turns[1].tools.map(\.id) == ["c2"])
    }

    @Test func toolsBeforeTheFirstAssistantMessageLandInALeadingTurnWithNoAssistantText() {
        let events: [TranscriptEvent] = [
            .toolCall(callID: "c0", tool: "read_file", input: .null),
            .assistantMessage(content: "first", thinking: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 2)
        #expect(turns[0].assistantText == nil)
        #expect(turns[0].tools.map(\.id) == ["c0"])
        #expect(turns[1].assistantText == "first")
    }

    // MARK: - turn_start/turn_end define boundaries when present (the deliberate deviation)

    @Test func turnStartAndTurnEndDefineBoundariesInsteadOfPerAssistantMessageSplitting() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .assistantMessage(content: "thinking then acting", thinking: "scratch"),
            .toolCall(callID: "c1", tool: "read_file", input: .null),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 1, structured: nil),
            .turnEnd(turnIdx: 0, tokensIn: 100, tokensOut: 50),
            .turnStart(turnIdx: 1),
            .assistantMessage(content: "second turn", thinking: nil),
            .turnEnd(turnIdx: 1, tokensIn: 10, tokensOut: 5),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 2, "must fold on turn_start/turn_end, not split again per assistant_message inside a bounded turn")
        #expect(turns[0].id == 0)
        #expect(turns[0].assistantText == "thinking then acting")
        #expect(turns[0].thinking == "scratch")
        #expect(turns[0].tools.count == 1)
        #expect(turns[0].tokensIn == 100)
        #expect(turns[0].tokensOut == 50)
        #expect(turns[1].id == 1)
        #expect(turns[1].tokensIn == 10)
        #expect(turns[1].tokensOut == 5)
    }

    /// Review fix (Critical): two non-adjacent gaps — content arriving
    /// while no turn is open — must never collide on the same synthesized
    /// id. Exact repro from the review: `turnStart(0)…turnEnd(0)`, a
    /// standalone tool_audit (gap 1), `turnStart(1)…turnEnd(1)`, a second
    /// standalone tool_audit (gap 2) -> `[id:0, id:<gap1>, id:1,
    /// id:<gap2>]`, where a fixed `-1` sentinel previously produced
    /// `[id:0, id:-1, id:1, id:-1]` — a duplicate id in the `Identifiable`
    /// result array, which breaks SwiftUI `ForEach` identity/diffing.
    @Test func twoNonAdjacentGapTurnsGetDistinctIdsAndAllIdsInTheResultAreUnique() {
        let events: [TranscriptEvent] = [
            .turnStart(turnIdx: 0),
            .turnEnd(turnIdx: 0, tokensIn: 1, tokensOut: 1),
            .toolAudit(tool: "notify_slack", declared: true, granted: true, blocked: false, restricted: true),
            .turnStart(turnIdx: 1),
            .turnEnd(turnIdx: 1, tokensIn: 2, tokensOut: 2),
            .toolAudit(tool: "notify_email", declared: true, granted: true, blocked: false, restricted: true),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 4)
        #expect(turns[0].id == 0)
        #expect(turns[1].id < 0, "the first gap turn (after turn 0's turn_end) must be a negative synthesized id")
        #expect(turns[2].id == 1)
        #expect(turns[3].id < 0, "the second gap turn (after turn 1's turn_end) must also be a negative synthesized id")
        #expect(turns[1].id != turns[3].id, "the two gap turns must NOT collide on the same id")

        let allIDs = turns.map(\.id)
        #expect(Set(allIDs).count == allIDs.count, "every TurnVM in the result must have a unique id — duplicates break SwiftUI ForEach identity")
    }

    // MARK: - finding/error counts

    @Test func findingCountAndHasErrorAreDerivedFromTheTurnsTools() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "reviewing", thinking: nil),
            .toolCall(callID: "c1", tool: "report_finding", input: .object(["summary": .string("bug")])),
            .toolCall(callID: "c2", tool: "bash", input: .object(["command": .string("false")])),
            .toolResult(callID: "c2", output: "", error: "exit 1", durationMS: 2, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].findingCount == 1)
        #expect(turns[0].hasError == true)
    }

    @Test func findingCountIsZeroAndHasErrorIsFalseWhenNeitherApplies() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "all clear", thinking: nil),
            .toolCall(callID: "c1", tool: "read_file", input: .null),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 1, structured: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns[0].findingCount == 0)
        #expect(turns[0].hasError == false)
    }

    // MARK: - last-open flag

    @Test func onlyTheLastTurnIsOpenByDefault() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "first", thinking: nil),
            .assistantMessage(content: "second", thinking: nil),
            .assistantMessage(content: "third", thinking: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 3)
        #expect(turns[0].isOpenByDefault == false)
        #expect(turns[1].isOpenByDefault == false)
        #expect(turns[2].isOpenByDefault == true)
    }

    @Test func aSingleTurnIsOpenByDefault() {
        let turns = buildTranscriptViewModel(events: [.assistantMessage(content: "only", thinking: nil)])
        #expect(turns.count == 1)
        #expect(turns[0].isOpenByDefault == true)
    }

    // MARK: - excluded event kinds never open/close/populate a turn

    @Test func excludedEventKindsAreIgnoredEntirely() {
        let events: [TranscriptEvent] = [
            .runStart(runID: "r1", workspaceID: "w1", agent: "a", provider: "p", model: "m", startedAt: "t", mode: "auto"),
            .assistantMessage(content: "only turn", thinking: nil),
            .assistantDelta(content: "partial..."),
            .usage(provider: "p", model: "m", servedModel: nil, inputTokens: 1, outputTokens: 1, cachedTokens: 0),
            .netFlow,
            .gateRequested(gateID: "g1", prompt: "approve?", decision: nil, decidedBy: nil),
            .unknown(type: "some_future_event"),
            .runComplete(runID: "r1", status: "ok", totalTokens: 2, durationMS: 5, error: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 1, "none of these variants may open a new turn")
        #expect(turns[0].tools.isEmpty, "none of these variants may populate a turn's tools")
    }

    /// Transcript-fidelity v2 (Plan 3, Task 2): the six new event kinds are
    /// transcript-level narrative content, not turn-scoped tool activity —
    /// they render as their own standalone `FeedRow`s in `TranscriptFeed.
    /// swift` instead of ever opening/closing/populating a `TurnVM`.
    @Test func v2NarrativeEventKindsAreAlsoIgnoredEntirely() {
        let events: [TranscriptEvent] = [
            .assistantMessage(content: "only turn", thinking: nil),
            .thinking(text: "reasoning...", provider: "anthropic", model: "m"),
            .thinkingDelta(content: "partial reasoning..."),
            .userMessage(content: "do it"),
            .seed(messageCount: 3, sourceTranscript: nil),
            .notice(kind: "context_trim", message: "trimmed"),
            .compaction(seq: 1, summarizedMessages: 5, backupPath: nil),
        ]

        let turns = buildTranscriptViewModel(events: events)

        #expect(turns.count == 1, "none of these v2 variants may open a new turn")
        #expect(turns[0].tools.isEmpty, "none of these v2 variants may populate a turn's tools")
    }
}

/// Mirrors `classify(_:)` for the one test that needs to assert an exact
/// non-generic/non-diff/etc kind without repeating the whole table.
private func classifyForTest(_ tool: String) -> ToolKind {
    switch tool {
    case "report_finding": return .finding
    case "read_file": return .read
    case "grep": return .grep
    case "glob": return .glob
    case "write_file", "edit_file": return .diff
    case "bash": return .terminal
    case "dispatch_agent", "dispatch_agents_parallel": return .subrun
    case "ast_grep": return .astGrep
    default: return tool.hasPrefix("coverage_") ? .coverage : .generic
    }
}

// ---------------------------------------------------------------------------
// summarizeInput (ToolCard.tsx:50-121)
// ---------------------------------------------------------------------------

@Suite
struct SummarizeInputTests {
    @Test func readFormatsPathWithStartAndEndLine() {
        let input = JSONValue.object(["path": .string("src/a.rs"), "start_line": .number(10), "end_line": .number(20)])
        #expect(summarizeInput(tool: "read_file", kind: .read, input: input) == "src/a.rs:10-20")
    }

    @Test func readFormatsPathWithOnlyStartLine() {
        let input = JSONValue.object(["path": .string("src/a.rs"), "start_line": .number(10)])
        #expect(summarizeInput(tool: "read_file", kind: .read, input: input) == "src/a.rs:10")
    }

    @Test func readFormatsBarePathWhenNoLineRangeIsPresent() {
        let input = JSONValue.object(["path": .string("src/a.rs")])
        #expect(summarizeInput(tool: "read_file", kind: .read, input: input) == "src/a.rs")
    }

    @Test func readReturnsNilWhenPathIsMissing() {
        #expect(summarizeInput(tool: "read_file", kind: .read, input: .object([:])) == nil)
    }

    @Test func grepJoinsPatternAndPathWithTwoSpaces() {
        let input = JSONValue.object(["pattern": .string("TODO"), "path": .string("src/")])
        #expect(summarizeInput(tool: "grep", kind: .grep, input: input) == "TODO  src/")
    }

    @Test func grepFallsBackToPatternAloneWhenPathIsAbsent() {
        let input = JSONValue.object(["pattern": .string("TODO")])
        #expect(summarizeInput(tool: "grep", kind: .grep, input: input) == "TODO")
    }

    @Test func globReturnsPatternOverPath() {
        let input = JSONValue.object(["pattern": .string("*.rs"), "path": .string("src/")])
        #expect(summarizeInput(tool: "glob", kind: .glob, input: input) == "*.rs")
    }

    @Test func terminalTruncatesLongCommandsToFiftySevenCharsPlusEllipsis() {
        let longCommand = String(repeating: "x", count: 100)
        let input = JSONValue.object(["command": .string(longCommand)])
        let result = summarizeInput(tool: "bash", kind: .terminal, input: input)
        #expect(result?.count == 58) // 57 chars + the ellipsis glyph
        #expect(result?.hasSuffix("…") == true)
    }

    @Test func terminalReadsCmdKeyWhenCommandKeyIsAbsent() {
        let input = JSONValue.object(["cmd": .string("ls -la")])
        #expect(summarizeInput(tool: "bash", kind: .terminal, input: input) == "ls -la")
    }

    @Test func diffReturnsBarePath() {
        let input = JSONValue.object(["path": .string("src/a.rs")])
        #expect(summarizeInput(tool: "write_file", kind: .diff, input: input) == "src/a.rs")
    }

    @Test func astGrepJoinsPatternAndLangWithMiddleDot() {
        let input = JSONValue.object(["pattern": .string("fn $NAME()"), "lang": .string("rust")])
        #expect(summarizeInput(tool: "ast_grep", kind: .astGrep, input: input) == "fn $NAME() · rust")
    }

    @Test func genericFallsThroughPriorityOrderedKeys() {
        // `name` outranks `description` in the priority list (`path`,
        // `pattern`, `query`, `name`, `description`).
        let withBoth = JSONValue.object(["name": .string("preferred"), "description": .string("not this one")])
        #expect(summarizeInput(tool: "some_tool", kind: .generic, input: withBoth) == "preferred")

        let descriptionOnly = JSONValue.object(["description": .string("a coverage summary")])
        #expect(summarizeInput(tool: "coverage_summary", kind: .coverage, input: descriptionOnly) == "a coverage summary")
    }

    @Test func genericReturnsNilWhenNoPriorityKeyIsPresent() {
        let input = JSONValue.object(["unrelated": .string("value")])
        #expect(summarizeInput(tool: "some_tool", kind: .generic, input: input) == nil)
    }

    @Test func bareStringInputIsTruncatedLikeAnyOtherSummary() {
        #expect(summarizeInput(tool: "some_tool", kind: .generic, input: .string("short")) == "short")
    }

    @Test func nonObjectNonStringInputReturnsNil() {
        #expect(summarizeInput(tool: "some_tool", kind: .generic, input: .array([.number(1)])) == nil)
        #expect(summarizeInput(tool: "some_tool", kind: .generic, input: .null) == nil)
    }
}
