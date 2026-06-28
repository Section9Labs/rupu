// WorkflowSettingsForm — edits the workflow-level name + description. Advanced
// top-level keys (trigger / inputs / autoflow / contracts / …) live in
// `meta.rest`; they are preserved verbatim on every emit and surfaced here as a
// read-only note pointing the operator at the YAML tab to edit them.

import type { WorkflowMeta } from '../../lib/workflowGraph';

interface WorkflowSettingsFormProps {
  meta: WorkflowMeta;
  onChange: (meta: WorkflowMeta) => void;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

export default function WorkflowSettingsForm({ meta, onChange }: WorkflowSettingsFormProps) {
  // Spread `meta` (which carries `rest`) so unmodeled top-level keys survive.
  function patch(p: Partial<WorkflowMeta>): void {
    onChange({ ...meta, ...p });
  }

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
