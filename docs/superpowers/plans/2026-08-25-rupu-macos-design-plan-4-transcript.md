# rupu.app Design Alignment — Plan 4: Transcript & Rich Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the app's flat plain-text transcript with the web's rendering system — turn grouping, markdown prose, syntax-highlighted code, per-tool-kind cards with audit/status badges, a real diff view, finding cards, and the ast_grep rich body with inline source preview and CST tree.

**Architecture:** A pure `TranscriptViewModel` ports `transcriptView.ts`'s pairing semantics (result→call by `call_id`, adjacency for file_edit/command_run, FIFO-per-tool-name for tool_audit, action_emitted merge); SwiftUI card views port `components/transcript/*`; HighlighterSwift (the one sanctioned third-party package) supplies highlight.js parity; two new read endpoints (`source`, `ast`) join `CPClient` with fixture drift coverage.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing, HighlighterSwift (exact-pinned SPM).

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md` §5 · **Contract:** `docs/macOS_design/V2-CONTRACT.md` ("Transcript" section) · **Web sources of truth:** `crates/rupu-cp/web/src/components/transcript/{transcriptView.ts,Turn.tsx,ToolCard.tsx,Markdown.tsx,DiffView.tsx,TerminalBlock.tsx,FindingCard.tsx,SourcePreview.tsx,AstTree.tsx,StructuredView.tsx}`, `web/src/lib/transcript.ts`, `web/src/components/{CodeHighlight.tsx,codeHighlight.css}`.

## Global Constraints

- Plans 1–3 have landed (tokens/`StatusTone`/`Icon`/chrome kit; tabbed run-detail panel; graph selection driving `focusStep`). If not, STOP and say so.
- **Dependency carve-out (matt, 2026-08-25):** `https://github.com/smittytone/HighlighterSwift` is the ONE permitted third-party Swift package, pinned `.exact` in `RupuKit/Package.swift`. No other package may ride in with it.
- Typography discipline: prose sans (`uiText` 12pt base), code/ids/diff mono (`dataMono`). Null discipline `—` never 0.
- Highlight language set: mirror the web — full set for markdown fences (highlight.js auto/name lookup), and previews highlight only when `language ∈ {rust, python, typescript, javascript, go, json}` (`CodeHighlight.tsx` `SOURCE_PREVIEW_LANGUAGES`); `ini` doubles for `toml`.
- The app's transcript auto-scroll live tail is KEPT (the web lacks it; deliberate superiority — spec §5).
- `make macos-test` + `make macos-build` green after every task; `make macos-fixtures` + `cargo test -p rupu-cp` whenever a serde-mirrored model changes. Never bare `git stash pop`.

## File structure

```
RupuKit/Package.swift                                  # + HighlighterSwift (exact pin)
RupuKit/Sources/RupuRunDetail/Rendering/CodeBlock.swift        # NEW: highlighted code view
RupuKit/Sources/RupuAPI/TranscriptModels.swift         # toolAudit/actionEmitted payloads
RupuKit/Sources/RupuAPI/SourceAstModels.swift          # NEW: APISourceSlice, APIAstNode
RupuKit/Sources/RupuAPI/CPClient.swift                 # + readSource / readAst
RupuKit/Sources/RupuRunDetail/TranscriptViewModel.swift        # NEW: pure pairing model
RupuKit/Sources/RupuRunDetail/Rendering/MarkdownView.swift     # NEW
RupuKit/Sources/RupuRunDetail/Rendering/ToolCards.swift        # NEW: header + simple bodies
RupuKit/Sources/RupuRunDetail/Rendering/DiffView.swift         # NEW
RupuKit/Sources/RupuRunDetail/Rendering/AstGrepBody.swift      # NEW (+ SourcePreview, AstTree)
RupuKit/Sources/RupuRunDetail/Rendering/FindingCard.swift      # NEW
RupuKit/Sources/RupuRunDetail/TranscriptFeed.swift     # Rewrite: turns + card dispatch
CLAUDE.md                                              # rule-4 carve-out + module notes
```

