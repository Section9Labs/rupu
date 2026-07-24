# Flow Designer Phase 2 — Authoring Gaps — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the `next` Flow Designer's authoring gaps — fields you can't edit, inputs that accept invalid values, and editors that silently corrupt or narrow data.

**Architecture:** Localized additions to the step/settings forms plus small parser/emitter changes. A shared `parseWithValue`/`formatWithValue` (smart-parse: JSON literals typed, templates/plain strings kept) backs the connector-param and notify editors. Passthrough bags future-proof the nested sub-schemas.

**Tech Stack:** React 18 + TypeScript, Vitest + Testing Library, `js-yaml`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-24-rupu-flow-designer-phase2-authoring-design.md`. Operator chose **smart-parse** for the `with:` editor (§2 option B).
- **All work in `crates/rupu-cp/web/`.** Test: `npx vitest run <path>`; typecheck: `npx tsc -b --noEmit`.
- **`next` path only; classic untouched; behind `[cp].workflow_editor_ui = 'next'`.** Tokens only; no backend change; no new npm dependency.
- Every changed editor gets a **round-trip test**: author → serialize → re-parse restores it, no narrowing/loss.
- Branch is `flow-designer-phase2`, stacked on `flow-designer-single-source` (Phase 1). Base of this task series is the Phase 2 spec commit.

## File Structure

| File | Change |
|---|---|
| `src/lib/withValue.ts` | **Create.** `parseWithValue(text): unknown` + `formatWithValue(v): string` (smart-parse). |
| `src/lib/withValue.test.ts` | **Create.** Typed literals, templates, plain strings, round-trip. |
| `src/components/workflow-editor/StepForm.tsx` | Severity `<select>`; `max_parallel` min-guard; `ActionFields` uses `parseWithValue`; new Notify editor in `GateFields`; `on_reject` rows kind-aware. |
| `src/components/workflow-editor/settings/AutoflowCard.tsx` | `source` text input + `priority` number input. |
| `src/lib/workflowGraph.ts` | Passthrough `_rest` for `Panel`/`Branch`/`Approval`; `validateGraph` flags `max_parallel < 1` and empty gate fields. |
| Respective `.test.tsx`/`.test.ts` | Tests per task. |

---

### Task 1: Simple missing controls & constraints

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — gate severity input (~576), `max_parallel` inputs (panel ~558, parallel arm)
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/settings/AutoflowCard.tsx` — add source/priority controls
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `validateGraph` gains `max_parallel < 1` + incomplete-gate checks
- Test: the three files' test siblings

**Interfaces:**
- Produces: no new exports. Severity becomes a `<select>`; `max_parallel < 1` is a `validateGraph` problem; `Autoflow.source`/`priority` are editable.

- [ ] **Step 1: Write the failing tests**

In `StepForm.test.tsx`:

```ts
it('gate severity is a select of the four severities and keeps an off-list value', () => {
  // render StepForm for a panel with gate enabled; assert a <select aria-label="Until no findings at severity or above">
  // with options low/medium/high/critical; set to 'high' → payload gate.until_no_findings_at_severity_or_above==='high'.
});
```

In `workflowGraph.test.ts`:

```ts
it('validateGraph flags max_parallel below 1', () => {
  const g = yamlToGraph({ name: 'w', steps: [{ id: 'p', parallel: [{ id: 's', agent: 'a', prompt: 'x' }], max_parallel: 0 }] });
  const problems = validateGraph(g);
  expect(Object.values(problems).flat().some((m) => /max.?parallel/i.test(m) && /1/.test(m))).toBe(true);
});

it('validateGraph flags an enabled gate with no severity', () => {
  const g = yamlToGraph({ name: 'w', steps: [{ id: 'p', panel: { panelists: ['r'], subject: 's', gate: {} } }] });
  expect(Object.values(validateGraph(g)).flat().some((m) => /gate/i.test(m))).toBe(true);
});
```

