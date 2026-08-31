# Transcript Fidelity Plan 3 — macOS Display Parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rupu.app's transcript feed renders every recorded event — v2 thinking blocks in position, tool audits (the app currently has NO `actions:` audit surface), turn separators with tokens, file-edit diff bodies, prompt/seed/notice/compaction rows, unknown-event rows — with the row-building logic extracted pure and tested.

**Architecture:** `TranscriptEvent` (RupuAPI) gains decode cases for the v2 variants and real payloads for `tool_audit`/`action_emitted` (currently payload-less stubs, TranscriptModels.swift:59-61,191-196). `TranscriptFeed` (RupuRunDetail) gets its `rows` computation extracted into an internal `TranscriptFeedRows.build(events:)` (Swift-Testing-testable) that also does audit FIFO pairing, then new row views.

**Tech Stack:** Swift 6 / SwiftUI, Swift Testing (NOT XCTest), no third-party deps, XcodeGen-owned project (`make macos-gen`; never touch `.xcodeproj`).

**Spec:** `docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md`

## Global Constraints

- Branch `feat/transcript-fidelity-plan-3` off main AFTER Plan 1 merges (Plan 2 independent); PR-only.
- Thin app target: all changes live in RupuKit modules (`apps/rupu-macos/RupuKit/Sources/{RupuAPI,RupuRunDetail}`), never in `App/`.
- `assistant_delta` / `thinking_delta` stay unrendered (consolidated-event convention documented in TranscriptFeed.swift:35-43). `net_flow` stays out of the feed (the Netflow tab is its display).
- Legacy transcripts must render at least as well as today (v1 `assistant_message.thinking` keeps its disclosure).
- Verify with `make macos-test` then `make macos-build`. GUI validation rule: green builds ≠ rendering green — matt runs the app before this PR merges.

---

### Task 1: Decode the v2 wire — new cases + real audit/action payloads

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuAPI/TranscriptModels.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuAPITests/TranscriptModelsV2Tests.swift` (create; follow the existing RupuAPI test target layout)

**Interfaces:**
- Produces (Task 2 consumes these exact cases):

```swift
case thinking(text: String?, provider: String, model: String)
case thinkingDelta(content: String)
case userMessage(content: String)
case seed(messageCount: UInt32, sourceTranscript: String?)
case notice(kind: String, message: String)
case compaction(seq: UInt32, summarizedMessages: UInt32, backupPath: String?)
case toolAudit(tool: String, declared: Bool, granted: Bool, blocked: Bool, restricted: Bool)
case actionEmitted(kind: String, payload: JSONValue, allowed: Bool, applied: Bool, reason: String?)
```

plus `APITranscriptPage.unparsed: Int?` (Plan 2 Task 1's server field; decode with `decodeIfPresent` so older servers still parse).

(`thinking.raw` and `seed.messages` / `compaction.messages` are deliberately not decoded — spec §3: `raw` is persisted for replay, never displayed; the counts/text are the display surface.)

- [ ] **Step 1: Write the failing tests:**

```swift
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
```

- [ ] **Step 2: Run to verify failure** — `make macos-test` → FAIL (cases don't exist / equality fails).

- [ ] **Step 3: Implement in `TranscriptModels.swift`:** replace `case actionEmitted` / `case toolAudit` stubs with the payload-carrying cases above, add the new cases, extend `DataKeys` (`text`, `message`, `seq`, `messageCount = "message_count"`, `sourceTranscript = "source_transcript"`, `summarizedMessages = "summarized_messages"`, `backupPath = "backup_path"`, `declared`, `granted`, `blocked`, `restricted`, `allowed`, `applied`, `reason`, `payload`), and add the decode arms:

```swift
        case "thinking":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .thinking(
                text: try data.decodeIfPresent(String.self, forKey: .text),
                provider: try data.decode(String.self, forKey: .provider),
                model: try data.decode(String.self, forKey: .model)
            )
        case "thinking_delta":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .thinkingDelta(content: try data.decode(String.self, forKey: .content))
        case "user_message":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .userMessage(content: try data.decode(String.self, forKey: .content))
        case "seed":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .seed(
                messageCount: try data.decode(UInt32.self, forKey: .messageCount),
                sourceTranscript: try data.decodeIfPresent(String.self, forKey: .sourceTranscript)
            )
        case "notice":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .notice(
                kind: try data.decode(String.self, forKey: .kind),
                message: try data.decode(String.self, forKey: .message)
            )
        case "compaction":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .compaction(
                seq: try data.decode(UInt32.self, forKey: .seq),
                summarizedMessages: try data.decode(UInt32.self, forKey: .summarizedMessages),
                backupPath: try data.decodeIfPresent(String.self, forKey: .backupPath)
            )
        case "tool_audit":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .toolAudit(
                tool: try data.decode(String.self, forKey: .tool),
                declared: try data.decode(Bool.self, forKey: .declared),
                granted: try data.decode(Bool.self, forKey: .granted),
                blocked: try data.decode(Bool.self, forKey: .blocked),
                restricted: try data.decode(Bool.self, forKey: .restricted)
            )
        case "action_emitted":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .actionEmitted(
                kind: try data.decode(String.self, forKey: .kind),
                payload: try data.decode(JSONValue.self, forKey: .payload),
                allowed: try data.decode(Bool.self, forKey: .allowed),
                applied: try data.decode(Bool.self, forKey: .applied),
                reason: try data.decodeIfPresent(String.self, forKey: .reason)
            )
