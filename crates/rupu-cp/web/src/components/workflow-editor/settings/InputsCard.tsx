// InputsCard — Inputs authoring card for the Settings inspector (Task 5),
// rendered only when `workflowEditorUi === 'next'` (WorkflowSettingsForm
// decides that; this component doesn't gate itself).
//
// An add/remove list of `InputModel` rows (name / type / required / default /
// enum / description).
//
// Rows are held in LOCAL state as `{ id, model }`, keyed by a client-side
// `id` that is independent of `model.name`. This matters because `rest.inputs`
// is a name-keyed map (see `writeInputs`): two rows both named `''` (e.g.
// right after clicking "+ input" twice, before either is named) would
// collide into a single map entry if the row list were recomputed from
// `readInputs(rest)` on every render — the second row silently overwrites
// the first. Keeping rows in local state means duplicate/blank names can
// coexist in the editor; only at WRITE time do we fold them into the
// name-keyed map, and blank-named rows are omitted from that map entirely
// (rather than emitted under an empty-string key) so they stay editable in
// the UI without polluting `rest.inputs`.
//
// `rows` is seeded from `readInputs(rest)` on mount and re-seeded ONLY when
// `rest.inputs` changes from something this component didn't itself just
// write (tracked via `lastWrittenInputsRef`) — e.g. a different workflow is
// loaded into the form. This avoids clobbering in-progress edits (like a
// still-blank row) on our own echoed-back `onRest` round-trip.

import { useEffect, useRef, useState } from 'react';
import { readInputs, writeInputs, type InputModel, type InputType } from '../../../lib/workflowMeta';
import { Button } from '../../ui/Button';

interface InputsCardProps {
  rest: Record<string, unknown>;
  onRest: (rest: Record<string, unknown>) => void;
}

