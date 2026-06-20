# CP Transcript rendering upgrade (Slice A.2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the Control Plane transcript viewer to render markdown + highlighted code, format each tool call correctly, show Okesu-style finding cards, group into collapsible turns, and fix three correctness bugs.

**Architecture:** Frontend-only (`crates/rupu-cp/web`). The pure `transcriptView` builder is upgraded + fully unit-tested; new focused presentational components (`Markdown`, `FindingCard`, `ToolCard`, `DiffView`, `TerminalBlock`, `StructuredView`, `Turn`) render the view model. Markdown libs lazy-load into the already-lazy transcript route chunk.

**Tech Stack:** React 18 + TypeScript + react-markdown + rehype-highlight (highlight.js).

**Spec:** `docs/superpowers/specs/2026-06-19-rupu-cp-transcript-rendering-design.md`

**Branch:** `feat-cp-transcript-rendering` (stacks on #323 → #322 → …; rebase onto `main` as the stack merges).

**Open-decision resolutions:** last turn expanded / earlier collapsed; highlight theme github-light; dispatch_agent rows link to the child transcript.

---

## Verified transcript shapes (from the investigation — rely on these)
- `rupu_transcript::Event` is adjacently tagged `{type, data}`. Types: `run_start{run_id,agent,provider,model,started_at,mode}`, `assistant_message{content, thinking?}`, `tool_call{call_id, tool, input}`, `tool_result{call_id, output, error?, duration_ms}`, `file_edit{path, kind, diff}`, `command_run{argv, cwd, exit_code, stdout_bytes, stderr_bytes}`, `usage{input_tokens,output_tokens,cached_tokens}`, `run_complete{run_id,status,total_tokens,duration_ms,error?}`. (Plus `action_emitted`/`gate_requested` — absent in current data.)
- **`report_finding` is a `tool_call`** with `tool:"report_finding"`, `input:{ severity: 'info'|'low'|'medium'|'high'|'critical', summary: string, scope: 'line'|'file'|'repo', file_path?: string, line_range?: [number,number], concern_id?: string, evidence: { code_excerpt?: string, rationale: string, references?: string[] } }`. Paired result is `{output:"finding_id: …"}`.
- **`command_run.argv`** is `["/bin/sh","-c","<cmd>"]` → the real command is `argv[2]`. Output text is in the paired `bash` tool_result, NOT in command_run.
- Tools: `read_file{path}`, `grep{pattern,path?}`, `glob{pattern}`, `write_file{path,content}`, `edit_file{path,old_string,new_string}`, `bash{command}`, `dispatch_agent{agent,prompt}` (result JSON `{output, tokens_used, transcript_path, sub_run_id, findings}`), `coverage_*`.
- The CP web tailwind already has the `sev.*` ramp (`critical #9333ea`, `high #dc2626`, `medium #ea580c`, `low #ca8a04`, `info #64748b`).
- The transcript route is already lazy-loaded (Slice A.1 code-split) — markdown deps should land in that chunk, not the main entry.

---

### Task 1: Upgrade `transcriptView` (the pure builder) + tests

**Files:** Modify `crates/rupu-cp/web/src/components/transcript/transcriptView.ts`, `src/components/transcript/transcriptView.test.ts`.

This is the core. Read the current file first (it has the old `action_emitted`/`user_message` branches + tool pairing). Restructure the output into **turns**, tag tool items, and fix the 3 bugs.

- [ ] **Step 1: define the new view model + write failing tests.** The model:
```ts
export type Severity = 'info'|'low'|'medium'|'high'|'critical';
export interface FindingView { severity: Severity; summary: string; scope: string; filePath?: string; lineRange?: [number,number]; concernId?: string; rationale: string; codeExcerpt?: string; references: string[] }
export interface ToolView {
  callId?: string; tool: string; input: unknown; output?: string; error?: string; durationMs?: number;
  kind: 'finding'|'read'|'grep'|'glob'|'diff'|'terminal'|'subrun'|'coverage'|'generic';
  finding?: FindingView;               // kind==='finding'
  diff?: { path: string; editKind: string; diff: string };  // kind==='diff' (from paired file_edit)
  terminal?: { command: string; cwd: string; exitCode: number };  // kind==='terminal' (from paired command_run)
}
export interface TurnView { assistant?: { content: string; thinking?: string }; tools: ToolView[]; summary: { toolCount: number; findingCount: number; result: 'ok'|'error'|'running' } }
export interface TranscriptView { header: TranscriptHeader | null; turns: TurnView[]; footer: TranscriptFooter | null }
```
   Tests (`transcriptView.test.ts`):
   - a `report_finding` tool_call (with the input above) → exactly one ToolView `kind:'finding'` whose `finding` carries severity/summary/rationale/references — and assert NO reliance on `action_emitted` (a stream with only the report_finding tool_call produces the finding).
   - a `bash` tool_call + a following `command_run{argv:["/bin/sh","-c","ls -la"], exit_code:0}` → a ToolView `kind:'terminal'` with `terminal.command === "ls -la"` and `exitCode 0` (NOT empty).
   - an `edit_file` tool_call + a following `file_edit{path, kind:'modify', diff}` → a ToolView `kind:'diff'` carrying the diff string.
   - a `read_file` tool_call → `kind:'read'`; `grep` → `kind:'grep'`; `dispatch_agent` → `kind:'subrun'`; `coverage_status` → `kind:'coverage'`; an unknown tool → `kind:'generic'`.
   - **turn grouping**: `[assistant_message(content A), tool_call(read), tool_result, assistant_message(content B), tool_call(report_finding), tool_result]` → 2 turns; turn 1 `{assistant.content:A, tools:[read], summary:{toolCount:1, findingCount:0}}`, turn 2 `{assistant.content:B, tools:[finding], summary:{toolCount:1, findingCount:1}}`.
   - header from run_start, footer from run_complete/usage (carry over the existing behavior).
   - a fixture that previously hit the dead `user_message`/`action_emitted` branches now routes through the normal path without producing a phantom item.

- [ ] **Step 2: run tests → fail.** `npm test -- --run transcriptView`.

- [ ] **Step 3: implement.** Rewrite `buildTranscriptView(events)`:
  - Iterate events; pair `tool_result` to `tool_call` by `call_id`; pair the next `file_edit`/`command_run` to the preceding edit/bash tool by adjacency.
  - For each tool_call, set `kind` by tool name (`report_finding`→finding (parse `input` into `FindingView`); `read_file`→read; `grep`→grep; `glob`→glob; `write_file`/`edit_file`→diff (attach the paired `file_edit.diff`); `bash`→terminal (command = `argv[2]` from the paired `command_run`, exitCode from it); `dispatch_agent`/`dispatch_agents_parallel`→subrun; `coverage_*`→coverage; else→generic).
  - Group into turns: a new turn starts at each `assistant_message`; its `tools` are the tool items until the next assistant_message (tools before the first assistant_message go in a leading turn with no assistant). Compute `summary` (toolCount, findingCount = tools where kind==='finding', result = error if any tool errored else running if no run_complete else ok).
  - Header from `run_start`; footer from `run_complete` (status/tokens/duration) or `usage` fallback. **Remove the `action_emitted`-finding and `user_message` branches.**
  - `coerce`/narrow `input` with `unknown` + type guards (no `any`). Helper `asFinding(input): FindingView | null` validates the report_finding shape.

- [ ] **Step 4: run → PASS.** `npm test -- --run transcriptView`. `npm run build` (strict).
- [ ] **Step 5: commit** `feat(cp/web): upgrade transcriptView — tool kinds, findings from report_finding, turns, bug fixes`.

---

### Task 2: `Markdown` component (react-markdown + rehype-highlight, lazy)

**Files:** Modify `crates/rupu-cp/web/package.json`; create `src/components/transcript/Markdown.tsx`; maybe `vite.config.ts`.

- [ ] **Step 1: add deps.** `cd crates/rupu-cp/web && npm i react-markdown rehype-highlight highlight.js`. Commit the lockfile.
- [ ] **Step 2: `Markdown.tsx`** — `export default function Markdown({ text }: { text: string })` wrapping `<ReactMarkdown rehypePlugins={[rehypeHighlight]}>{text}</ReactMarkdown>` with rupu prose styles via a wrapper `div` + a `components` map (or a `prose`-ish className) for headings/lists/inline code/links. Import the github-light highlight.js theme CSS (`import 'highlight.js/styles/github.css'`) — verify the import path; pick a light theme. Keep it focused.
- [ ] **Step 3: chunking.** `npm run build` → confirm `react-markdown`/`highlight.js` land in a route/transcript chunk, NOT the main entry (main should stay ~48 KB). If they leak into main, add a `markdown` group to `vite.config.ts` `manualChunks` (`markdown: ['react-markdown','rehype-highlight','highlight.js']`). Paste the chunk summary showing the main entry size.
- [ ] **Step 4:** `npm test -- --run` passes. Commit `feat(cp/web): Markdown component (react-markdown + rehype-highlight, lazy)`.

---

### Task 3: `StructuredView` (dep-free recursive KV)

**Files:** Create `src/components/transcript/StructuredView.tsx`, `StructuredView.test.tsx`.

- [ ] **Step 1: failing test** — `StructuredView` given `{a: 1, b: true, c: ["x","y"], d: {e: "f"}}` renders keys `a/b/c/d` and values (NOT `[object Object]`); given a homogeneous array of objects renders a table (header = union of keys). (testing-library + jsdom.)
- [ ] **Step 2: implement** — `export default function StructuredView({ value }: { value: unknown })`: recursive renderer — objects → indented KV rows (`key: <StructuredView value/>`); homogeneous record arrays (all elements objects with overlapping keys) → a small `<table>`; booleans → green/slate pill; numbers → mono; strings (short → inline; long/multiline → `<pre>`); scalar arrays → comma-joined chips; depth cap (e.g. >4) → `<pre>{JSON.stringify}`. STATIC Tailwind. No `any` (narrow `unknown`).
- [ ] **Step 3:** test PASS; `npm run build`. Commit `feat(cp/web): StructuredView (recursive KV renderer for tool JSON)`.

---

### Task 4: `FindingCard` (Okesu finding card)

**Files:** Create `src/components/transcript/FindingCard.tsx` + a shared `src/lib/severity.ts` (if not already present — check `CoverageDetail.tsx` for an existing `SEV_STYLES`; reuse/lift it to `lib/severity.ts` so finding card + coverage share one map).

- [ ] **Step 1:** `lib/severity.ts` — `export const SEVERITY_STYLE: Record<Severity, { text: string; bg: string; ring: string; bar: string; label: string }>` keyed `critical/high/medium/low/info` mapping to the `sev.*` palette (e.g. critical → `{ text:'text-sev-critical', bg:'bg-purple-50', ring:'ring-purple-200', bar:'bg-[#9333ea]', label:'CRITICAL' }` …). STATIC class strings. (If `CoverageDetail.tsx` already has this exact map, move it here and have CoverageDetail import it — DRY.)
- [ ] **Step 2: `FindingCard.tsx`** — props `{ finding: FindingView }` (from Task 1). Render the Okesu anatomy: a severity top-hairline (`<div class="h-1 {bar}"/>`), a `.severity-badge`-style pill (`{bg} {text} ring-1 {ring}`) with the severity label, `scope` + `concern_id` chips, a severity-tinted `summary` title, a `file_path:line_range` location chip, the `rationale` as **`<Markdown>`** (Task 2) body, the `code_excerpt` as a code block (`<pre>` or `<Markdown>` fenced), and `references` as a link list (each `<a href target=_blank rel=noopener>`). Match the approved mockup `.superpowers/brainstorm/30886-1781918234/content/transcript-upgrade.html`. STATIC Tailwind. A small render test (severity badge text + summary present).
- [ ] **Step 3:** test PASS; `npm run build`. Commit `feat(cp/web): FindingCard (Okesu finding-card from report_finding)`.

---

### Task 5: `DiffView` + `TerminalBlock`

**Files:** Create `src/components/transcript/DiffView.tsx`, `DiffView.test.ts`(logic), `src/components/transcript/TerminalBlock.tsx`.

- [ ] **Step 1: diff parse test** — a pure helper `parseDiff(diff: string): { type:'hunk'|'add'|'del'|'ctx'; text:string }[]` : given `"@@ -1,1 +1,1 @@\n- old\n+ new"` → `[{type:'hunk',...},{type:'del',text:'- old'},{type:'add',text:'+ new'}]`. Test it.
- [ ] **Step 2: `DiffView.tsx`** — `{ diff: string; path?: string; editKind?: string }`: parse the unified diff (the helper), render each line colored (del = red bg, add = green bg, hunk = grey, ctx = plain), monospace. Header = `editKind` + `path`. STATIC Tailwind.
- [ ] **Step 3: `TerminalBlock.tsx`** — `{ command: string; output?: string; exitCode?: number; cwd?: string }`: a dark slate-900 block — a `$ {command}` line, the `output` body (mono, `whitespace-pre-wrap`), and an exit badge (green if 0, red otherwise) + cwd meta. STATIC Tailwind.
- [ ] **Step 4:** tests PASS; `npm run build`. Commit `feat(cp/web): DiffView + TerminalBlock`.

---

### Task 6: `ToolCard` dispatcher

**Files:** Create `src/components/transcript/ToolCard.tsx`.

- [ ] **Step 1: implement** — `{ tool: ToolView }`: a card with a header (tool name in mono brand, a short input summary — e.g. read_file → path, grep → pattern, bash → command, finding → severity, dispatch → agent — and a result/exit badge: `error`→red, completed→green, running→pulse) and a collapsible body that switches on `tool.kind`:
  - `finding` → `<FindingCard finding={tool.finding}/>` (rendered inline, not collapsed).
  - `read` → line-numbered code block of `tool.output` (split lines, prefix numbers) in a `<pre>`.
  - `grep` → `tool.output` as a match list (grouped by file if cheap; else a `<pre>`).
  - `glob` → path list.
  - `diff` → `<DiffView diff={tool.diff.diff} path={tool.diff.path} editKind={tool.diff.editKind}/>`.
  - `terminal` → `<TerminalBlock command={tool.terminal.command} output={tool.output} exitCode={tool.terminal.exitCode} cwd={tool.terminal.cwd}/>`.
  - `subrun` → a callout: the dispatched agent, output preview, tokens, and a **link to the child transcript** (`/transcript?path=<output.transcript_path>` — parse `tool.output` JSON for `transcript_path`; guard if absent).
  - `coverage` → `<StructuredView value={parsedOutputJson}/>` (try `JSON.parse(tool.output)`, fall back to `<pre>`).
  - `generic` → if `tool.output` parses as JSON → `<StructuredView>`, else `<pre>`; show `tool.input` via `<StructuredView>` too. Error (`tool.error`) shown in red.
   Collapsible: tool body collapsed by default EXCEPT findings (always shown). STATIC Tailwind, no `any`.
- [ ] **Step 2:** `npm run build` strict + `npm test -- --run`. Commit `feat(cp/web): ToolCard dispatcher (per-tool rendering)`.

---

### Task 7: `Turn` (collapse) + `TranscriptPanel` rewire

**Files:** Create `src/components/transcript/Turn.tsx`; modify `src/components/TranscriptPanel.tsx`.

- [ ] **Step 1: `Turn.tsx`** — `{ turn: TurnView; defaultExpanded: boolean }`: a collapsible group. Collapsed: a summary header — an assistant-snippet (first line of content) + pills (`{toolCount} tools`, `{findingCount} findings` in red when >0) + a result chip (ok/error/running) + a chevron. Expanded: the assistant message via `<Markdown text={turn.assistant.content}/>`, a collapsible thinking block (`<Markdown>` dim) if present, then the `tools.map(t => <ToolCard tool={t}/>)`. Local `useState(defaultExpanded)`.
- [ ] **Step 2: `TranscriptPanel.tsx`** — consume the upgraded `buildTranscriptView(events)` (now returns `{header, turns, footer}`); render the header, then `turns.map((t, i) => <Turn turn={t} defaultExpanded={i === turns.length - 1} />)` (last turn expanded, earlier collapsed), then the footer. Keep the existing live-append + path-change reset + connection indicator from Slice A. Ensure the markdown CSS theme import is loaded (via `Markdown.tsx`).
- [ ] **Step 3: build + tests** — `npm run build` (strict; confirm main entry chunk still ~48 KB, markdown in a lazy chunk) + `npm test -- --run` (the transcriptView + component tests pass; existing tests still green). Paste the chunk summary + test line.
- [ ] **Step 4: commit** `feat(cp/web): turn-collapse + wire upgraded transcript rendering`.

---

## Self-review

**Spec coverage:** markdown+highlight ✓ (T2, used in T4/T7); per-tool formatting ✓ (T6 dispatcher using T3/T4/T5); finding cards from report_finding ✓ (T1 detection + T4 card); StructuredView ✓ (T3); turn-collapse ✓ (T7); the 3 fixes ✓ (T1: report_finding-not-action_emitted, command_run.argv, removed user_message); frontend-only ✓; lazy markdown / main chunk preserved ✓ (T2/T7 verify). Open-decision resolutions baked in (last turn expanded T7; github-light T2; dispatch child link T6).

**Placeholder scan:** the "check CoverageDetail for an existing SEV_STYLES" (T4) is a DRY directive with the concrete fallback (define the map); the chunk-verification steps name the exact check + fallback (`manualChunks` group). No TBD; every component has its props + render contract.

**Type consistency:** `TurnView`/`ToolView`/`FindingView`/`Severity` defined in T1 (transcriptView) and consumed by `FindingCard`(T4), `ToolCard`(T6), `Turn`(T7); `SEVERITY_STYLE` in `lib/severity.ts` (T4) shared; `parseDiff` (T5) consumed by `DiffView`; `Markdown` (T2) consumed by FindingCard/Turn.

**Notes for the executor:** frontend-only; no backend/Rust. `#strict` tsc, no `any` (narrow `unknown`), STATIC Tailwind (no `bg-${x}` — use the SEVERITY_STYLE literal maps + inline `style` for any dynamic color). The web UI is matt-validated (faithful to the approved mockup). Keep the main entry chunk small (markdown libs lazy). Tasks build on T1 (the view model) — do T1 first, then T2–T5 (independent components), then T6 (dispatcher needs T3/T4/T5), then T7 (needs T1/T2/T6). Stacks on #323.
