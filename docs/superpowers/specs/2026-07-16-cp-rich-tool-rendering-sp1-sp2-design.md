# CP rich tool-call rendering — SP1 (tool structured output) + SP2 (ast_grep pilot renderer) — design

**Date:** 2026-07-16
**Status:** approved-in-principle (decomposition + ordering approved); detailed design below
**Part of:** the "CP rich tool-call rendering" initiative (TODO.md). Four layered sub-projects; this spec covers SP1 + SP2. SP3 (in-project file parser + source access) and SP4 (AST visualization) are separate specs.

## Motivation

The CP transcript renders `ast_grep` output as an unstyled fallback blob, and even `grep` is a bare newline-split with a count. `ast_grep` produces genuinely structured results — matched nodes plus **metavariable bindings** (`$VAR`, `$$$`) with source ranges — but the tool currently reformats ast-grep's JSON down to `path:line:col: text`, discarding the structure, and the CP has no way to recover it (it is read-only, cannot re-run the tool). This spec (a) carries the structured match data through the transcript to the CP without degrading the compact text the LLM reads, and (b) renders `ast_grep` results richly in the CP as the pilot for the broader no-unstyled-blobs initiative.

## SP1 — tool structured output (the enabler)

### Wire contract change

Add an optional, generic structured payload to tool results — usable by any tool later, `ast_grep` first:

- `ToolOutput` (`crates/rupu-tools/src/tool.rs`): add `structured: Option<serde_json::Value>` (`#[serde(skip_serializing_if = "Option::is_none", default)]`). Default `None`. Existing `ToolOutput { .. }` literals across the crate gain `structured: None` (or via a constructor/`..Default`).
- `Event::ToolResult` (`crates/rupu-transcript/src/event.rs`): add `structured: Option<serde_json::Value>` (`skip_serializing_if=Option::is_none`, `default`). Backward compatible — old transcripts deserialize with `structured: None`.
- Runner emit sites (`crates/rupu-agent/src/runner.rs`): the main `Ok(out)` arm threads `out.structured` into the `Event::ToolResult` it writes. The other three `ToolResult` writes (permission-deny, unknown-tool, invoke-error paths) pass `structured: None`.
- CP backend (`crates/rupu-cp/src/api/transcript.rs`): **no change** — it reads `Vec<rupu_transcript::Event>` and re-serializes verbatim; the new field flows through automatically.

Rationale: this rides the existing `call_id`-keyed `tool_result` event (no fragile adjacency pairing like `DerivedEvent`), keeps `stdout` untouched so the model's compact `path:line:col:` output is unchanged, and touches the fewest layers.

### `ast_grep` structured payload (v1 schema)

When `ast_grep` parses ast-grep's `--json=stream` output on a success path, it ALSO builds this payload and sets `structured: Some(..)`. `stdout` stays exactly the compact `path:line:col: match` text (LLM-facing, unchanged). Schema:

```jsonc
{
  "tool": "ast_grep",
  "pattern": "impl $T for $S",   // echo of the input
  "lang": "rust",                // echo of the input
  "matchCount": 3,               // total matches found (pre-truncation)
  "fileCount": 2,                // distinct files
  "truncated": false,            // true if matches[] was capped
  "matches": [                   // capped at MAX_STRUCTURED_MATCHES = 200
    {
      "file": "crates/rupu-tools/src/ast_grep.rs", // workspace-relative (prefix stripped)
      "range": { "startLine": 46, "startCol": 1, "endLine": 46, "endCol": 30 }, // 1-based
      "text": "impl Tool for AstGrepTool",         // the matched source (may be multi-line)
      "metaVars": {
        // name -> binding. `textOffset` is the [start,end) CHAR offset of the
        // binding WITHIN this match's `text`, precomputed in Rust so the CP can
        // highlight sub-spans by simple string slicing (no byte/UTF-16 math).
        "single": {
          "T": { "text": "Tool", "textOffset": { "start": 5, "end": 9 } },
          "S": { "text": "AstGrepTool", "textOffset": { "start": 14, "end": 25 } }
        },
        "multi": {
          // name -> [ { text, textOffset }, ... ]
        }
      }
    }
  ]
}
```

