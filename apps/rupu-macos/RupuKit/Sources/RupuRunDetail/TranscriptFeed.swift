import SwiftUI
import Foundation
import RupuAPI
import RupuDesign

/// Renders a flat `[TranscriptEvent]` array as a scrolling feed. Deliberately
/// has **no run-specific coupling** — it doesn't know about `RunDetailStore`,
/// a run ID, or a transcript path; it just renders whatever events it's
/// handed, in order. `RunDetailScreen` (Task 8) feeds it `store.transcript`;
/// Task 9 (Session detail) is expected to reuse this exact view unchanged
/// for a session's own transcript.
///
/// Per the brief's render list: `assistant_message`/`assistant_delta` become
/// prose blocks; `tool_call` pairs with its matching `tool_result` (by
/// `call_id`) into one collapsed row; `gate_requested` gets a 2px
/// `Color.status(.waiting)` left edge; `command_run`/`file_edit` render as
/// one-line summaries; `run_complete` is a terminal row. Every other variant
/// (`turn_start`/`turn_end`/`usage`/`action_emitted`/`tool_audit`/`net_flow`/
/// `.unknown`) — and an orphaned `tool_result` with no matching `tool_call`
/// still renders standalone rather than vanishing silently) — is skipped:
/// nothing in this phase's design renders them yet.
public struct TranscriptFeed: View {
    private let events: [TranscriptEvent]

    public init(events: [TranscriptEvent]) {
        self.events = events
    }

    public var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                        TranscriptRowView(row: row)
                            .id(index)
                    }
                    if rows.isEmpty {
                        MicroLabel("NO TRANSCRIPT EVENTS YET")
                            .foregroundStyle(Color.rupuMute)
                            .padding(.top, 24)
                    }
                    // Anchor for auto-scroll-to-bottom as new tail events arrive.
                    Color.clear.frame(height: 1).id(Self.bottomAnchorID)
                }
                .padding(12)
            }
            .onChange(of: events.count) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) {
                    proxy.scrollTo(Self.bottomAnchorID, anchor: .bottom)
                }
            }
        }
    }

    private static let bottomAnchorID = "transcript-feed-bottom"

    /// Pairs each `tool_call` with the `tool_result` sharing its `call_id`
    /// (a linear scan — transcripts this phase are small enough that this
    /// never needs to be an index) and drops the standalone `tool_result`
    /// row once it's been folded into its pair. A `tool_result` whose
    /// `call_id` never appears on any `tool_call` in this event list (a
    /// truncated snapshot, a pre-Phase-2 transcript) still renders on its
    /// own rather than disappearing.
    private var rows: [TranscriptRow] {
        let callIDsWithCall: Set<String> = Set(events.compactMap {
            if case .toolCall(let callID, _, _) = $0 { return callID }
            return nil
        })
        var resultsByCallID: [String: TranscriptEvent] = [:]
        for event in events {
            if case .toolResult(let callID, _, _, _, _) = event {
                resultsByCallID[callID] = event
            }
        }

        var built: [TranscriptRow] = []
        for event in events {
            switch event {
            case .assistantMessage, .assistantDelta, .gateRequested, .commandRun, .fileEdit, .runComplete:
                built.append(.single(event))
            case .toolCall(let callID, _, _):
                built.append(.toolPair(call: event, result: resultsByCallID[callID]))
            case .toolResult(let callID, _, _, _, _):
                guard !callIDsWithCall.contains(callID) else { continue } // folded into its pair above
                built.append(.single(event))
            case .runStart, .turnStart, .turnEnd, .usage, .actionEmitted, .toolAudit, .netFlow, .unknown:
                continue
            }
        }
        return built
    }
}

/// One rendered feed row: either a single event, or a `tool_call` folded
/// together with its (possibly absent, if not finished yet) `tool_result`.
private enum TranscriptRow {
    case single(TranscriptEvent)
    case toolPair(call: TranscriptEvent, result: TranscriptEvent?)
}

