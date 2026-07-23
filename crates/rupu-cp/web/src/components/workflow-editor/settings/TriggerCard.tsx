// TriggerCard — Trigger authoring card for the Settings inspector (Task 5),
// rendered only when `workflowEditorUi === 'next'` (WorkflowSettingsForm
// decides that; this component doesn't gate itself).
//
// A segmented control for `on` (manual/cron/event) with mode-specific fields
// beneath it. Every change reads the CURRENT model fresh off `rest` via
// `readTrigger`, patches it, and writes back via `writeTrigger` — never
// hand-building the `trigger:` shape here — so the emitted trigger always
// stays schema-valid: manual carries no cron/event/filter, cron carries only
// `cron`, event carries `event` (+ optional `filter`).

import { readTrigger, writeTrigger, type TriggerOn } from '../../../lib/workflowMeta';

interface TriggerCardProps {
  rest: Record<string, unknown>;
  onRest: (rest: Record<string, unknown>) => void;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

const ON_OPTIONS: { value: TriggerOn; label: string }[] = [
  { value: 'manual', label: 'manual' },
  { value: 'cron', label: 'cron' },
  { value: 'event', label: 'event' },
];

export default function TriggerCard({ rest, onRest }: TriggerCardProps) {
  const model = readTrigger(rest);

  function setOn(on: TriggerOn): void {
    // Switching mode drops the previous mode's fields (mirrors StepForm's
    // switchKind) — only the fields valid for the new `on` survive.
    onRest(writeTrigger(rest, { on }));
  }
  function setCron(cron: string): void {
    onRest(writeTrigger(rest, { on: 'cron', cron: cron === '' ? undefined : cron }));
  }
  function setEvent(event: string): void {
    onRest(writeTrigger(rest, { on: 'event', event: event === '' ? undefined : event, filter: model.filter }));
  }
  function setFilter(filter: string): void {
    onRest(writeTrigger(rest, { on: 'event', event: model.event, filter: filter === '' ? undefined : filter }));
  }

  return (
    <div className="wfx-card" data-testid="trigger-card">
      <div className="wfx-card-h">Trigger</div>
      <div className="wfx-card-b">
        <div>
          <span className={labelCls}>On</span>
          <div className="wfx-seg" role="group" aria-label="Trigger on">
            {ON_OPTIONS.map((opt) => (
              <button
                key={opt.value}
                type="button"
                aria-pressed={model.on === opt.value}
                onClick={() => setOn(opt.value)}
              >
                {opt.label}
              </button>
            ))}
          </div>
        </div>

        {model.on === 'cron' && (
          <label className="block">
            <span className={labelCls}>Cron</span>
            <input
              type="text"
              value={model.cron ?? ''}
              onChange={(e) => setCron(e.target.value)}
              aria-label="Trigger cron"
              placeholder="min hour dom mon dow"
              className={`${fieldCls} font-mono`}
            />
            <span className="mt-1 block text-note text-ink-mute">5-field: min hour dom mon dow</span>
          </label>
        )}

        {model.on === 'event' && (
          <>
            <label className="block">
              <span className={labelCls}>Event</span>
              <input
                type="text"
                value={model.event ?? ''}
                onChange={(e) => setEvent(e.target.value)}
                aria-label="Trigger event"
                className={`${fieldCls} font-mono`}
              />
            </label>
            <label className="block">
              <span className={labelCls}>Filter (optional)</span>
              <input
                type="text"
                value={model.filter ?? ''}
                onChange={(e) => setFilter(e.target.value)}
                aria-label="Trigger filter"
                className={`${fieldCls} font-mono`}
              />
              <span className="mt-1 block text-note text-ink-mute">
                minijinja expression over the event payload
              </span>
            </label>
          </>
        )}
      </div>
    </div>
  );
}