interface Row {
  id: string;
  model: InputModel;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';
const checkLabelCls = 'flex items-center gap-2 text-lead text-ink';

const TYPE_OPTIONS: InputType[] = ['string', 'int', 'bool'];

/** Parse the "default" text field into the value `writeInputs` should emit,
 *  coerced by the input's declared `type`. Empty text → no default at all.
 *  A non-numeric `int` default (or any `bool` default other than the literal
 *  strings "true"/"false") is kept as the raw string rather than dropped —
 *  the operator is mid-typing, not necessarily wrong. */
function parseDefault(type: InputType, raw: string): unknown {
  if (raw === '') return undefined;
  if (type === 'int') {
    const n = Number(raw);
    return Number.isNaN(n) ? raw : n;
  }
  if (type === 'bool') {
    if (raw === 'true') return true;
    if (raw === 'false') return false;
    return raw;
  }
  return raw;
}

/** Render a stored default value back into the text field's string. */
function defaultToText(v: unknown): string {
  if (v === undefined) return '';
  if (typeof v === 'string') return v;
  return String(v);
}

export default function InputsCard({ rest, onRest }: InputsCardProps) {
  // Deterministic id generator (no Date.now()/Math.random()) — stable React
  // keys and row addressing independent of `name`.
  const idCounter = useRef(0);
  function nextId(): string {
    idCounter.current += 1;
    return `input-row-${idCounter.current}`;
  }

  const [rows, setRows] = useState<Row[]>(() => readInputs(rest).map((model) => ({ id: nextId(), model })));
  // Tracks the `rest.inputs` value this component itself last produced (via
  // `commit` below), so the reconcile effect can tell "the parent echoed our
  // own write back" (skip — would clobber blank/duplicate-name rows still
  // being edited) apart from "a genuinely new `rest` arrived" (reseed).
  const lastWrittenInputsRef = useRef<unknown>(rest.inputs);

  useEffect(() => {
    if (rest.inputs !== lastWrittenInputsRef.current) {
      lastWrittenInputsRef.current = rest.inputs;
      setRows(readInputs(rest).map((model) => ({ id: nextId(), model })));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rest.inputs]);

  // Pending "add enum value" text per row index — local UI state, never part
  // of the emitted meta (only a committed chip, via addEnumValue, is).
  const [pendingEnum, setPendingEnum] = useState<Record<number, string>>({});

  /** Commit a new row list: update local editing state, then fold into the
   *  name-keyed map and emit. Blank-named rows are dropped before
   *  `writeInputs` so they never emit an empty-string key — they simply stay
   *  visible/editable in `rows` until named. */
  function commit(next: Row[]): void {
    setRows(next);
    const named = next.map((r) => r.model).filter((m) => m.name.trim() !== '');
    const nextRest = writeInputs(rest, named);
    lastWrittenInputsRef.current = nextRest.inputs;
    onRest(nextRest);
  }
  function updateRow(i: number, p: Partial<InputModel>): void {
    commit(rows.map((r, j) => (j === i ? { ...r, model: { ...r.model, ...p } } : r)));
  }
  function addRow(): void {
    commit([...rows, { id: nextId(), model: { name: '', type: 'string', required: false, enumValues: [] } }]);
  }
  function removeRow(i: number): void {
    commit(rows.filter((_, j) => j !== i));
  }
  function addEnumValue(i: number): void {
    const v = (pendingEnum[i] ?? '').trim();
    if (v === '') return;
    const row = rows[i].model;
    if (!row.enumValues.includes(v)) {
      updateRow(i, { enumValues: [...row.enumValues, v] });
    }
    setPendingEnum((prev) => ({ ...prev, [i]: '' }));
  }
  function removeEnumValue(i: number, v: string): void {
    updateRow(i, { enumValues: rows[i].model.enumValues.filter((x) => x !== v) });
  }

  return (
    <div className="wfx-card" data-testid="inputs-card">
      <div className="wfx-card-h">Inputs</div>
      <div className="wfx-card-b">
        {rows.length === 0 && <p className="text-ui text-ink-mute">No inputs declared.</p>}

        {rows.map(({ id, model: row }, i) => (
          <div key={id} className="wfx-inputrow">
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={row.name}
                onChange={(e) => updateRow(i, { name: e.target.value })}
                aria-label={`Input ${i + 1} name`}
                placeholder="name"
                className={`${fieldCls} font-mono`}
              />
              <Button
                variant="danger-outline"
                onClick={() => removeRow(i)}
                aria-label={`Remove input ${i + 1}`}
                className="shrink-0 px-2.5"
              >
                Remove
              </Button>
            </div>

            <div className="wfx-row-two">
              <label className="block">
                <span className={labelCls}>Type</span>
                <select
                  value={row.type}
                  onChange={(e) => updateRow(i, { type: e.target.value as InputType })}
                  aria-label={`Input ${i + 1} type`}
                  className={fieldCls}
                >
                  {TYPE_OPTIONS.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </label>
              <label className="block">
                <span className={labelCls}>Default</span>
                <input
                  type="text"
                  value={defaultToText(row.default)}
                  onChange={(e) => updateRow(i, { default: parseDefault(row.type, e.target.value) })}
                  aria-label={`Input ${i + 1} default`}
                  className={fieldCls}
                />
              </label>
            </div>

            <label className={checkLabelCls}>
              <input
                type="checkbox"
                checked={row.required}
                onChange={(e) => updateRow(i, { required: e.target.checked })}
                aria-label={`Input ${i + 1} required`}
              />
              Required
            </label>

            <label className="block">
              <span className={labelCls}>Description</span>
              <textarea
                value={row.description ?? ''}
                onChange={(e) => updateRow(i, { description: e.target.value === '' ? undefined : e.target.value })}
                aria-label={`Input ${i + 1} description`}
                rows={4}
                className={`${fieldCls} resize-y`}
              />
            </label>

            <div>
              <span className={labelCls}>Enum values (optional)</span>
              {row.enumValues.length > 0 && (
                <div className="wfx-chiprow mb-1.5">
                  {row.enumValues.map((v) => (
                    <span key={v} className="wfx-chip">
                      {v}
                      <button
                        type="button"
                        onClick={() => removeEnumValue(i, v)}
                        aria-label={`Remove enum value ${v} from input ${i + 1}`}
                      >
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              )}
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={pendingEnum[i] ?? ''}
                  onChange={(e) => setPendingEnum((prev) => ({ ...prev, [i]: e.target.value }))}
                  aria-label={`Input ${i + 1} enum value`}
                  placeholder="add enum value"
                  className={fieldCls}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      addEnumValue(i);
                    }
                  }}
                />
                <Button
                  variant="secondary"
                  size="sm"
                  aria-label={`Add enum value to input ${i + 1}`}
                  className="shrink-0"
                  onClick={() => addEnumValue(i)}
                >
                  Add
                </Button>
              </div>
            </div>
          </div>
        ))}

        <button type="button" onClick={addRow} className="wfx-addbtn">
          + input
        </button>
      </div>
    </div>
  );
}