private struct TranscriptRowView: View {
    let row: TranscriptRow

    var body: some View {
        switch row {
        case .single(let event):
            singleRow(event)
        case .toolPair(let call, let result):
            ToolCallRow(call: call, result: result)
        }
    }

    @ViewBuilder
    private func singleRow(_ event: TranscriptEvent) -> some View {
        switch event {
        case .assistantMessage(let content, let thinking):
            ProseRow(content: content, thinking: thinking)
        case .assistantDelta(let content):
            ProseRow(content: content, thinking: nil)
        case .gateRequested(let gateID, let prompt, let decision, let decidedBy):
            GateRequestedRow(gateID: gateID, prompt: prompt, decision: decision, decidedBy: decidedBy)
        case .commandRun(let argv, let cwd, let exitCode, let stdoutBytes, let stderrBytes):
            CommandRunRow(argv: argv, cwd: cwd, exitCode: exitCode, stdoutBytes: stdoutBytes, stderrBytes: stderrBytes)
        case .fileEdit(let path, let kind, let diff):
            FileEditRow(path: path, kind: kind, diff: diff)
        case .runComplete(let runID, let status, let totalTokens, let durationMS, let error):
            RunCompleteRow(runID: runID, status: status, totalTokens: totalTokens, durationMS: durationMS, error: error)
        case .toolCall, .toolResult, .runStart, .turnStart, .turnEnd, .usage, .actionEmitted, .toolAudit, .netFlow, .unknown:
            EmptyView()
        }
    }
}

// MARK: - Row kinds

private struct ProseRow: View {
    let content: String
    let thinking: String?

    @State private var thinkingExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(content)
                .font(.system(size: 12.5))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
            if let thinking, !thinking.isEmpty {
                DisclosureGroup(isExpanded: $thinkingExpanded) {
                    Text(thinking)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Color.rupuDim)
                        .padding(.top, 4)
                        .textSelection(.enabled)
                } label: {
                    MicroLabel("Thinking")
                        .foregroundStyle(Color.rupuMute)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

private struct ToolCallRow: View {
    let call: TranscriptEvent
    let result: TranscriptEvent?

    @State private var expanded = false

    private var callID: String {
        if case .toolCall(let id, _, _) = call { return id }
        return ""
    }
    private var tool: String {
        if case .toolCall(_, let tool, _) = call { return tool }
        return ""
    }
    private var input: JSONValue? {
        if case .toolCall(_, _, let input) = call { return input }
        return nil
    }
    private var resultOutput: String? {
        if case .toolResult(_, let output, _, _, _) = result { return output }
        return nil
    }
    private var resultError: String? {
        if case .toolResult(_, _, let error, _, _) = result { return error }
        return nil
    }
    private var resultDurationMS: UInt64? {
        if case .toolResult(_, _, _, let durationMS, _) = result { return durationMS }
        return nil
    }
    private var resultStructured: JSONValue? {
        if case .toolResult(_, _, _, _, let structured) = result { return structured }
        return nil
    }

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 8) {
                if let input {
                    jsonBlock(label: "Input", json: input)
                }
                if let resultError {
                    MicroLabel("Error")
                        .foregroundStyle(Color.status(.fail))
                    Text(resultError)
                        .font(.identifier)
                        .foregroundStyle(Color.status(.fail))
                        .textSelection(.enabled)
                } else if let resultOutput {
                    jsonBlock(label: "Output", text: resultOutput)
                }
                if let resultStructured {
                    jsonBlock(label: "Structured", json: resultStructured)
                }
                if result == nil {
                    MicroLabel("AWAITING RESULT")
                        .foregroundStyle(Color.rupuMute)
                }
            }
            .padding(.top, 6)
        } label: {
            HStack(spacing: 8) {
                Text(tool)
                    .font(.identifier)
                    .foregroundStyle(Color.rupuInk)
                Text("#\(callID)")
                    .font(.identifier)
                    .foregroundStyle(Color.rupuMute)
                if let resultDurationMS {
                    MicroLabel(Fmt.duration(ms: resultDurationMS))
                        .foregroundStyle(Color.rupuDim)
                }
                if resultError != nil {
                    Circle().fill(Color.status(.fail)).frame(width: 6, height: 6)
                }
                Spacer(minLength: 0)
            }
        }
        .padding(10)
        .panelStyle(.innerCard)
    }

    @ViewBuilder
    private func jsonBlock(label: String, json: JSONValue) -> some View {
        jsonBlock(label: label, text: prettyPrintedJSON(json))
    }

    @ViewBuilder
    private func jsonBlock(label: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            MicroLabel(label)
                .foregroundStyle(Color.rupuMute)
            Text(text)
                .font(.identifier)
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 4))
        }
    }
}