In `AutoflowCard.test.tsx` (read the file's render helper first):

```ts
it('renders source and priority controls bound to the model', () => {
  // render AutoflowCard with a model; assert inputs aria-labeled "Autoflow source" and "Autoflow priority" exist,
  // and editing them flows through commit() to model.source / model.priority.
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx src/lib/workflowGraph.test.ts src/components/workflow-editor/settings/AutoflowCard.test.tsx`
Expected: FAIL — severity is a text input; no max_parallel/gate problems; no source/priority controls.

- [ ] **Step 3: Implement**

Severity `<select>` — replace the gate severity `<input type="text">` (StepForm.tsx ~576-586) with:

```tsx
            <select
              value={panel.gate.until_no_findings_at_severity_or_above ?? ''}
              onChange={(e) =>
                patchGate({ until_no_findings_at_severity_or_above: e.target.value === '' ? undefined : e.target.value })
              }
              aria-label="Until no findings at severity or above"
              className={fieldCls}
            >
              <option value="">(choose severity)</option>
              {['low', 'medium', 'high', 'critical'].map((s) => (
                <option key={s} value={s}>{s}</option>
              ))}
              {/* keep an off-list value authored by hand so it still round-trips */}
              {panel.gate.until_no_findings_at_severity_or_above &&
                !['low', 'medium', 'high', 'critical'].includes(panel.gate.until_no_findings_at_severity_or_above) && (
                  <option value={panel.gate.until_no_findings_at_severity_or_above}>
                    {panel.gate.until_no_findings_at_severity_or_above}
                  </option>
                )}
            </select>
```

`max_parallel` guard — add `min={1}` to the panel `max_parallel` input (StepForm.tsx ~558) and the parallel arm's `max_parallel` input (find it in `ParallelFields`): `<input type="number" min={1} ... />`.

In `validateGraph` (`workflowGraph.ts`), add per-node checks (in the same style as the existing agent/prompt/panel checks): if `d.max_parallel !== undefined && d.max_parallel < 1` → `add(d.id, '`max_parallel` must be at least 1')`; if `d.kind === 'panel' && d.panel?.gate` and any of `until_no_findings_at_severity_or_above`/`fix_with`/`max_iterations` is missing → `add(d.id, 'gate needs a severity, a fix agent, and max iterations')`.

Autoflow controls — in `AutoflowCard.tsx`, add (near the other fields, using the file's `commit({ ...model, ... })` pattern and `fieldCls`/`labelCls`):

```tsx
      <label className="block">
        <span className={labelCls}>Source (optional)</span>
        <input type="text" value={model.source ?? ''}
          onChange={(e) => commit({ ...model, source: e.target.value === '' ? undefined : e.target.value })}
          aria-label="Autoflow source" className={fieldCls} />
      </label>
      <label className="block">
        <span className={labelCls}>Priority</span>
        <input type="number" value={model.priority ?? ''}
          onChange={(e) => commit({ ...model, priority: e.target.value === '' ? 0 : Number(e.target.value) })}
          aria-label="Autoflow priority" className={fieldCls} />
      </label>
```

(Verify `AutoflowModel` has `source?: string` and `priority?: number` — the audit confirmed `readAutoflow`/`writeAutoflow` already handle them; if the model type is missing them, add the fields to the type in `lib/workflowMeta.ts`.)

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/ src/lib/`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/StepForm.test.tsx src/components/workflow-editor/settings/AutoflowCard.tsx src/components/workflow-editor/settings/AutoflowCard.test.tsx src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "feat(cp): severity select, max_parallel guard, autoflow source/priority"
```

---

### Task 2: Smart-parse `with:` value editor

**Files:**
- Create: `crates/rupu-cp/web/src/lib/withValue.ts`
- Create: `crates/rupu-cp/web/src/lib/withValue.test.ts`
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — `ActionFields` (`patchWith` + value render)
- Test: `StepForm.test.tsx`

**Interfaces:**
- Produces: `export function parseWithValue(text: string): unknown` and `export function formatWithValue(v: unknown): string`. Task 4 (notify) consumes both.

- [ ] **Step 1: Write the failing test**

Create `src/lib/withValue.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { parseWithValue, formatWithValue } from './withValue';

describe('parseWithValue', () => {
  it('parses JSON literals to typed values', () => {
    expect(parseWithValue('3')).toBe(3);
    expect(parseWithValue('true')).toBe(true);
    expect(parseWithValue('["a","b"]')).toEqual(['a', 'b']);
    expect(parseWithValue('{"k":1}')).toEqual({ k: 1 });
  });
  it('keeps templates and plain strings as strings', () => {
    expect(parseWithValue('{{ inputs.x }}')).toBe('{{ inputs.x }}');
    expect(parseWithValue('hello world')).toBe('hello world');
  });
  it('empty text signals deletion (undefined)', () => {
    expect(parseWithValue('')).toBeUndefined();
    expect(parseWithValue('   ')).toBeUndefined();
  });
  it('a JSON-quoted string stays a string', () => {
    expect(parseWithValue('"true"')).toBe('true');
  });
});

describe('formatWithValue round-trips parseWithValue', () => {
  it.each(['3', 'true', '["a","b"]', '{"k":1}', '{{ inputs.x }}', 'hello world'])('%s', (text) => {
    const v = parseWithValue(text);
    if (v === undefined) return;
    expect(parseWithValue(formatWithValue(v))).toEqual(v);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/withValue.test.ts`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement**

Create `src/lib/withValue.ts`:

```ts
// Smart-parse for a connector `with:` param field. Connector values may be
// literals (numbers, bools, lists) OR runtime minijinja templates ({{ ... }}),
// which the backend renders at run time and does NOT type-check at parse time.
// So: a JSON literal is stored as its typed value; anything else — a template
// or a plain word — is kept verbatim as a string.

/** Parse a `with:` field's text into its stored value. Empty/whitespace →
 *  `undefined` (the caller deletes the key). A valid JSON document parses to
 *  its typed value (number/bool/null/array/object, and a JSON-quoted string
 *  stays a string); anything else — a `{{ template }}` or a bare word — is
 *  returned verbatim (untrimmed) as a string. */
export function parseWithValue(text: string): unknown {
  if (text.trim() === '') return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** Render a stored `with:` value back into its text field. Strings show
 *  verbatim; everything else shows its JSON text (so `3`/`true`/`["a"]`
 *  round-trip through `parseWithValue`). */
export function formatWithValue(v: unknown): string {
  return typeof v === 'string' ? v : JSON.stringify(v);
}
```

Wire into `ActionFields` (StepForm.tsx). Change `patchWith` to parse, and the value render to format. `patchWith` becomes:

```tsx
  function patchWith(key: string, text: string): void {
    const next = { ...withObj };
    const v = parseWithValue(text);
    if (v === undefined) delete next[key];
    else next[key] = v;
    patch({ with: next });
  }
```

And the per-param `<input>`'s `value` (currently `typeof withObj[key] === 'string' ? ... : ''`) becomes `value={formatWithValue(withObj[key])}` so a numeric/bool/list value displays instead of blank. Import `parseWithValue, formatWithValue` from `../../lib/withValue`.

- [ ] **Step 4: Run to verify pass**

Add a `StepForm.test.tsx` test: authoring `count` = `3` on an action step yields `d.with.count === 3` (number), and rendering an action step with `d.with = { count: 3 }` shows `3` in the field (not blank). Run:
`cd crates/rupu-cp/web && npx vitest run src/lib/withValue.test.ts src/components/workflow-editor/StepForm.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/withValue.ts src/lib/withValue.test.ts src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/StepForm.test.tsx
git commit -m "feat(cp): smart-parse connector with: values (typed literals, templates stay strings)"
```

---

### Task 3: Passthrough bags for Panel / Branch / Approval

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `parsePanel`, the branch parse (in `parseStepData`), the approval parse; and the corresponding emit arms in `nodeToStepObject`
- Test: `crates/rupu-cp/web/src/lib/workflowGraph.test.ts`

**Interfaces:**
- Produces: no new exports. Unmodeled keys nested directly under `panel`/`branch`/`approval` survive a round-trip.

- [ ] **Step 1: Write the failing test**

```ts
describe('nested passthrough', () => {
  it.each([
    ['panel', { name: 'w', steps: [{ id: 'p', panel: { panelists: ['r'], subject: 's', future_key: 42 } }] }, (s: any) => s.panel.future_key],
    ['branch', { name: 'w', steps: [{ id: 'b', branch: { condition: 'x', future_key: 42 } }] }, (s: any) => s.branch.future_key],
    ['approval', { name: 'w', steps: [{ id: 'g', approval: { required: true, future_key: 42 } }] }, (s: any) => s.approval.future_key],
  ])('an unmodeled key under %s survives a round-trip', (_name, input, get) => {
    const g = yamlToGraph(input as Record<string, unknown>);
    const out = graphToWorkflowObject(g) as { obj: Record<string, unknown> };
    expect(get((out.obj.steps as Record<string, unknown>[])[0])).toBe(42);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts -t "nested passthrough"`
Expected: FAIL — `future_key` is dropped.

- [ ] **Step 3: Implement**

For each of `parsePanel`, the branch parse, and the approval parse: after extracting the known fields, collect every OTHER key from the raw object into a `_rest: Record<string, unknown>` stored on the parsed structure (mirror how `parseStepData` builds `raw_passthrough` — read that as the reference pattern). Add a matching optional field to the parsed TS shape (e.g. `PanelData` gains `_rest?: Record<string, unknown>`; branch stores `branchRest`; approval stores `approvalRest` — match the existing field-naming convention in `StepNodeData`).

In `nodeToStepObject`'s panel/branch/approval emit arms: after building the known keys, spread the `_rest` back with the clobber-safe guard `for (const [k, v] of Object.entries(rest)) if (!(k in target)) target[k] = v;` (exactly like the step-level `raw_passthrough` merge at the end of `nodeToStepObject`).

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts`
Expected: PASS. Confirm the pre-existing round-trip suite still passes (no known key accidentally swept into `_rest`).

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "feat(cp): passthrough bags for panel/branch/approval sub-schemas"
```

---

### Task 4: Notify editor in `GateFields`

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — `GateFields` gains a Notify list editor
- Test: `StepForm.test.tsx`

**Interfaces:**
- Consumes: `parseWithValue`/`formatWithValue` (Task 2). Reuses the `d.approvalNotify` field (already parsed as `Record<string, unknown>[]`).

- [ ] **Step 1: Write the failing test**

```ts
it('a gate notify entry (action + with) can be added and round-trips', () => {
  // render GateFields (a StepForm for an approval_gate node); click "Add notification";
  // set action name + a with param; assert the emitted d.approvalNotify contains
  // { action: '<name>', with: { <key>: <typed value> } } and that it serializes under approval.notify.
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx -t notify`
Expected: FAIL — no notify UI exists.

- [ ] **Step 3: Implement**

In `GateFields`, add a **Notify** section modeled on the existing `On reject` list (`approvalOnReject`) and the `ActionFields` param editor: read `d.approvalNotify ?? []`; render each entry with an action-name input and a `with:` param editor (reuse the same per-key text inputs + `parseWithValue`/`formatWithValue` as `ActionFields` — extract a small shared `WithParamsEditor` sub-component if that avoids duplication, otherwise inline mirroring `ActionFields`); an "Add notification" button appends `{}`; a remove button per row; an empty list omits `notify` (already handled by the emit guard `d.approvalNotify && d.approvalNotify.length > 0`). Patch through `patch({ approvalNotify: next })`.

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/StepForm.test.tsx
git commit -m "feat(cp): notify editor for approval gates"
```

---

### Task 5: `on_reject` rows become kind-aware

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — `GateFields` on_reject rows
- Test: `StepForm.test.tsx`

**Interfaces:**
- Consumes: `parseWithValue`/`formatWithValue` (Task 2). Fixes rejection-risk F.4 (an action-shaped on_reject row gaining agent/prompt on edit).

- [ ] **Step 1: Write the failing test**

```ts
it('editing an action-shaped on_reject row never injects agent/prompt (F.4)', () => {
  // render GateFields for a gate whose approvalOnReject[0] = { id: 'cleanup', action: 'scm.prs.comment', with: { body: 'x' } };
  // the row should render action fields (not agent/prompt); edit its id; assert the row still has action/with and NO agent/prompt key.
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx -t "on_reject"`
Expected: FAIL — the row renders agent/prompt and an id edit adds them.

- [ ] **Step 3: Implement**

Each `on_reject` row: detect its shape — action-shaped if the row object has an `action` key, else agent-shaped. Render a small kind toggle (agent | action) per row that reflects the detected shape and lets the author switch. For an **agent** row show the existing id/agent/prompt fields; for an **action** row show id + action-name input + a `with:` param editor (same `parseWithValue`/`formatWithValue` controls as Task 2/4). `updateReject` must merge only the fields for that row's shape — when the row is action-shaped, never write `agent`/`prompt` (and vice versa). Switching the toggle clears the other shape's fields on that row.

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/ && npx vitest run`
Expected: PASS (full suite).

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/StepForm.test.tsx
git commit -m "fix(cp): on_reject rows are kind-aware (action-shaped rows no longer corrupt on edit)"
```

---

## Operator gate (before merge)
matt authors each in the running app (light + dark): pick a gate severity from the dropdown; type `max_parallel` 0 → inline warning; set an action param to a number and to a `{{ template }}` → both persist correctly (number shows as a number, not blank); add a gate notification; add an action-shaped on-reject cleanup step and edit it without it breaking; set an autoflow source/priority.

## Self-review notes
- Spec items A-F all map: A/B/E → Task 1; the `with:` fork (smart-parse) → Task 2; F → Task 3; C → Task 4; D → Task 5.
- Tasks 4 and 5 depend on Task 2's `parseWithValue`/`formatWithValue`; ordered accordingly.