```

(`payload` needs a `DataKeys` entry too.) Update the type's doc comment — the "carry no associated payload this phase" paragraph is now false. Fix every exhaustive-switch compile error this creates across RupuKit (the compiler enumerates them; `RunDetailStore`/`SessionDetailStore` filters and `TranscriptFeed` are the likely sites — temporary `break`/`EmptyView()` arms are fine here ONLY until Task 2, same single-PR rule as Plan 1).

- [ ] **Step 4: Run** — `make macos-test` → PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(macos): decode transcript v2 — thinking/seed/user/notice/compaction, real tool_audit + action_emitted payloads"`

---

### Task 2: TranscriptFeed — pure row builder + rows for everything

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuRunDetail/TranscriptFeed.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuRunDetailTests/TranscriptFeedRowsTests.swift` (create alongside the existing RupuRunDetail tests)

**Interfaces:**
- Consumes: Task 1 cases.
- Produces: internal pure builder

```swift
enum TranscriptFeedRows {
    enum Row: Equatable {
        case single(TranscriptEvent)
        case toolPair(call: TranscriptEvent, result: TranscriptEvent?, audit: AuditInfo?)
        case standaloneAudit(tool: String, audit: AuditInfo, input: JSONValue?)
        case turnSeparator(turnIdx: UInt32, tokensIn: UInt64?, tokensOut: UInt64?)
    }
    struct AuditInfo: Equatable {
        let declared: Bool, granted: Bool, blocked: Bool, restricted: Bool
    }
    static func build(events: [TranscriptEvent]) -> [Row]
}
```

- [ ] **Step 1: Write the failing tests:**

```swift
import Testing
@testable import RupuRunDetail
import RupuAPI

@Suite struct TranscriptFeedRowsTests {
    @Test func thinkingUserSeedNoticeCompactionUnknownAllProduceRows() {
        let rows = TranscriptFeedRows.build(events: [
            .seed(messageCount: 4, sourceTranscript: "/w/turn1.jsonl"),
            .userMessage(content: "do it"),
            .thinking(text: "pick", provider: "anthropic", model: "m"),
            .notice(kind: "context_trim", message: "trimmed"),
            .compaction(seq: 1, summarizedMessages: 9, backupPath: nil),
            .unknown(type: "hologram_projection"),
        ])
        #expect(rows.count == 6, "no event may vanish: \(rows)")
    }

    @Test func auditPairsFIFOOntoItsCallAndActionNodeAuditStandsAlone() {
        let call: TranscriptEvent = .toolCall(callID: "c1", tool: "issues.create", input: .object([:]))
        let rows = TranscriptFeedRows.build(events: [
            call,
            .toolAudit(tool: "issues.create", declared: true, granted: true, blocked: false, restricted: true),
            .toolResult(callID: "c1", output: "ok", error: nil, durationMS: 3, structured: nil),
            // Action node: action_emitted then tool_audit, no call/result at all.
            .actionEmitted(kind: "repos.comment", payload: .object(["body": .string("hi")]), allowed: true, applied: true, reason: nil),
            .toolAudit(tool: "repos.comment", declared: true, granted: true, blocked: false, restricted: true),
        ])
        guard case .toolPair(_, let result, let audit) = rows[0] else {
            Issue.record("expected toolPair first, got \(rows)"); return
        }
        #expect(result != nil)
        #expect(audit == TranscriptFeedRows.AuditInfo(declared: true, granted: true, blocked: false, restricted: true))
        guard case .standaloneAudit(let tool, _, let input) = rows[1] else {
            Issue.record("expected standalone audit, got \(rows)"); return
        }
        #expect(tool == "repos.comment")
        #expect(input == .object(["body": .string("hi")]))
    }

