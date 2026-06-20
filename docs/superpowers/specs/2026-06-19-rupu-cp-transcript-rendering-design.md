# rupu Control Plane — Transcript rendering upgrade (Slice A.2) — Design

**Date:** 2026-06-19
**Author:** matt + Claude
**Status:** Design draft (visual design validated via brainstorm companion)
**Builds on:** Slice A (PR #322, the transcript viewer) + Slice A.1 (PR #323). Stacks on those; rebase onto `main` as the stack merges.

## Summary

Slice A shipped a transcript viewer that renders the agent event stream as a plain conversation (plain-text messages, JSON tool cards). matt's visual pass (#4) asked for it to "better parse the information": **render markdown + highlighted code, format each tool call correctly, and show findings in a specific format like Okesu.** This slice upgrades the rendering — grounded in a study of Okesu's UI and a map of rupu's actual transcript content.

Deliverables:
1. **Markdown + highlighted code** for assistant messages (and thinking) via `react-markdown` + `rehype-highlight`, lazy-loaded into the transcript route chunk (main bundle unaffected).
2. **Per-tool formatting** — each builtin tool renders in a bespoke shape (read_file → line-numbered code; grep → matches; bash → dark terminal + exit badge; edit/write → diff; dispatch_agent → sub-run card; coverage tools → chips/tables; generic → a structured KV view, not raw JSON).
3. **Okesu-style finding cards** — built from `report_finding` tool calls, with the Okesu severity ramp (critical purple → high red → medium orange → low yellow → info slate) and finding-card anatomy.
4. **Clean conversation + turn-collapse** — keep the clean role-labeled card style (not avatar bubbles), but group at the turn/tick level with a collapsible summary header (N tool calls · N findings · result pill) for long runs.
5. **Three correctness fixes** found in the current renderer.

This is a **frontend-only** slice (`crates/rupu-cp/web`). No backend changes — the transcript endpoint already streams the full event records.

## Design decisions (locked with matt)
- **Markdown stack:** `react-markdown` + `rehype-highlight` (highlight.js), lazy-loaded into the transcript chunk.
- **Container:** clean conversation (role labels + focused cards) + **turn-collapse** with summary headers.
- **Finding rendering:** Okesu finding-card anatomy + the vendored `sev.*` ramp (already in `crates/rupu-cp/web/tailwind.config.ts`).
- **Generic tool I/O:** a recursive `StructuredView` (dep-free KV/table) instead of a raw JSON dump (Okesu's `StructuredView` pattern).

## Three correctness fixes (from the transcript investigation)
1. **Findings come from `report_finding` tool calls, NOT `action_emitted`.** The current `transcriptView.ts` has a dead `action_emitted → finding` branch — the runtime never emits `action_emitted`. Findings are `ToolCall{tool:"report_finding", input:{severity, summary, scope, file_path?, line_range?, concern_id?, evidence:{code_excerpt?, rationale, references?}}}` paired with a `ToolResult{output:"finding_id: …"}`. **The finding card must be built from the `report_finding` ToolCall.input.** Remove the dead `action_emitted` finding branch.
2. **`command_run` carries `argv` (not `command`).** `CommandRun { argv: Vec<String>, cwd, exit_code, stdout_bytes, stderr_bytes }`; the real command is `argv[2]` (always `["/bin/sh","-c","<cmd>"]`). The current renderer reads `data.command`/`data.cmd` → empty. Fix to `argv[2]` + show `exit_code` (colored) + cwd; the output body comes from the **paired `bash` ToolResult**.
3. **No `user_message` event** exists in the Rust schema — remove that unreachable branch from `transcriptView.ts`.

## The rendering map (authoritative — from the transcript investigation)

Transcript = JSONL of `rupu_transcript::Event` (adjacently tagged `{type, data}`). Per event/tool:

| Event / tool | Rendering |
|---|---|
| `run_start` | Header: agent · provider · model · mode · started_at |
| `assistant_message.content` | **Markdown** (react-markdown + rehype-highlight) |
| `assistant_message.thinking` | Collapsible markdown "thinking" block (dim; usually null) |
| `tool_call` + `tool_result` (generic) | Tool card; pair by `call_id`; output via `StructuredView` (JSON) or `<pre>` (plain) |
| `tool_call{read_file}` (`input.path`) | Line-numbered code block; header = path + range |
| `tool_call{grep}` (`input.pattern`) | Match list grouped by file; header = pattern + count |
| `tool_call{glob}` (`input.pattern`) | Path list |
| `tool_call{write_file/edit_file}` | **Diff view** from the adjacent `FileEdit.diff` |
| `tool_call{bash}` (`input.command`) | **Dark terminal block** + exit badge (from paired `CommandRun.exit_code`) |
| `tool_call{report_finding}` | **Finding card** (severity-colored, from `input`) |
| `tool_call{coverage_mark/status/remaining/concerns_*}` | Coverage chips/tables via `StructuredView` |
| `tool_call{dispatch_agent[s_parallel]}` | **Sub-run callout** — output + tokens + a link to the child transcript (`output.transcript_path`) |
| `file_edit` `{path, kind, diff}` | Diff view (header = kind + path); pair to the preceding edit tool by adjacency |
| `command_run` `{argv, cwd, exit_code, stdout_bytes, stderr_bytes}` | Terminal meta (cmd = `argv[2]`, exit badge, cwd, byte counts); body from paired bash result |
| `usage` `{input_tokens, output_tokens, cached_tokens}` | Fold into the footer token totals |
| `run_complete` | Footer: status, total_tokens, duration_ms, error |
| `action_emitted` / `gate_requested` | Defensive support; absent in current transcripts (render a small generic row if seen) |
| `turn_start` / `turn_end` / `assistant_delta` | Skip (deltas coalesced into assistant_message) |

## Components (frontend, `crates/rupu-cp/web/src/components/transcript/`)

Keep the existing `transcriptView.ts` as the **pure builder** (upgrade it) + `TranscriptPanel.tsx` as the live container; add focused presentational sub-components:

- **`transcriptView.ts`** (upgrade) — the pure mapping. Changes: tag each tool item with its `tool` name + parsed `input`; detect `report_finding` → a `finding` view item built from the input; pair `file_edit`/`command_run` to their tool by call_id/adjacency; group items into **turns** (a turn = an assistant_message + the tool calls until the next assistant_message), each with a summary (tool count, finding count, terminal result). Remove the dead `action_emitted`-finding + `user_message` branches. Fully unit-tested.
- **`Markdown.tsx`** — wraps `react-markdown` + `rehype-highlight` with the rupu prose styles (headings, lists, inline code, fenced code w/ highlight.js theme). Lazy-loaded (the whole transcript route is already lazy via Slice A.1 code-split; ensure the markdown libs land in that chunk, not the main bundle). Used for assistant content + thinking.
- **`FindingCard.tsx`** — the Okesu finding card from a `report_finding` input: severity top-hairline + `.severity-badge` (the `sev.*` ramp), severity-tinted `summary` title, `scope`/`concern_id` chips, `file_path:line_range` location, `evidence.rationale` (markdown) body, `evidence.code_excerpt` as a code block, `evidence.references` as a link list (CWE/URLs). A `severityStyle(sev)` helper (shared map; reuse/extend the existing severity styles from `CoverageDetail.tsx`/`StatusPill`).
- **`ToolCard.tsx`** — the tool dispatcher: a header (tool name + a short input summary + a result/exit badge), collapsible body; switches on `tool` to the bespoke renderer (read_file/grep/glob → code/list; bash → `TerminalBlock`; edit/write → `DiffView`; report_finding → `FindingCard`; dispatch_agent → sub-run card; coverage_* + generic → `StructuredView`/`<pre>`).
- **`DiffView.tsx`** — render a unified diff (`FileEdit.diff`) with red/green lines + `@@` hunk headers. Lightweight (no dep — parse the diff string).
- **`TerminalBlock.tsx`** — dark slate-900 terminal for bash (command `argv[2]` + output from the paired result + exit badge).
- **`StructuredView.tsx`** — a dependency-free recursive key/value renderer for arbitrary JSON tool I/O (Okesu's pattern): objects → indented KV; homogeneous record arrays → a small table; booleans/numbers styled; long strings → `<pre>`; depth cap → JSON fallback. Replaces raw `JSON.stringify` for tool inputs/outputs.
- **`Turn.tsx`** — a collapsible turn group: a summary header (assistant snippet + "N tools · N findings" pills + result), expandable to the full message + tool cards. Collapsed by default only for old/long turns (e.g. keep the last turn expanded; collapse earlier ones, or collapse all past a threshold — pick a sensible default, expand-on-click).
- **`TranscriptPanel.tsx`** (modify) — consume the upgraded `transcriptView` (turns) + render `Turn`s; keep the live-append + path-change reset + the header/footer from Slice A.

## Dependencies
Add `react-markdown`, `rehype-highlight` (+ `highlight.js` for the theme CSS) to `crates/rupu-cp/web/package.json`. Verify the vite `manualChunks` (from A.1) keeps them OUT of the main entry chunk — they should land in the transcript/RunDetail route chunk (which is already lazy). If needed, add a `markdown` manualChunks group. Confirm the post-build main entry chunk stays small (~48 KB).

## Testing
- **`transcriptView.test.ts`** (extend) — the pure builder: a `report_finding` tool call → a `finding` view item with severity/summary/evidence (and NO reliance on `action_emitted`); a `bash` tool + `command_run` → a terminal item with `argv[2]` as the command + exit_code; an `edit_file` + `file_edit` → a diff item; turn grouping (assistant + its tools form one turn with the right summary counts); the dead `user_message`/`action_emitted` branches removed (a fixture that previously hit them now routes correctly).
- **Component tests** (where cheap, testing-library): `FindingCard` renders the severity badge + title from an input; `StructuredView` renders an object as KV (not `[object Object]`); `DiffView` colors +/- lines.
- `npm run build` (strict, main chunk stays small) + `npm test -- --run`. Rendering validated by matt (same rule as the rest of the CP).

## Open decisions for review
1. **Turn-collapse default** — collapse all but the last turn, vs. collapse only past N turns, vs. all-expanded-with-a-collapse-all. Lean: keep the **last** turn expanded, earlier turns collapsed (summary visible), expand-on-click — best for long runs.
2. **highlight.js theme** — pick a light theme matching the rupu palette (e.g. github-light). Cosmetic; lean github-light.
3. **dispatch_agent child-transcript link** — link to the child via `output.transcript_path` (opens the sub-run transcript in the same panel/a new view). Lean: yes, link it (the path is already in the tool output + validated by the transcript endpoint).
