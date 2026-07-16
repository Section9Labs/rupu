# CP rich tool-call rendering SP1+SP2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry `ast_grep`'s structured match data (metavariable bindings + ranges) through the transcript to the CP without changing the LLM-facing text, and render `ast_grep` results richly in the CP as the pilot for the no-unstyled-blobs initiative.

**Architecture:** SP1 (Rust) adds a generic `structured: Option<serde_json::Value>` to `ToolOutput` and `Event::ToolResult`, threaded through the runner's `tool_result` emit; `ast_grep` populates it with a v1 payload built from ast-grep's `--json=stream` output (stdout stays the compact `path:line:col:` text). SP2 (CP React/TS) plumbs the field into the transcript view model and adds an `AstGrepBody` renderer (group-by-file, count badge, metavar bindings table + inline highlight), with a text-parse fallback for transcripts lacking the payload.

**Tech Stack:** Rust 2021 (`serde`, `serde_json`, `tokio`); CP web = React + TypeScript + Vite, highlight.js already present. No new crate or npm dependencies.

**Spec:** `docs/superpowers/specs/2026-07-16-cp-rich-tool-rendering-sp1-sp2-design.md`.

## Global Constraints

- Rust 2021, MSRV pinned. Do NOT run workspace-wide `cargo fmt`; format only touched files (`rustfmt <file>`). Pre-existing warnings in untouched files are out of scope.
- Workspace deps only; this plan adds NO new crate dependencies and NO new npm dependencies.
- `unsafe_code` forbidden; `#![deny(clippy::all)]`.
- **The LLM-facing `ast_grep` `stdout` must stay byte-for-byte the compact `path:line:col: <first line of match>` text.** The structured payload is additive, on a separate field. A test must assert stdout is unchanged.
- Wire additions use `#[serde(skip_serializing_if = "Option::is_none", default)]` (backward compatible: old transcripts deserialize with `structured: None`).
- `ast_grep` structured payload v1 schema (from the spec): top-level `{ tool, pattern, lang, matchCount, fileCount, truncated, matches[] }`; each match `{ file (workspace-relative), range:{startLine,startCol,endLine,endCol} (1-based), text (full match), metaVars:{ single:{name:{text,textOffset:{start,end}}}, multi:{name:[{text,textOffset}]} } }`. `textOffset` is the `[start,end)` **char (Unicode-scalar) offset** of the binding within the match `text`. `matches` capped at `MAX_STRUCTURED_MATCHES = 200`; `matchCount`/`fileCount` are true totals; `truncated=true` when capped.
- CP is light-only today; no source-file access (clickable `path:line` is SP3, out of scope).

---

### Task 1: Wire the `structured` field through `ToolOutput` → `Event::ToolResult` → runner

Adds the generic optional structured payload to the tool-output type and the transcript event, threaded through the runner. All tools default it to `None`; only `ast_grep` (Task 2) sets it. Independently testable via a serde round-trip; a reviewer can accept the wire change before any tool populates it.

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs` (`ToolOutput`, ~line 187-200)
- Modify: `crates/rupu-transcript/src/event.rs` (`Event::ToolResult`, ~line 37-43)
- Modify: `crates/rupu-agent/src/runner.rs` (4 `Event::ToolResult` writes: ~1116, ~1149, ~1166, ~1202)
- Modify (compiler-driven `structured: None` fills): every `ToolOutput { .. }` literal — in `crates/rupu-tools/src/{bash,edit_file,grep,ast_grep,read_file,glob,write_file,dispatch_agent,dispatch_agents_parallel}.rs` and `crates/rupu-agent/src/{mcp_tool,coverage_tools}.rs`
- Test: `crates/rupu-transcript/src/event.rs` (inline `#[cfg(test)]`) or its test dir

**Interfaces:**
- Produces: `ToolOutput.structured: Option<serde_json::Value>` and `Event::ToolResult.structured: Option<serde_json::Value>`. Task 2 sets `ToolOutput.structured`; the runner copies it to the event; SP2 reads `data.structured`.

- [ ] **Step 1: Write the failing round-trip test**

Add to `crates/rupu-transcript/src/event.rs` inside a `#[cfg(test)] mod tests { use super::*; ... }` (create the module if absent):