    @Test func turnEndBecomesASeparatorAndDeltasStayHidden() {
        let rows = TranscriptFeedRows.build(events: [
            .turnStart(turnIdx: 0),
            .assistantDelta(content: "x"),
            .thinkingDelta(content: "y"),
            .assistantMessage(content: "done", thinking: nil),
            .turnEnd(turnIdx: 0, tokensIn: 11, tokensOut: 7),
        ])
        #expect(rows == [
            .single(.assistantMessage(content: "done", thinking: nil)),
            .turnSeparator(turnIdx: 0, tokensIn: 11, tokensOut: 7),
        ])
    }

    @Test func orphanToolResultStillRendersStandalone() {
        let rows = TranscriptFeedRows.build(events: [
            .toolResult(callID: "ghost", output: "late", error: nil, durationMS: 1, structured: nil),
        ])
        #expect(rows == [.single(.toolResult(callID: "ghost", output: "late", error: nil, durationMS: 1, structured: nil))])
    }
}
```

(If `turnEnd` decodes with more associated values after Plan 1's additive fields, extend the case + tests accordingly — the Swift decoder only needs `stop_reason`/`response_id` if the separator shows them; showing tokens only is the contract here, so the decoder may ignore the extra JSON keys.)

- [ ] **Step 2: Run to verify failure** — `make macos-test` → FAIL.

- [ ] **Step 3: Implement.**

3a. Extract the current `rows` computation into `TranscriptFeedRows.build(events:)` (same file, internal so tests reach it via `@testable`) and extend it: `call_id` pairing as today; plus a FIFO `[String: [Int]]` queue of audit-awaiting toolPair row indices keyed by tool name (mirror of the web's `pendingAuditsByTool`, transcriptView.ts:258-268); `.toolAudit` pops its tool's queue and sets the pair's `audit`, else emits `.standaloneAudit` consuming the stashed `.actionEmitted` payload for that kind (FIFO per kind, mirror of `pendingActionArgsByTool`); `.actionEmitted` itself emits no row (stash only — one merged entry per action call, same invariant as the web, ISSUES.md I-40); `.turnEnd` → `.turnSeparator`; `.turnStart`/`.runStart`/`.usage`/`.netFlow` → skipped (unchanged rationale, documented); `.assistantDelta`/`.thinkingDelta` → skipped (consolidated-event convention); everything else that previously fell through (`.thinking`, `.userMessage`, `.seed`, `.notice`, `.compaction`, `.unknown`) → `.single`.

3b. New row views, matching the file's existing row idiom (`.panelStyle(.innerCard)`, `Eyebrow`, `Color.rupu*`):

```swift
private struct ThinkingRow: View {
    let text: String?
    @State private var expanded = false
    var body: some View {
        Group {
            if let text, !text.isEmpty {
                DisclosureGroup(isExpanded: $expanded) {
                    Text(text)
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                        .padding(.top, 4)
                        .textSelection(.enabled)
                } label: {
                    Eyebrow("Thinking")
                }
            } else {
                Text("[redacted reasoning]")
                    .font(.noteText)
                    .italic()
                    .foregroundStyle(Color.rupuMute)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

private struct UserPromptRow: View {
    let content: String
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("Prompt")
            Text(content)
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.innerCard)
    }
}

private struct MetaLineRow: View { // seed / notice / compaction / unknown
    let label: String
    let detail: String
    var body: some View {
        HStack(spacing: 8) {
            Text(label).font(.metaText).foregroundStyle(Color.rupuDim)
            Text(detail)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

private struct TurnSeparatorRow: View {
    let turnIdx: UInt32
    let tokensIn: UInt64?
    let tokensOut: UInt64?
    var body: some View {
        HStack(spacing: 8) {
            Rectangle().fill(Color.rupuMute.opacity(0.25)).frame(height: 1)
            Text("turn \(turnIdx)\(tokenSuffix)")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .fixedSize()
            Rectangle().fill(Color.rupuMute.opacity(0.25)).frame(height: 1)
        }
    }
    private var tokenSuffix: String {
        guard let tokensIn, let tokensOut else { return "" }
        return " · \(tokensIn) in · \(tokensOut) out"
    }
}

private struct AuditBadge: View {
    let audit: TranscriptFeedRows.AuditInfo
    var body: some View {
        Text(audit.blocked ? "blocked" : audit.restricted ? (audit.declared ? "declared" : "undeclared") : "unrestricted")
            .font(.dataMono(9))
            .foregroundStyle(audit.blocked ? Color.status(.failed) : Color.rupuDim)
            .padding(.horizontal, 4).padding(.vertical, 1)
            .background((audit.blocked ? Color.status(.failed) : Color.rupuMute).opacity(0.12))
            .clipShape(RoundedRectangle(cornerRadius: 3))
    }
}
```

Wire them in `TranscriptRowView`: `.thinking` → `ThinkingRow`; `.userMessage` → `UserPromptRow`; `.seed(let n, let source)` → `MetaLineRow(label: "seed", detail: source.map { "\(n) prior messages seed this run · from \(($0 as NSString).lastPathComponent)" } ?? "\(n) prior messages seed this run")`; `.notice(let kind, let message)` → `MetaLineRow(label: kind, detail: message)`; `.compaction(let seq, let n, _)` → `MetaLineRow(label: "compaction", detail: "seq \(seq) · summarized \(n) messages")`; `.unknown(let type)` → `MetaLineRow(label: "event", detail: "unrecognized: \(type)")`; `.turnSeparator` → `TurnSeparatorRow`; `.standaloneAudit` → a `MetaLineRow`-style row with `AuditBadge` plus a pretty-printed `input` block when present; `ToolCallRow` gains an optional `audit: TranscriptFeedRows.AuditInfo?` shown as `AuditBadge` in its label HStack.

3c. `FileEditRow` gains the diff body (the field is already decoded and dropped today):

```swift
private struct FileEditRow: View {
    let path: String
    let kind: String
    let diff: String
    @State private var expanded = false
    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            Text(diff)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 4))
        } label: {
            HStack(spacing: 8) {
                Text(kind).font(.metaText).foregroundStyle(Color.rupuDim)
                Text(path)
                    .font(.dataMono(11.5))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}
```

3d. **Unparsed badge** — `TranscriptFeed.init` gains `unparsedCount: Int = 0`; when `> 0`, render at the top of the LazyVStack:

```swift
if unparsedCount > 0 {
    MetaLineRow(label: "warning", detail: "\(unparsedCount) transcript line\(unparsedCount == 1 ? "" : "s") could not be parsed (older app or newer rupu)")
}
```

Thread the value from wherever each caller holds the `APITranscriptPage` (e.g. `RunDetailStore`'s transcript state → `RunDetailTabs.swift:131`; callers without a page keep the default `0`).

3e. Update the type-level doc comment (the "every other variant … is skipped" paragraph) to the new truth: only deltas, `run_start`/`turn_start`/`usage`, and `net_flow` (Netflow tab) are deliberately unrendered; everything else has a row. Remove any Task 1 temporary arms (`grep -rn "until Task 2" apps/rupu-macos` must return nothing).

- [ ] **Step 4: Run** — `make macos-test && make macos-build` → PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(macos): transcript feed renders everything — thinking rows, audits with badge, turn separators, diff bodies, prompt/seed/notice/compaction/unknown"`

---

### Task 3: Fixtures, full verification, PR

**Files:**
- Modify (regenerated): `apps/rupu-macos/Fixtures/*.json` (only if Plan 1's regen didn't land or drifted)

- [ ] **Step 1:** `make macos-fixtures` then `git status` — commit any drift; `cargo test -p rupu-cp` must pass (fixture drift check).
- [ ] **Step 2:** `make macos-gen && make macos-test && make macos-build` → all green.
- [ ] **Step 3:** Commit + PR; hold merge for matt's GUI run (project rule 7: green ≠ rendering green). Ask him to check a run with gates + audits, a thinking-heavy run, and one legacy pre-v2 transcript.

```bash
git push origin feat/transcript-fidelity-plan-3
gh pr create --title "Transcript fidelity 3/3: macOS display parity" --body "Implements docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-3-macos-display.md (spec: docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md). Depends on Plan 1. Merge gated on matt's GUI validation run.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
