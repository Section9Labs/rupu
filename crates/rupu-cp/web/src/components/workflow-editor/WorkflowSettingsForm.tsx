// WorkflowSettingsForm — edits the workflow-level name + description. Advanced
// top-level keys (trigger / inputs / autoflow / contracts / …) live in
// `meta.rest`; they are preserved verbatim on every emit.
//
// Classic path (default / `workflowEditorUi !== 'next'`): renders EXACTLY
// today's markup, byte-identical — every advanced key (including
// `trigger`/`inputs`) surfaces as a read-only chip pointing the operator at
// the YAML tab. This branch must never be touched by future edits to the
// `next` branch below it.
//
// `next` path (Task 5, extended Task 6): an early return, BEFORE the classic
// markup, so the classic branch stays untouched. `trigger`/`inputs` get
// dedicated authoring cards (TriggerCard / InputsCard, under `./settings/`)
// instead of chips; `autoflow` gets an authoring card (AutoflowCard) plus a
// read-only lifecycle viz below it (LifecycleRibbon). Any OTHER advanced key
// still surfaces as a read-only chip below the cards.

import type { WorkflowMeta } from '../../lib/workflowGraph';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import TriggerCard from './settings/TriggerCard';
import InputsCard from './settings/InputsCard';
import AutoflowCard from './settings/AutoflowCard';
import LifecycleRibbon from './settings/LifecycleRibbon';

interface WorkflowSettingsFormProps {
  meta: WorkflowMeta;
  onChange: (meta: WorkflowMeta) => void;
  /** Workflow-editor-UI flag — gates the Trigger/Inputs authoring cards.
   *  Defaults to 'classic' (today's read-only rest-chips form, unchanged) for
   *  callers that don't thread it. */
  workflowEditorUi?: WorkflowEditorUi;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

export default function WorkflowSettingsForm({
  meta,
  onChange,
  workflowEditorUi = 'classic',
}: WorkflowSettingsFormProps) {
  // Spread `meta` (which carries `rest`) so unmodeled top-level keys survive.
  function patch(p: Partial<WorkflowMeta>): void {
    onChange({ ...meta, ...p });
  }

  // ── `next` (Task 5): Trigger + Inputs authoring cards ────────────────────
  if (workflowEditorUi === 'next') {
    const onRest = (rest: Record<string, unknown>): void => {
      patch({ rest });
    };
    const restKeys = Object.keys(meta.rest).filter((k) => k !== 'trigger' && k !== 'inputs' && k !== 'autoflow');

    return (
      <div className="space-y-4" data-ui="next">
        <label className="block">
          <span className={labelCls}>Name</span>
          <input
            type="text"
            value={meta.name}
            onChange={(e) => patch({ name: e.target.value })}
            aria-label="Workflow name"
            className={fieldCls}
          />
        </label>

        <label className="block">
          <span className={labelCls}>Description</span>
          <textarea
            value={meta.description ?? ''}
            onChange={(e) => patch({ description: e.target.value === '' ? undefined : e.target.value })}
            aria-label="Workflow description"
            rows={3}
            className={`${fieldCls} resize-y`}
          />
        </label>

        <TriggerCard rest={meta.rest} onRest={onRest} />
        <InputsCard rest={meta.rest} onRest={onRest} />
        <AutoflowCard rest={meta.rest} onRest={onRest} />
        <LifecycleRibbon rest={meta.rest} />

        {restKeys.length > 0 && (
          <div className="rounded-md border border-border bg-surface px-3 py-2.5">
            <p className="text-ui font-medium text-ink-dim">
              Preserved advanced keys — edit these in the YAML tab:
            </p>
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {restKeys.map((k) => (
                <span
                  key={k}
                  className="inline-flex items-center rounded px-1.5 py-0.5 font-mono text-note font-medium ring-1 bg-surface text-ink-mute ring-border"
                >
                  {k}
                </span>
              ))}
            </div>
          </div>
        )}
      </div>
    );
  }

  // ── classic (unchanged) ───────────────────────────────────────────────────
  const restKeys = Object.keys(meta.rest);

  return (
    <div className="space-y-4">
      <label className="block">
        <span className={labelCls}>Name</span>
        <input
          type="text"
          value={meta.name}
          onChange={(e) => patch({ name: e.target.value })}
          aria-label="Workflow name"
          className={fieldCls}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Description</span>
        <textarea
          value={meta.description ?? ''}
          onChange={(e) => patch({ description: e.target.value === '' ? undefined : e.target.value })}
          aria-label="Workflow description"
          rows={3}
          className={`${fieldCls} resize-y`}
        />
      </label>

      {restKeys.length > 0 && (
        <div className="rounded-md border border-border bg-surface px-3 py-2.5">
          <p className="text-ui font-medium text-ink-dim">
            Preserved advanced keys — edit these in the YAML tab:
          </p>
          <div className="mt-1.5 flex flex-wrap gap-1.5">
            {restKeys.map((k) => (
              <span
                key={k}
                className="inline-flex items-center rounded px-1.5 py-0.5 font-mono text-note font-medium ring-1 bg-surface text-ink-mute ring-border"
              >
                {k}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