```rust
#[test]
fn tool_result_structured_roundtrips_and_is_omitted_when_none() {
    // Present:
    let e = Event::ToolResult {
        call_id: "c1".into(),
        output: "ok".into(),
        error: None,
        duration_ms: 5,
        structured: Some(serde_json::json!({"tool":"ast_grep","matchCount":2})),
    };
    let s = serde_json::to_string(&e).unwrap();
    assert!(s.contains("\"structured\""));
    let back: Event = serde_json::from_str(&s).unwrap();
    match back {
        Event::ToolResult { structured: Some(v), .. } => {
            assert_eq!(v["matchCount"], 2);
        }
        _ => panic!("expected ToolResult with structured=Some"),
    }

    // Absent → omitted from JSON, and old JSON without the field still parses.
    let e2 = Event::ToolResult {
        call_id: "c2".into(),
        output: "ok".into(),
        error: None,
        duration_ms: 1,
        structured: None,
    };
    let s2 = serde_json::to_string(&e2).unwrap();
    assert!(!s2.contains("structured"));
    let legacy = r#"{"type":"tool_result","data":{"call_id":"c3","output":"x","duration_ms":0}}"#;
    let parsed: Event = serde_json::from_str(legacy).unwrap();
    assert!(matches!(parsed, Event::ToolResult { structured: None, .. }));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-transcript tool_result_structured_roundtrips`
Expected: FAIL to COMPILE — `Event::ToolResult` has no field `structured`.

- [ ] **Step 3: Add the field to `Event::ToolResult`**

In `crates/rupu-transcript/src/event.rs`, in the `ToolResult` variant (after `duration_ms: u64,`):

```rust
    /// Optional structured payload emitted alongside the human/LLM-facing
    /// `output` string (e.g. ast_grep match + metavariable data). Additive
    /// and backward compatible — absent on legacy transcripts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    structured: Option<serde_json::Value>,
```