Derivation notes for the implementer:
- ast-grep JSON gives 0-based `range.start/end.{line,column}` and `range.byteOffset.{start,end}` for the match and for each metavariable (under `metaVariables.single` / `.multi`). Convert line/col to **1-based** for `range`. Compute each metavar's `textOffset` as CHAR indices into the match `text`: `rel_byte = metavar.byteOffset.start - match.byteOffset.start`, then map `rel_byte` (and the end) to char indices via `text.char_indices()` (guard against out-of-range → skip that metavar's offset, keep its `text`).
- Cap `matches` at `MAX_STRUCTURED_MATCHES = 200`; if the total exceeds it, keep the first 200 and set `truncated: true`. `matchCount`/`fileCount` reflect the true totals. (No silent unbounded transcript growth.)
- The payload is built from the SAME parsed JSON lines the tool already iterates for `stdout`/coverage — one pass, no second ast-grep invocation.
- The stderr-is-error contract is unchanged: on any error path, `structured` is `None` and `stdout` empty (existing behavior).

## SP2 — CP `ast_grep` rich renderer (the pilot)

### View-model plumbing (`crates/rupu-cp/web/src`)

- `lib/transcript.ts`: add `structured?: unknown` to the `tool_result` member of `TranscriptEvent`.
- `components/transcript/transcriptView.ts`:
  - Extend `ToolKind` with `'ast_grep'`; add `classify('ast_grep') → 'ast_grep'`.
  - Add `structured?: unknown` to `ToolView`; in the `case 'tool_result'` reducer, copy `data.structured` onto the paired `ToolView`.
  - Add a `summarizeInput` case for `ast_grep` so the ToolCard header reads e.g. `ast_grep · impl $T for $S · rust`.
- These are the unit-testable seams (transcriptView.ts already has unit tests) — cover: classify maps `ast_grep`; `structured` is copied onto the view; header summary.

### Renderer (`components/transcript/ToolCard.tsx`)

Add `AstGrepBody` + a `tool.kind === 'ast_grep'` branch. Behavior:

1. **If `tool.structured` is present** (SP1 landed): parse it as the v1 payload and render:
   - Header row: match-count badge (`N matches in M files`), pattern + lang chips.
   - **Grouped by file**, each file a collapsible disclosure (reuse `Turn.tsx`'s inline `useState` + chevron pattern — there is no shared disclosure component) showing the file's match count.
   - Each match: the `text` snippet in a mono block; **metavariable bindings highlighted** inline using `metaVars.*.textOffset` (wrap the sub-ranges in colored spans) — MUST-have is a **bindings table** (`$T = Tool`, `$S = AstGrepTool`); inline highlight is a SHOULD.
   - `truncated: true` → a visible "showing first 200 of N" notice (never silently drop).
2. **Fallback if `structured` is absent** (old transcripts, or a future ast-grep that errors): parse `tool.output` text lines (`path:line:col: match`), group by file with a count — i.e. at minimum an upgraded `GrepBody`, never the raw blob.

Light-only styling (the CP is light-only today). No source-file access is required for SP2 — everything needed is in the payload/text. (Clickable `path:line` → source is SP3.)

### Out of scope for SP1+SP2 (tracked in TODO.md)

- Rich renderers for the other tools (`grep` upgrade, `read_file` highlighting, `glob`, terminal/diff/subrun audits, generic-fallback hardening) — SP2 follow-up tasks.
- In-project file parser + clickable source preview — SP3.
- Playground-style AST/tree visualization — SP4.

## Testing

- **SP1 (Rust):** unit tests in `crates/rupu-tools/tests/ast_grep.rs` — assert `structured` is `Some` on a match, with correct `matchCount`/`fileCount`, 1-based `range`, workspace-relative `file`, and metavar bindings incl. `textOffset` for a known fixture (e.g. `fn $NAME($$$PARAMS) -> $RET { $$$ }` over a small rust file → `NAME`/`RET` single + `PARAMS` multi). Assert `stdout` is byte-for-byte the same compact text as before (no LLM-facing regression). Assert `truncated`/cap behavior with a fixture exceeding the cap (use a small cap override if practical, else many matches). Guard with `skip_if_no_ast_grep`. A serde round-trip test for `Event::ToolResult` with `structured` present + absent (backward compat).
- **SP2 (CP TS):** unit tests alongside `transcriptView` tests — classify `ast_grep`; `structured` copied to `ToolView`; text-fallback parser groups by file. Renderer visual correctness (`AstGrepBody`) is verified by `npm run build` clean + a manual/browser visual check (the CP is a React web app — buildable and screenshottable; not GPUI). Flag the visual check explicitly before merge.

## Delivery / integration

- SP1 (Rust) lands first (SP2's wire field depends on it), SP2 (CP) second — one branch `cp-rich-tool-rendering`, one PR.
- No new crate dependencies (Rust); no new npm dependencies (reuse react-markdown/highlight.js/StructuredView already present).
- The CP embeds `crates/rupu-cp/web/dist` at build time — a release shipping this needs `make cp-web` (already the standing release rule).