---

### Task 1: HighlighterSwift + CodeBlock

**Files:**
- Modify: `RupuKit/Package.swift` (add dependency, exact pin to the latest tagged release at execution time; record the version in the commit message)
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/CodeBlock.swift`
- Modify: `CLAUDE.md` — macOS rule 4 becomes: "**No third-party Swift dependencies, except HighlighterSwift (syntax highlighting; exact-pinned; approved 2026-08-25).** Swift Testing (not XCTest) for all tests."
- Test: `RupuKit/Tests/RupuRunDetailTests/CodeBlockTests.swift`

**Interfaces:**
- Produces:
  ```swift
  public enum CodeHighlighter {           // wraps HighlighterSwift; one shared instance
      static func highlight(_ code: String, language: String?, dark: Bool) -> AttributedString
      // language nil → auto-detect; theme "atom-one-light"/"atom-one-dark" (closest pair
      // to web codeHighlight.css); background stripped (CodeBlock paints rupuSurface);
      // "toml" mapped to "ini" before the call. Highlighter init failure → plain mono fallback.
  }
  public struct CodeBlock: View { public init(_ code: String, language: String?) }
  // dataMono(11.5), rupuSurface fill, radius 6, horizontal scroll, text selection enabled;
  // resolves dark via colorScheme and re-highlights on theme change.
  ```
- [ ] **Step 1: Failing tests** — `highlight("let x = 1", language: "swift", dark: false)` returns an AttributedString with >1 distinct foreground color; unknown language falls back to unstyled (1 color) without crashing; `toml` request does not throw.
- [ ] **Step 2:** Add the pinned dependency, implement, GREEN (`make macos-test`; `make macos-gen` if the app target needs the transitive product linked).
- [ ] **Step 3:** CLAUDE.md rule edit. **Commit** — `feat(macos-transcript): HighlighterSwift (exact-pinned) + CodeBlock view; CLAUDE.md dependency carve-out`

### Task 2: API surface — audit payloads + source/AST endpoints

**Files:**
- Modify: `RupuKit/Sources/RupuAPI/TranscriptModels.swift`
- Create: `RupuKit/Sources/RupuAPI/SourceAstModels.swift`
- Modify: `RupuKit/Sources/RupuAPI/CPClient.swift`
- Modify: fixture rig — `crates/rupu-cp` fixture emitter + `apps/rupu-macos/Fixtures/` (run `make macos-fixtures`)
- Test: `RupuKit/Tests/RupuAPITests/` (extend transcript decode suite; new SourceAstDecodeTests against fixtures)

**Interfaces:**
- `TranscriptEvent` payload upgrades (replacing the empty cases at `TranscriptModels.swift:59-61`):
  ```swift
  case toolAudit(tool: String, declared: Bool, granted: Bool, blocked: Bool, restricted: Bool)
  case actionEmitted(data: JSONValue)   // opaque on the web too (lib/transcript.ts:21)
  // netFlow stays payload-less (nothing renders it).
  ```
- New models (serde mirrors of the Rust responses — `SourceSlice` / `AstResponse` in `web/src/lib/api.ts:1656-1692` name the fields):
  ```swift
  public struct APISourceLine: Decodable, Sendable { public let n: Int; public let text: String }
  public struct APISourceSlice: Decodable, Sendable {
      public let available: Bool; public let path: String?; public let language: String?
      public let startLine: Int?; public let endLine: Int?; public let targetLine: Int?
      public let totalLines: Int?; public let lines: [APISourceLine]?; public let reason: String?
  }   // camelCase keys on the wire, same as the web
  public struct APIAstNode: Decodable, Sendable {
      public let kind: String; public let named: Bool; public let field: String?
      public let startLine: Int; public let startCol: Int; public let endLine: Int; public let endCol: Int
      public let matched: Bool; public let children: [APIAstNode]
  }
  public struct APIAstResponse: Decodable, Sendable {
      public let available: Bool; public let language: String?; public let root: APIAstNode?
      public let truncated: Bool?; public let reason: String?
  }
  ```
- `CPClient`:
  ```swift
  public func readSource(runID: String, path: String, line: Int, context: Int? = nil, host: String? = nil) async throws -> APISourceSlice
  // GET /api/runs/:id/source?path=&line=[&context=][&host=]
  public func readAst(runID: String, path: String, line: Int, col: Int, host: String? = nil) async throws -> APIAstResponse
  // GET /api/runs/:id/ast?path=&line=&col=[&host=]
  ```
- [ ] **Step 1:** Extend the rupu-cp fixture emitter with a source-slice and an ast-response sample (plus a transcript fixture containing `tool_audit` + `action_emitted` lines); `make macos-fixtures`; `cargo test -p rupu-cp` green (drift gate).
- [ ] **Step 2: Failing Swift decode tests** against the new fixtures (toolAudit field round-trip; actionEmitted payload preserved; source slice with `available:false` + `reason`; ast tree with a `matched` grandchild). RED → implement → GREEN.
- [ ] **Step 3: Commit** — `feat(macos-api): tool_audit/action_emitted payloads + readSource/readAst with fixture drift coverage`

### Task 3: TranscriptViewModel (pure pairing port)

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/TranscriptViewModel.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/TranscriptViewModelTests.swift`

