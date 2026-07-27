// LoopForm — the "Group into loop" / edit-loop panel (Phase 3 Task 5, spec §5).
//
// A small floating card (mirrors the "graph paused" chip / connection-error
// banner's positioning idiom already used in WorkflowEditorGraph/WorkflowEditor
// — an absolutely-positioned overlay with border/shadow/panel tokens, not a new
// modal primitive). Create mode seeds `memberIds` from the current multi-
// selection; edit mode seeds it from the loop's existing `nodes` and offers a
// per-node checkbox list to add/remove members. Delete removes the whole group
// (member steps are untouched — `removeLoop` in workflowGraph.ts).

import { useMemo, useState } from 'react';
import { Button } from '../ui/Button';
import type { LoopDraft, WorkflowLoop } from '../../lib/workflowGraph';

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

export interface LoopFormProps {
  /** `null` for a brand-new loop; the loop being edited otherwise. */
  editing: WorkflowLoop | null;
  /** Every candidate member id, in canvas node order — powers the member
   *  checkbox list (create mode pre-checks exactly the multi-selection that
   *  triggered the action; edit mode pre-checks the loop's current `nodes`). */
  candidateIds: string[];
  /** Ids pre-checked when the form opens (the triggering selection, or the
   *  loop's current membership). */
  initialMemberIds: string[];
  /** Every OTHER loop's name, for the uniqueness check (excludes `editing`'s
   *  own name, which is allowed to resubmit unchanged). */
  otherLoopNames: string[];
  onSubmit: (draft: LoopDraft, memberIds: string[]) => void;
  onDelete?: () => void;
  onCancel: () => void;
}

export default function LoopForm({
  editing,
  candidateIds,
  initialMemberIds,
  otherLoopNames,
  onSubmit,
  onDelete,
  onCancel,
}: LoopFormProps) {
  const [name, setName] = useState(editing?.name ?? '');
  const [until, setUntil] = useState(editing?.until ?? '');
  const [maxIterations, setMaxIterations] = useState(editing?.maxIterations ?? 5);
  const [onMax, setOnMax] = useState<'fail' | 'proceed'>(editing?.onMax ?? 'fail');
  const [memberIds, setMemberIds] = useState<Set<string>>(new Set(initialMemberIds));

  const toggleMember = (id: string): void => {
    setMemberIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const trimmedName = name.trim();
  const nameTaken = otherLoopNames.includes(trimmedName);
  const canSubmit =
    trimmedName.length > 0 && !nameTaken && memberIds.size >= 2 && until.trim().length > 0 && maxIterations >= 1;

  const memberCount = useMemo(() => memberIds.size, [memberIds]);

  return (
    <div
      role="dialog"
      aria-label={editing ? `Edit loop ${editing.name}` : 'Group into loop'}
      className="absolute right-3 top-16 z-30 w-72 rounded-lg border border-border bg-panel p-3 shadow-card"
    >
      <div className="mb-2 flex items-center justify-between">
        <h3 className="text-ui font-semibold text-ink">{editing ? 'Edit loop' : 'Group into loop'}</h3>
        <button type="button" onClick={onCancel} aria-label="Cancel" className="text-ink-mute hover:text-ink">
          ✕
        </button>
      </div>

      <div className="flex flex-col gap-2.5">
        <label>
          <span className={labelCls}>Name</span>
          <input
            className={fieldCls}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="refine"
            aria-label="Loop name"
          />
          {nameTaken && <p className="mt-1 text-note text-err">a loop named "{trimmedName}" already exists</p>}
        </label>

        <label>
          <span className={labelCls}>Until</span>
          <input
            className={fieldCls}
            value={until}
            onChange={(e) => setUntil(e.target.value)}
            placeholder="{{ steps.critique.approved }}"
            aria-label="Until condition"
          />
        </label>

        <div className="flex gap-2">
          <label className="flex-1">
            <span className={labelCls}>Max iterations</span>
            <input
              type="number"
              min={1}
              className={fieldCls}
              value={maxIterations}
              onChange={(e) => setMaxIterations(Math.max(1, Number(e.target.value) || 1))}
              aria-label="Max iterations"
            />
          </label>
          <label className="flex-1">
            <span className={labelCls}>On max</span>
            <select
              className={fieldCls}
              value={onMax}
              onChange={(e) => setOnMax(e.target.value === 'proceed' ? 'proceed' : 'fail')}
              aria-label="On max"
            >
              <option value="fail">fail</option>
              <option value="proceed">proceed</option>
            </select>
          </label>
        </div>

        <fieldset>
          <legend className={labelCls}>
            Members ({memberCount} selected{memberCount < 2 ? ' — need at least 2' : ''})
          </legend>
          <div className="max-h-40 overflow-y-auto rounded-md border border-border">
            {candidateIds.map((id) => (
              <label key={id} className="flex items-center gap-2 border-b border-border px-2 py-1 text-lead last:border-b-0">
                <input type="checkbox" checked={memberIds.has(id)} onChange={() => toggleMember(id)} />
                <span className="truncate text-ink">{id}</span>
              </label>
            ))}
            {candidateIds.length === 0 && (
              <p className="px-2 py-1 text-note text-ink-mute">no steps to group</p>
            )}
          </div>
        </fieldset>

        <div className="mt-1 flex items-center justify-between">
          {editing && onDelete ? (
            <Button variant="danger-outline" size="sm" onClick={onDelete}>
              Remove loop
            </Button>
          ) : (
            <span />
          )}
          <div className="flex gap-2">
            <Button variant="secondary" size="sm" onClick={onCancel}>
              Cancel
            </Button>
            <Button
              variant="primary"
              size="sm"
              disabled={!canSubmit}
              onClick={() =>
                onSubmit({ name: trimmedName, until, maxIterations, onMax }, [...memberIds])
              }
            >
              {editing ? 'Save' : 'Create loop'}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
