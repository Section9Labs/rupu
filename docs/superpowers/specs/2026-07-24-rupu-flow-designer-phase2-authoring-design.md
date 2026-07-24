# Flow Designer (`next`) — Phase 2: authoring gaps (design)

**Date:** 2026-07-24
**Status:** Design; pending operator sign-off on the one UX fork (§2).
**Parent:** `2026-07-24-rupu-flow-designer-single-source-design.md` (§8 deferred these). Findings: `2026-07-24-rupu-flow-designer-correctness-findings.md` (§P2, and rejection-risks F.4/F.5).
**Scope:** `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` + `settings/AutoflowCard.tsx` + `lib/workflowGraph.ts` (passthrough). Behind `[cp].workflow_editor_ui = 'next'`; classic untouched. No backend change.

Phase 1 made the editor's *structure* trustworthy. Phase 2 closes the **authoring gaps** the contract audit found — fields you can't edit, inputs that accept invalid values, and editors that silently corrupt or narrow data. Each is independent and low-risk.

## 1. The items (with clear answers)

| # | Gap | Fix |
|---|---|---|
| A | **Severity is free text** (`panel.gate.until_no_findings_at_severity_or_above`) — a typo like `midium` 400s. | Replace the text `<input>` with a `<select>` of `low / medium / high / critical` (the backend `Severity` enum). Keep the current value as an extra option if it's off-list (so an odd hand-authored value round-trips). |
| B | **`max_parallel` has no min guard** — `0`/negative 400s (`InvalidMaxParallel`). | `min={1}` on the number inputs (panel + parallel); a value `< 1` shows an inline "must be ≥ 1" and is flagged by `validateGraph` so the save gate/badge catch it. |
| C | **`Approval.notify` has no UI** — parsed + preserved but invisible; a gate's notify hooks can only be seen in the YAML pane. | Add a **Notify** list editor to `GateFields`: each entry is a connector action (`action` name + `with:` params), reusing the same controls as an `action` step. Add / remove rows; empty list → omit `notify`. |
| D | **`on_reject` rows assume agent/prompt shape** — an action-shaped cleanup step (`action`+`with`) corrupts on edit (adds `agent`/`prompt` → `ActionMutuallyExclusive` 400). | Make each `on_reject` row **kind-aware**: detect whether the row is agent-shaped or action-shaped and render the matching fields; a small kind toggle per row lets you author either. Editing an action row never injects agent/prompt. |
| E | **`Autoflow.source` / `Autoflow.priority` have no control** — preserved, not editable. | Add a `source` text input and a `priority` number input to `AutoflowCard` (both optional; priority defaults to 0 = omitted). |
| F | **`Panel` / `Branch` / `Approval` sub-schemas have no passthrough bag** — an unmodeled future key nested under those is dropped on first edit. | Give each a `_rest` catch-all in its parser (`parsePanel` / branch / approval), merged back on emit with the same clobber-safe `if (!(k in o))` rule Step already uses. Harmless today; future-proofs the round-trip. |

## 2. The one fork — the `with:` value editor

**Problem.** Today `ActionFields` (and the new Notify editor) render one text `<input>` per `with:` param and always store a **string** (`patchWith(key, value: string)`). A hand-authored numeric/boolean/list value (`count: 3`, `dry_run: true`, `paths: [a, b]`) shows **blank** and, if edited, narrows to a string. But `with:` values may also be **runtime templates** (`{{ inputs.count }}`) — the backend renders them at run time and does not type-check values at parse time — so a strict per-type input would wrongly reject a valid template.

Two viable approaches (I recommend **B**, the smart-parse, precisely because of the template reality):

- **Option A — Schema-typed inputs + expression escape.** Read each param's declared type from the tool's `input_schema` and render the matching control (text / number / checkbox / JSON textarea), plus a per-field "use expression" toggle that swaps to a text box for a `{{…}}` template. Most polished; most code; the toggle is the price of the template escape hatch.
- **Option B — One smart-parse text field.** Keep a single text input per param, but on save parse the text: a JSON literal (`3`, `true`, `["a","b"]`, `{"k":1}`) is stored as its typed value; anything else — a template `{{…}}` or a plain word — stays a string. Round-trips typed values, no quotes needed for plain strings, templates just work (they aren't valid JSON so they stay strings). One small ambiguity: a literal string that looks like JSON (`"true"`, `"3"`) becomes typed; rare for a param, and the field shows the parsed value back so it's visible.

Both fix the "shows blank / narrows to string" bug. A is the higher-fidelity authoring experience; B is ~1/4 the code and handles the template case more naturally. **Recommendation: B**, with A tracked as a later polish if the type-blindness ever bites.

## 3. Phasing within Phase 2
- **2.1 — quick wins:** A (severity select), B (max_parallel guard), E (autoflow inputs), F (passthrough bags). Small, independent, each its own task.
- **2.2 — the value editor:** the chosen `with:` approach (§2), shared by `ActionFields` and the new Notify editor.
- **2.3 — the list editors:** C (notify) and D (on_reject kind-awareness), which depend on 2.2's value control.

## 4. Constraints & testing
- `next` path only; classic untouched; behind the flag. Tokens only; no backend change; no new dep.
- Every changed editor gets a round-trip test: author the field → serialize → `parse` accepts it → re-parse restores it (the same net Phase 1 established). Severity select emits a valid enum; `max_parallel<1` is flagged; a numeric/bool/template `with:` value round-trips without narrowing; an action-shaped `on_reject` row survives an edit without gaining agent/prompt; notify entries round-trip; passthrough keys under panel/branch/approval survive an edit.
- Operator gate: matt authors each in the running app (light + dark).