**Interfaces:**
- Produces (semantics are a line-for-line port of `transcriptView.ts` — read it first):
  ```swift
  public enum ToolKind: String, Sendable { case finding, read, grep, glob, diff, terminal, subrun, coverage, astGrep, generic }
  public struct ToolEntry: Identifiable, Equatable, Sendable {
      public let id: String                 // call_id (or synthesized for standalone audits)
      public let tool: String
      public let kind: ToolKind
      public let input: JSONValue
      public let output: String?; public let errorText: String?; public let durationMS: UInt64?
      public let structured: JSONValue?
      public let fileEdit: (path: String, kind: String, diff: String)?   // adjacency-paired
      public let command: (argv: [String], cwd: String, exitCode: Int32)? // adjacency-paired
      public let audit: (declared: Bool, granted: Bool, blocked: Bool, restricted: Bool)?
      public let actionPayload: JSONValue?  // action_emitted merged onto its audit
  }
  public struct TurnVM: Identifiable, Equatable, Sendable {
      public let id: Int                    // turn index (synthesized when no turn events)
      public let assistantText: String?; public let thinking: String?
      public let tools: [ToolEntry]
      public let findingCount: Int; public let hasError: Bool; public let isOpenByDefault: Bool
      public let tokensIn: UInt64?; public let tokensOut: UInt64?
  }
  public func buildTranscriptViewModel(events: [TranscriptEvent]) -> [TurnVM]
  ```
- Pairing rules (from `transcriptView.ts`, cited lines): result→call by `call_id`; `file_edit` by adjacency onto the nearest preceding unpaired write/edit call; `command_run` by adjacency onto bash; `tool_audit` FIFO queue keyed on tool NAME (L258-268 — deliberately not adjacency), standalone entry when no call queued; `action_emitted` stashed then merged onto the matching audit entry (L437-458). ToolKind classification mirrors L214-237 (by tool name: read/grep/glob/bash→terminal/write+edit→diff/run_agent→subrun/emit_finding→finding/coverage/ast_grep; else generic). Turn boundaries from `turn_start`/`turn_end`; when a transcript has none, each `assistant_message` opens a synthetic turn. `assistant_delta`, `net_flow`, `usage`, `run_start` are excluded from turns (run_start/run_complete feed the header/footer as today). Last turn `isOpenByDefault`.
- Input summary helper: `public func summarizeInput(tool: String, kind: ToolKind, input: JSONValue) -> String?` — per-kind per `ToolCard.tsx:50-121` (`path:start-end`, `pattern  path`, first ~60 chars of command, `pattern · lang`).