private struct GateRequestedRow: View {
    let gateID: String
    let prompt: String
    let decision: String?
    let decidedBy: String?

    var body: some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.status(.waiting))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    MicroLabel("Gate")
                        .foregroundStyle(Color.status(.waiting))
                    Text("#\(gateID)")
                        .font(.identifier)
                        .foregroundStyle(Color.rupuMute)
                }
                Text(prompt)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color.rupuInk)
                if let decision {
                    Text(decidedBy.map { "\(decision) by \($0)" } ?? decision)
                        .font(.system(size: 11))
                        .foregroundStyle(Color.rupuDim)
                }
            }
            .padding(.leading, 10)
            .padding(.vertical, 8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.status(.waiting).opacity(0.06))
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}

private struct CommandRunRow: View {
    let argv: [String]
    let cwd: String
    let exitCode: Int32
    let stdoutBytes: UInt64
    let stderrBytes: UInt64

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(Color.status(exitCode == 0 ? .done : .fail))
                .frame(width: 6, height: 6)
            Text("$ \(argv.joined(separator: " "))")
                .font(.identifier)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 0)
            MicroLabel("exit \(exitCode)")
                .foregroundStyle(exitCode == 0 ? Color.rupuDim : Color.status(.fail))
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

private struct FileEditRow: View {
    let path: String
    let kind: String
    let diff: String

    var body: some View {
        HStack(spacing: 8) {
            MicroLabel(kind)
                .foregroundStyle(Color.rupuDim)
            Text(path)
                .font(.identifier)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

private struct RunCompleteRow: View {
    let runID: String
    let status: String
    let totalTokens: UInt64
    let durationMS: UInt64
    let error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle()
                    .fill(Color.status(status == "ok" ? .done : .fail))
                    .frame(width: 6, height: 6)
                MicroLabel("Run complete — \(status)")
                    .foregroundStyle(Color.rupuInk)
                Spacer(minLength: 0)
                MicroLabel("\(Fmt.count(Int(totalTokens))) tok · \(Fmt.duration(ms: durationMS))")
                    .foregroundStyle(Color.rupuDim)
            }
            if let error {
                Text(error)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Color.status(.fail))
            }
        }
        .padding(10)
        .panelStyle(.innerCard)
    }
}

// MARK: - JSON pretty printing

private func prettyPrintedJSON(_ value: JSONValue) -> String {
    switch value {
    case .object, .array:
        let object = foundationValue(value)
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else { return "" }
        return text
    case .string(let s): return s
    case .number(let n): return n.truncatingRemainder(dividingBy: 1) == 0 ? String(Int64(n)) : String(n)
    case .bool(let b): return b ? "true" : "false"
    case .null: return "null"
    }
}

private func foundationValue(_ value: JSONValue) -> Any {
    switch value {
    case .string(let s): return s
    case .number(let n): return n
    case .bool(let b): return b
    case .object(let o): return o.mapValues(foundationValue)
    case .array(let a): return a.map(foundationValue)
    case .null: return NSNull()
    }
}