(If the variant's other fields are `pub`, match their visibility. `serde_json::Value` is already in scope via the `input: Value` field on `ToolCall`; if not, add `use serde_json::Value;` or qualify as `serde_json::Value`.)

- [ ] **Step 4: Add the field to `ToolOutput`**

In `crates/rupu-tools/src/tool.rs`, in `ToolOutput` (after the `derived` field):

```rust
    /// Optional structured payload the runtime copies onto the emitted
    /// `tool_result` event, in addition to `stdout`. Lets a tool ship
    /// machine-rendered data (e.g. ast_grep matches + metavariables) to
    /// the control plane without changing the text the agent reads.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured: Option<serde_json::Value>,
```

- [ ] **Step 5: Thread it through the runner and fix all construction sites (compiler-driven)**

In `crates/rupu-agent/src/runner.rs`, the four `Event::ToolResult { .. }` writes now fail to compile. At the **success arm** (the write inside `Ok(out) => { .. }`, ~line 1166 where `out` is bound) add:

```rust
                            structured: out.structured.clone(),
```

At the **other three** writes (permission-deny ~1116, unknown-tool ~1149, invoke-error ~1202) add `structured: None,`.

Then build the two crates; the compiler lists every `ToolOutput { .. }` literal missing the new field. Add `structured: None,` to each (all of `bash.rs`, `edit_file.rs`, `grep.rs`, `read_file.rs`, `glob.rs`, `write_file.rs`, `dispatch_agent.rs`, `dispatch_agents_parallel.rs`, plus `mcp_tool.rs`, `coverage_tools.rs`). Do NOT touch `ast_grep.rs`'s literal yet if you prefer, but adding `structured: None` there is fine — Task 2 overwrites it.

Run: `cargo build -p rupu-tools -p rupu-agent -p rupu-transcript`
Expected: clean build once every literal has the field.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p rupu-transcript tool_result_structured_roundtrips`
Expected: PASS.

- [ ] **Step 7: Lint, format, full compile check**

Run: `cargo clippy -p rupu-tools -p rupu-agent -p rupu-transcript` (clean on touched files).
Format each touched file with `rustfmt`.
Run: `cargo test -p rupu-tools -p rupu-agent -p rupu-transcript` (no regressions).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(transcript): add structured payload to tool_result / ToolOutput"
```

---

### Task 2: `ast_grep` builds and emits the structured payload

Populates `ToolOutput.structured` with the v1 payload from the ast-grep JSON the tool already parses, in the same pass. `stdout` is unchanged.

**Files:**
- Modify: `crates/rupu-tools/src/ast_grep.rs` (the success-parse loop ~155-209 and the final `ToolOutput`, ~211)
- Test: `crates/rupu-tools/tests/ast_grep.rs`

**Interfaces:**
- Consumes: `ToolOutput.structured` (Task 1).
- Produces: the v1 payload described in Global Constraints on a successful `ast_grep` run; `None` on error/binary-missing paths.

- [ ] **Step 1: Write the failing test**

Add to `crates/rupu-tools/tests/ast_grep.rs`:

```rust
#[tokio::test]
async fn emits_structured_payload_with_metavars() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("s.rs")
        .write_str("fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n")
        .unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "fn $NAME($$$PARAMS) -> $RET { $$$ }", "lang": "rust" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();

    // stdout is still the compact text (unchanged LLM contract).
    assert!(out.stdout.contains("s.rs:1:1: fn add"), "stdout: {}", out.stdout);

    let s = out.structured.expect("structured payload present");
    assert_eq!(s["tool"], "ast_grep");
    assert_eq!(s["lang"], "rust");
    assert_eq!(s["matchCount"], 1);
    assert_eq!(s["fileCount"], 1);
    assert_eq!(s["truncated"], false);
    let m = &s["matches"][0];
    assert_eq!(m["file"], "s.rs"); // workspace-relative
    assert_eq!(m["range"]["startLine"], 1); // 1-based
    assert_eq!(m["metaVars"]["single"]["NAME"]["text"], "add");
    assert_eq!(m["metaVars"]["single"]["RET"]["text"], "i32");
    // textOffset slices back to the binding text within the match text.
    let text = m["text"].as_str().unwrap();
    let chars: Vec<char> = text.chars().collect();
    let o = &m["metaVars"]["single"]["NAME"]["textOffset"];
    let (st, en) = (o["start"].as_u64().unwrap() as usize, o["end"].as_u64().unwrap() as usize);
    assert_eq!(chars[st..en].iter().collect::<String>(), "add");
    // multi metavar present.
    assert!(m["metaVars"]["multi"]["PARAMS"].is_array());
}

#[tokio::test]
async fn structured_is_none_on_error() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "fn $N() { $$$ }", "lang": "rust", "path": "nope_dir" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
    assert!(out.structured.is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-tools --test ast_grep emits_structured_payload_with_metavars`
Expected: FAIL — `out.structured` is `None` (payload not built yet).

- [ ] **Step 3: Build the payload in the parse loop**

In `crates/rupu-tools/src/ast_grep.rs`, add a module-level constant and two helpers (near the top, after imports):

```rust
/// Cap on how many matches ride in the structured payload (transcript-size guard).
const MAX_STRUCTURED_MATCHES: usize = 200;

/// Map a byte offset within `text` to a char (Unicode-scalar) index.
/// Returns None if the byte offset isn't a char boundary/in range.
fn byte_to_char_idx(text: &str, byte_off: usize) -> Option<usize> {
    if byte_off == text.len() {
        return Some(text.chars().count());
    }
    text.char_indices().position(|(b, _)| b == byte_off)
}

/// Build the `textOffset` {start,end} (char indices into the match `text`)
/// for a metavar node, from absolute byte offsets. Returns None if either
/// endpoint can't be mapped.
fn text_offset(match_text: &str, match_byte_start: u64, node: &Value) -> Option<serde_json::Value> {
    let bo = node.get("range")?.get("byteOffset")?;
    let ns = bo.get("start")?.as_u64()?;
    let ne = bo.get("end")?.as_u64()?;
    let rel_s = ns.checked_sub(match_byte_start)? as usize;
    let rel_e = ne.checked_sub(match_byte_start)? as usize;
    let cs = byte_to_char_idx(match_text, rel_s)?;
    let ce = byte_to_char_idx(match_text, rel_e)?;
    Some(serde_json::json!({ "start": cs, "end": ce }))
}

/// Convert one metavar node ({text, range}) into {text, textOffset?}.
fn metavar_binding(match_text: &str, match_byte_start: u64, node: &Value) -> serde_json::Value {
    let text = node.get("text").and_then(Value::as_str).unwrap_or("");
    let mut obj = serde_json::json!({ "text": text });
    if let Some(off) = text_offset(match_text, match_byte_start, node) {
        obj["textOffset"] = off;
    }
    obj
}
```

Then extend the success branch (`if error.is_none() { .. }`). Alongside the existing `by_file` accumulation, collect structured matches. Replace the loop body / add to it so that, per parsed `obj`, after computing `rel_path`, `line`, `col`, you also (when under the cap) build a match entry. Concretely, before the loop declare:

```rust
            let mut matches_json: Vec<serde_json::Value> = Vec::new();
            let mut total_matches: usize = 0;
            let mut files_seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
```

Inside the loop, after `by_file.entry(rel_path.clone()).or_default().push(line);` (note: clone `rel_path` since it's now used twice), add:

```rust
                total_matches += 1;
                files_seen.insert(rel_path.clone());
                if matches_json.len() < MAX_STRUCTURED_MATCHES {
                    let end = obj.get("range").and_then(|r| r.get("end"));
                    let end_line = end.and_then(|e| e.get("line")).and_then(Value::as_u64).unwrap_or(line0) + 1;
                    let end_col = end.and_then(|e| e.get("column")).and_then(Value::as_u64).unwrap_or(col0) + 1;
                    let full_text = obj.get("text").and_then(Value::as_str).unwrap_or("");
                    let match_byte_start = obj
                        .get("range").and_then(|r| r.get("byteOffset"))
                        .and_then(|b| b.get("start")).and_then(Value::as_u64)
                        .unwrap_or(0);

                    let mv = obj.get("metaVariables");
                    let mut single = serde_json::Map::new();
                    if let Some(s) = mv.and_then(|m| m.get("single")).and_then(Value::as_object) {
                        for (name, node) in s {
                            single.insert(name.clone(), metavar_binding(full_text, match_byte_start, node));
                        }
                    }
                    let mut multi = serde_json::Map::new();
                    if let Some(s) = mv.and_then(|m| m.get("multi")).and_then(Value::as_object) {
                        for (name, arr) in s {
                            let items: Vec<serde_json::Value> = arr.as_array().map(|a| {
                                a.iter().map(|node| metavar_binding(full_text, match_byte_start, node)).collect()
                            }).unwrap_or_default();
                            multi.insert(name.clone(), serde_json::Value::Array(items));
                        }
                    }

                    matches_json.push(serde_json::json!({
                        "file": rel_path,
                        "range": { "startLine": line, "startCol": col, "endLine": end_line, "endCol": end_col },
                        "text": full_text,
                        "metaVars": { "single": single, "multi": multi },
                    }));
                }
```

(Note: the existing `by_file.entry(rel_path)` currently MOVES `rel_path`; change it to `by_file.entry(rel_path.clone())` so `rel_path` remains available for the structured entry above/after.)

After the `for (path, matched_lines) in by_file { .. emit .. }` block, build the payload and thread it out. Change the tail so it produces a `structured` local:

```rust
        // (declare before the `if error.is_none()` block so it's in scope at the end)
        let mut structured: Option<serde_json::Value> = None;
```
and at the END of the `if error.is_none()` block (after the coverage emit loop), set:
```rust
            structured = Some(serde_json::json!({
                "tool": "ast_grep",
                "pattern": i.pattern,
                "lang": i.lang,
                "matchCount": total_matches,
                "fileCount": files_seen.len(),
                "truncated": total_matches > matches_json.len(),
                "matches": matches_json,
            }));
```
(`i.pattern` / `i.lang` are still owned here — the coverage loop only used `i.pattern.clone()`.)

Finally, set it on the returned `ToolOutput`:

```rust
        Ok(ToolOutput {
            stdout,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
            structured,
        })
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-tools --test ast_grep`
Expected: PASS — all ast_grep tests including the two new ones (and the pre-existing 6 still green: stdout unchanged, error paths give `structured: None`).

- [ ] **Step 5: Lint + format**

Run: `cargo clippy -p rupu-tools --tests` (clean on `ast_grep.rs`).
Format: `rustfmt crates/rupu-tools/src/ast_grep.rs crates/rupu-tools/tests/ast_grep.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-tools/src/ast_grep.rs crates/rupu-tools/tests/ast_grep.rs
git commit -m "feat(tools): ast_grep emits structured match+metavar payload"
```

---

### Task 3: CP transcript view-model plumbing for `ast_grep` + `structured`

Plumbs the new `structured` field into the CP TS wire type and view model, and classifies `ast_grep` as its own tool kind. Pure, unit-testable logic (no rendering).

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/transcript.ts` (`tool_result` member of `TranscriptEvent`)
- Modify: `crates/rupu-cp/web/src/components/transcript/transcriptView.ts` (`ToolKind`, `classify`, `ToolView`, `case 'tool_result'` reducer, `summarizeInput`)
- Test: the existing transcriptView test file (find it: `crates/rupu-cp/web/src/components/transcript/transcriptView.test.ts` or `.spec.ts`)

**Interfaces:**
- Consumes: on-disk `tool_result.data.structured` (Task 1/2).
- Produces: `ToolView.structured?: unknown` populated; `ToolView.kind === 'ast_grep'` for the ast_grep tool. Consumed by Task 4's renderer.

- [ ] **Step 1: Locate the test file and add failing tests**

Find the transcriptView test file: `ls crates/rupu-cp/web/src/components/transcript/ | grep -i transcriptView`. Add tests mirroring the existing style:

```ts
it("classifies ast_grep as its own kind and carries structured payload", () => {
  const events = [
    { type: "assistant_message", data: { text: "searching" } },
    { type: "tool_call", data: { call_id: "c1", tool: "ast_grep", input: { pattern: "impl $T for $S", lang: "rust" } } },
    { type: "tool_result", data: { call_id: "c1", output: "a.rs:1:1: impl X for Y", duration_ms: 3, structured: { tool: "ast_grep", matchCount: 1, matches: [] } } },
  ];
  const view = buildTranscriptView(events as any);
  const tool = view.flatMap((t) => t.tools).find((x) => x.tool === "ast_grep")!;
  expect(tool.kind).toBe("ast_grep");
  expect((tool.structured as any).matchCount).toBe(1);
});
```

(Match the import path/naming of the existing tests — reuse their `buildTranscriptView` import and event-shape helpers if any.)

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run transcriptView` (or the repo's test script — check `package.json` `scripts.test`).
Expected: FAIL — `kind` is not `'ast_grep'` and `structured` is undefined.

- [ ] **Step 3: Add the wire-type field**

In `crates/rupu-cp/web/src/lib/transcript.ts`, on the `tool_result` member of the `TranscriptEvent` union, add `structured?: unknown` to its `data`:

```ts
  | { type: 'tool_result'; data: { call_id: string; output: string; error?: string | null; duration_ms: number; structured?: unknown } }
```

(Match the exact existing shape of that member; only add the `structured?: unknown` field.)

- [ ] **Step 4: Extend the view model**

In `crates/rupu-cp/web/src/components/transcript/transcriptView.ts`:
- Add `'ast_grep'` to the `ToolKind` union.
- In `classify(tool)`, add `if (tool === 'ast_grep') return 'ast_grep';` (place before the generic fallback).
- Add `structured?: unknown;` to the `ToolView` type.
- In the `case 'tool_result':` reducer, when populating the paired `ToolView`, add `view.structured = data.structured;` (match how `output`/`error` are copied).
- In `summarizeInput`, add an `ast_grep` case producing a header like `` `${input.pattern} · ${input.lang}` `` (guard for missing fields).

- [ ] **Step 5: Run tests to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run transcriptView`
Expected: PASS — new + existing transcriptView tests green.

- [ ] **Step 6: Typecheck + lint touched files**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` (no new type errors).
If the repo has eslint: `npx eslint src/components/transcript/transcriptView.ts src/lib/transcript.ts`.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/lib/transcript.ts crates/rupu-cp/web/src/components/transcript/transcriptView.ts crates/rupu-cp/web/src/components/transcript/transcriptView.test.ts
git commit -m "feat(cp): plumb ast_grep + structured payload into transcript view model"
```

---

### Task 4: `AstGrepBody` rich renderer in `ToolCard.tsx`

Renders `ast_grep` results: from the structured payload when present (group-by-file, count badge, metavar bindings table + inline highlight), else a text-parse fallback. This is the visible pilot.

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/ToolCard.tsx` (add `AstGrepBody` + a `tool.kind === 'ast_grep'` branch near the existing dispatch ~line 407-431)
- Test: `crates/rupu-cp/web/src/components/transcript/` (a small render/parse unit test if the repo tests components; otherwise a pure helper test for the fallback parser)

**Interfaces:**
- Consumes: `ToolView.structured` + `ToolView.output` + `ToolView.kind === 'ast_grep'` (Task 3).

- [ ] **Step 1: Add the fallback parser as a pure, testable helper**

In `ToolCard.tsx` (or a co-located `astGrep.ts` helper), add a pure function that parses the compact text into grouped matches, used when `structured` is absent:

```ts
// Parses `path:line:col: text` lines into { file, matches: [{line, col, text}] }[].
export function parseAstGrepText(output: string): { file: string; matches: { line: number; col: number; text: string }[] }[] {
  const byFile = new Map<string, { line: number; col: number; text: string }[]>();
  for (const raw of output.split("\n")) {
    if (!raw.trim()) continue;
    const m = raw.match(/^(.*?):(\d+):(\d+): (.*)$/);
    if (!m) continue;
    const [, file, line, col, text] = m;
    if (!byFile.has(file)) byFile.set(file, []);
    byFile.get(file)!.push({ line: Number(line), col: Number(col), text });
  }
  return [...byFile.entries()].map(([file, matches]) => ({ file, matches }));
}
```

Add a unit test for it (empty input → `[]`; two files → grouped; a non-matching line → skipped).

- [ ] **Step 2: Run the helper test (red→green as you implement)**

Run: `cd crates/rupu-cp/web && npx vitest run astGrep` (or wherever the test lands).
Expected: FAIL first (function absent), PASS after Step 1.

- [ ] **Step 3: Implement `AstGrepBody` and wire the dispatch branch**

Add the component (reuse the inline `useState` + lucide `ChevronRight/ChevronDown` disclosure pattern from `Turn.tsx`; mono styling like `GrepBody`):

```tsx
function HighlightedMatch({ text, single, multi }: { text: string; single: Record<string, any>; multi: Record<string, any> }) {
  // Collect [start,end,name] spans from metavar textOffsets, sort, render.
  const chars = Array.from(text); // codepoint array aligns with Rust char offsets
  const spans: { start: number; end: number; name: string }[] = [];
  for (const [name, b] of Object.entries(single ?? {})) {
    if (b?.textOffset) spans.push({ start: b.textOffset.start, end: b.textOffset.end, name });
  }
  for (const [name, arr] of Object.entries(multi ?? {})) {
    for (const b of (arr as any[]) ?? []) if (b?.textOffset) spans.push({ start: b.textOffset.start, end: b.textOffset.end, name });
  }
  spans.sort((a, z) => a.start - z.start);
  const out: React.ReactNode[] = [];
  let cursor = 0;
  spans.forEach((s, i) => {
    if (s.start < cursor || s.start > chars.length) return; // skip overlaps/out-of-range
    if (s.start > cursor) out.push(<span key={`t${i}`}>{chars.slice(cursor, s.start).join("")}</span>);
    out.push(<span key={`m${i}`} className="rounded bg-amber-100 text-amber-900 px-0.5" title={`$${s.name}`}>{chars.slice(s.start, s.end).join("")}</span>);
    cursor = s.end;
  });
  if (cursor < chars.length) out.push(<span key="tail">{chars.slice(cursor).join("")}</span>);
  return <code className="whitespace-pre-wrap">{out}</code>;
}

function AstGrepBody({ tool }: { tool: ToolView }) {
  const s = tool.structured as any | undefined;
  if (s && Array.isArray(s.matches)) {
    // Group structured matches by file.
    const byFile = new Map<string, any[]>();
    for (const m of s.matches) {
      if (!byFile.has(m.file)) byFile.set(m.file, []);
      byFile.get(m.file)!.push(m);
    }
    return (
      <div className="text-xs">
        <div className="mb-1 text-slate-500">
          {s.matchCount} match{s.matchCount === 1 ? "" : "es"} in {s.fileCount} file{s.fileCount === 1 ? "" : "s"}
          {s.pattern ? <span className="ml-2 rounded bg-slate-100 px-1 font-mono">{s.pattern}</span> : null}
          {s.lang ? <span className="ml-1 rounded bg-slate-100 px-1 font-mono">{s.lang}</span> : null}
          {s.truncated ? <span className="ml-2 text-amber-600">showing first {s.matches.length} of {s.matchCount}</span> : null}
        </div>
        {[...byFile.entries()].map(([file, ms]) => (
          <FileGroup key={file} file={file} count={ms.length}>
            {ms.map((m, i) => (
              <div key={i} className="border-l-2 border-slate-200 pl-2 py-1">
                <div className="text-slate-400">{file}:{m.range?.startLine}:{m.range?.startCol}</div>
                <div className="font-mono"><HighlightedMatch text={m.text ?? ""} single={m.metaVars?.single} multi={m.metaVars?.multi} /></div>
                <MetaVarTable single={m.metaVars?.single} multi={m.metaVars?.multi} />
              </div>
            ))}
          </FileGroup>
        ))}
      </div>
    );
  }
  // Fallback: parse the compact text.
  const groups = parseAstGrepText(tool.output ?? "");
  const count = groups.reduce((n, g) => n + g.matches.length, 0);
  return (
    <div className="text-xs">
      <div className="mb-1 text-slate-500">{count} match{count === 1 ? "" : "es"} in {groups.length} file{groups.length === 1 ? "" : "s"}</div>
      {groups.map((g) => (
        <FileGroup key={g.file} file={g.file} count={g.matches.length}>
          {g.matches.map((m, i) => (
            <div key={i} className="font-mono whitespace-pre"><span className="text-slate-400">{g.file}:{m.line}:{m.col}: </span>{m.text}</div>
          ))}
        </FileGroup>
      ))}
    </div>
  );
}
```

Add small `FileGroup` (collapsible, using the `Turn.tsx` chevron pattern) and `MetaVarTable` (rows `$name = value`, iterating `single` then `multi`) helpers in the same file. Then add the dispatch branch beside the others (~line 424-431):

```tsx
        {tool.kind === 'ast_grep' && <AstGrepBody tool={tool} />}
```

- [ ] **Step 4: Build the web app clean**

Run: `cd crates/rupu-cp/web && npm run build`
Expected: build succeeds (no TS/rollup errors).
Run: `npx vitest run` (all web tests green).

- [ ] **Step 5: Visual verification (required before merge)**

The renderer's visual correctness cannot be asserted by unit tests. Before the branch merges, verify in a browser: run the CP against a run whose transcript contains an `ast_grep` call (or a fixture transcript), open `/transcript`, and confirm: grouped-by-file, collapsible, count badge, bindings table, and metavar highlighting render correctly in the light theme. Report this as `DONE_WITH_CONCERNS` noting "visual check pending human/browser confirmation" if you cannot drive a browser — the controller routes the visual check (matt or computer-use), it does not block the code review.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/components/transcript/
git commit -m "feat(cp): rich ast_grep transcript renderer (group-by-file, metavars, highlight)"
```

---

## Self-Review

**1. Spec coverage:**
- SP1 `structured` on `ToolOutput` + `Event::ToolResult`, threaded via runner, backend passthrough → Task 1. ✓
- `ast_grep` v1 payload (schema, 1-based ranges, rel path, metavars single/multi, textOffset char indices, cap 200 + truncated, stdout unchanged) → Task 2. ✓
- CP wire type + view model (classify ast_grep, ToolView.structured, summarizeInput) → Task 3. ✓
- `AstGrepBody` (structured render: group-by-file, count badge, bindings table, inline highlight; text fallback) → Task 4. ✓
- Testing: Rust round-trip + payload + stdout-unchanged + error-None; TS classify/structured + fallback parser; build clean + visual-check gate → Tasks 1-4. ✓
- Out of scope (other tools, file parser/clickable source, AST viz) → correctly absent; tracked in TODO.md. ✓

**2. Placeholder scan:** No TBD/TODO; concrete code in every code step. All identifiers are plain ASCII.

**3. Type consistency:** `structured: Option<serde_json::Value>` identical across `ToolOutput` (tool.rs), `Event::ToolResult` (event.rs), runner copy (`out.structured.clone()`); TS `structured?: unknown` on wire type + `ToolView`; renderer reads `tool.structured`. Payload keys (`tool`/`pattern`/`lang`/`matchCount`/`fileCount`/`truncated`/`matches`/`file`/`range`/`text`/`metaVars`/`single`/`multi`/`textOffset`) match between Task 2 (Rust build), Task 3 (test), and Task 4 (render). ✓