- [ ] **Step 1: Failing tests** — table-driven over synthetic event arrays: call/result pairing; orphan result; file_edit adjacency; command adjacency; audit FIFO with two same-name calls out of order; standalone audit; action merge; classification table; synthetic turns fallback; finding/tool counts; last-open flag.
- [ ] **Step 2:** RED → implement → GREEN.
- [ ] **Step 3: Commit** — `feat(macos-transcript): pure TranscriptViewModel — pairing, audits, ToolKind, turn folding`

### Task 4: MarkdownView

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/MarkdownView.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/MarkdownBlocksTests.swift`

**Interfaces:**
- Produces:
  ```swift
  public enum MarkdownBlock: Equatable, Sendable {
      case heading(level: Int, text: String)
      case paragraph(text: String)          // inline md parsed by AttributedString at render
      case listItem(indent: Int, ordered: Bool, marker: String, text: String)
      case quote(text: String)
      case fence(language: String?, code: String)
      case rule
      case table(raw: String)               // honest fallback: mono block (spec §5)
  }
  public func parseMarkdownBlocks(_ source: String) -> [MarkdownBlock]
  public struct MarkdownView: View { public init(_ source: String) }
  ```
- Render: headings `leadText`/semibold scaled by level; paragraphs/list items via `AttributedString(markdown:options:.init(interpretedSyntax:.inlineOnlyPreservingWhitespace))` (bold/italic/links/inline code — inline code in `dataMono` on `rupuSurface`); quote = 2px `rupuBorder` left bar + `rupuDim`; `fence` → Task 1's `CodeBlock`; `table` → `dataMono(11)` block. Text selection enabled throughout.

- [ ] **Step 1: Failing parser tests** — fence with language, fence unterminated (consumes to end), nested list markers, quote runs merging, table detection (`|`-led lines), heading levels, plain paragraphs joining soft-wrapped lines.
- [ ] **Step 2:** RED → implement → GREEN. **Commit** — `feat(macos-transcript): markdown renderer — block splitter + inline attributes + highlighted fences`

### Task 5: Tool cards — header, badges, simple bodies

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/ToolCards.swift`
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/DiffView.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/DiffParseTests.swift`

**Interfaces:**
- `ToolCardView(entry: ToolEntry, runID: String?, host: String?)` — collapsed `DisclosureGroup` (collapsed by default, matching today's behavior): header = `Icon(.settings, size: 11)`-style gear + tool name `dataMono(11.5)` + `summarizeInput` (`metaText`, `rupuDim`) + AuditBadge + StatusBadge.
  - `AuditBadge(audit:)` per `ToolCard.tsx:315-344`: `blocked` (err Badge) / `not granted` (warn) / `audited` (mute); `help()` carries the explanation.
  - `StatusBadge(entry:)`: error (err) / `Fmt.duration(ms:)` (mute) / `ok` (ok tone).
- Bodies by `entry.kind`:
  - `.diff` → `DiffView(diff: entry.fileEdit?.diff ?? entry.output ?? "")`:
    ```swift
    public enum DiffLine: Equatable, Sendable { case hunk(String), add(String), del(String), ctx(String), meta(String) }
    public func parseUnifiedDiff(_ text: String) -> [DiffLine]   // ---/+++/diff --git/index guards per DiffView.tsx:42-70
    ```
    render `dataMono(11.5)`: add `rupuOkBg`/`rupuOk` · del `rupuErrBg`/`rupuErr` · hunk `rupuMute` · meta `rupuDim`.
  - `.terminal` → prompt line (`$` in `rupuOk` + argv joined), output `<pre>`-style mono scroll capped ~24 lines, exit Badge (ok 0 / err non-zero) + cwd `rupuMute`.
  - `.read` → mono block capped; `.grep` → match-count line + mono lines; `.glob` → mono path list.
  - `.subrun` → status + token Badge chips + "View sub-run transcript →" ghost button (routes via the existing session/run navigation when `runID` resolvable, else hidden).
  - `.coverage`/`.generic` → `StructuredView(value: entry.structured ?? entry.input)`:
    ```swift
    public struct StructuredView: View { public init(value: JSONValue, depth: Int = 0) }
    // objects → key rows; homogeneous object arrays → compact table; scalar arrays → chip
    // list; bools → tone pill; long strings → mono block; depth cap 4 (StructuredView.tsx)
    ```
  - `.finding` and `.astGrep` bodies land in Tasks 6–7; until then they fall through to `StructuredView` (compiles, honest).
  - Error output → err-tinted block alongside any body.
- [ ] **Step 1: Failing diff-parse tests** — hunk/add/del/ctx classification; `---`/`+++`/`diff --git` as meta not del/add; empty diff.
- [ ] **Step 2:** RED → implement all views → GREEN; `make macos-build`. **Commit** — `feat(macos-transcript): tool cards — audit/status badges, diff/terminal/read/grep/glob/structured bodies`

### Task 6: ast_grep body + SourcePreview + AstTree

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/AstGrepBody.swift` (includes `SourcePreviewView`, `AstTreeView`)
- Test: `RupuKit/Tests/RupuRunDetailTests/AstGrepModelTests.swift`

**Interfaces:**
- Decode model (from `entry.structured`; authoritative shape = the ast_grep structured serde in `crates/rupu-tools` — the CP rich-rendering arc — cross-checked against `ToolCard.tsx:806-874`; lock it with a fixture extracted in Task 2 if the sample there lacks ast_grep, extend that fixture now):
  ```swift
  public struct AstGrepMetaVar: Equatable, Sendable { public let name: String; public let text: String; public let start: Int; public let end: Int } // codepoint offsets
  public struct AstGrepMatch: Equatable, Sendable { public let path: String; public let line: Int; public let col: Int; public let text: String; public let metaVars: [AstGrepMetaVar] }
  public func parseAstGrepStructured(_ value: JSONValue) -> (matches: [AstGrepMatch], pattern: String?, lang: String?, shownOf: (Int, Int)?)?
  ```
- `AstGrepBodyView(entry: ToolEntry, runID: String?, host: String?)`: header `N matches in M files` + `pattern`/`lang` Badges + amber `showing first N of M` when truncated; per-file collapsible groups (path + count Badge, default open); per match: `path:line:col` link-styled `dataMono` + `tree` ghost button, the snippet with each metavar range tinted `rupuWarnBg`/`rupuWarn` (`AttributedString` ranges built with `String.unicodeScalars`-safe codepoint slicing — mirror `HighlightedMatch`'s `Array.from` discipline, `ToolCard.tsx:610-640`), a `$name = text` bindings grid, and independent toggles for inline `SourcePreviewView` / `AstTreeView`. Structured payload absent → plain text fallback rendering of `entry.output`.
- `SourcePreviewView(runID: String, path: String, line: Int, host: String?)` — lazy `readSource` on first expand: gutter `n` (`dataMono`, `rupuMute`, right-aligned) + line text; the `targetLine` row tinted `rupuWarnBg`; per-line `CodeHighlighter` when `language ∈` the preview set; `available:false` → `reason` text; loading → `ProgressView`.
- `AstTreeView(runID: String, path: String, line: Int, col: Int, host: String?)` — lazy `readAst`: recursive `DisclosureGroup` rows — optional `field:` prefix (`rupuDim`), `kind` `dataMono`, range `start:col–end:col` (`metaText`, `rupuMute`); `matched` row tinted `rupuWarnBg` with its ancestor chain auto-expanded (compute the path to the matched node first); named-only by default + `show anonymous` Toggle (anonymous at 70% opacity); `truncated` → "tree truncated (large file)" note.
- [ ] **Step 1: Failing tests** — `parseAstGrepStructured` against the Task 2 fixture (paths/metavar offsets); codepoint-slicing correctness on a snippet containing a multibyte char before the metavar; matched-ancestor path computation on a 3-deep tree.
- [ ] **Step 2:** RED → implement → GREEN. **Commit** — `feat(macos-transcript): ast_grep rich body + source preview + CST tree`

### Task 7: Turn feed, FindingCard, screen sweep + checkpoint

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Rendering/FindingCard.swift`
- Rewrite: `RupuKit/Sources/RupuRunDetail/TranscriptFeed.swift`
- Modify: `RupuKit/Sources/RupuRunDetail/{SessionDetailScreen,AgentRunDetailScreen}.swift` (only if the feed's new init needs threading `runID`/`host`)
- Modify: `CLAUDE.md` (RupuRunDetail module line: rich transcript note)
- Test: existing `TranscriptFeed`-adjacent suites adapt to the new row structure

**Interfaces:**
- `FindingCard(entry: ToolEntry, runID: String?, host: String?)` — reads the finding fields from `entry.structured` (a `.finding`-kind entry); per `FindingCard.tsx`: severity hairline bar (`Color.severity`), severity pill + scope/concern-id chips, severity-tinted bold summary, clickable `path:start–end` chip toggling `SourcePreviewView`, rationale via `MarkdownView`, code excerpt `CodeBlock`, reference links.
- `TranscriptFeed` rewrite: `buildTranscriptViewModel(events:)` → one collapsible `TurnRow` per `TurnVM` — collapsed: chevron + ~100-char snippet (`uiText`) + tool-count pill (`Icon` + count), finding pill (warn, only >0), result pill (ok/err/running); expanded: `assistant` Eyebrow, `MarkdownView(assistantText)`, collapsible thinking (dim, 2px left border, collapsed default), then `ToolCardView`/`FindingCard`/`AstGrepBodyView` per entry (dispatch on `ToolKind`). `gate_requested` and `run_complete` rows keep their current standalone treatments, restyled with `TintBanner`/tokens. Auto-scroll live tail preserved exactly as today (`TranscriptFeed.swift:58-62` semantics).
- [ ] **Step 1:** Implement; adapt tests; `make macos-test` + `make macos-build` green. Grep `RupuRunDetail` for `JSONSerialization` pretty-print leftovers and the old `ProseRow`/`ToolCallRow` names → 0.
- [ ] **Step 2:** CLAUDE.md notes. Full gates: `make macos-test && make macos-build && cargo test -p rupu-cp`.
- [ ] **Step 3:** `make macos-run` — screenshots (both themes) of a run transcript with markdown + diff + ast_grep beside the web, for matt's checkpoint. **Commit** — `feat(macos-transcript): turn-based rich transcript feed + finding cards — checkpoint evidence`

---

## Self-review notes

- Spec §5 coverage: carve-out→T1; turn structure→T3/T7; markdown→T4; pairing/audit badges→T3/T5; bodies incl. diff-finally-rendered→T5; ast_grep/source/AST + new client surface→T2/T6; finding card→T7; session/agent inherit→T7 (same feed).
- Type consistency: `ToolEntry`/`TurnVM`/`ToolKind`/`CodeHighlighter`/`APISourceSlice`/`APIAstNode` names identical across tasks; T5/T6/T7 dispatch on the T3 model.
- Order matters: T1 (CodeBlock) before T4/T6; T2 (models) before T3/T6; T3 before T5/T7.
- Known deltas vs web, accepted: GFM tables render as mono block; no intra-line word diff (web lacks it too); netflow transcript events stay unrendered (web drops them as well).
