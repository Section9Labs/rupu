// LifecycleRibbon — read-only viz of the autoflow lifecycle for the Settings
// inspector (Task 6), rendered only when `workflowEditorUi === 'next'`
// (WorkflowSettingsForm decides that; this component doesn't gate itself).
//
// Six stages — selector -> author gate -> claim -> run -> reconcile ->
// outcome — summarized from `readAutoflow(rest)`. Purely derived from props;
// NON-interactive (no click handlers, no local state). Renders a muted
// "autoflow disabled" hint instead of the stage strip when autoflow is
// absent or disabled, so an operator glancing at Settings sees at once
// whether this workflow runs autonomously.

import { Fragment } from 'react';
import { readAutoflow } from '../../../lib/workflowMeta';

interface LifecycleRibbonProps {
  rest: Record<string, unknown>;
}

interface Stage {
  h: string;
  v: string;
  m: string;
}

export default function LifecycleRibbon({ rest }: LifecycleRibbonProps) {
  const model = readAutoflow(rest);

  if (!model || !model.enabled) {
    return (
      <div className="wfx-lc" data-testid="lifecycle-ribbon">
        <p className="wfx-lc-hint">
          Autoflow disabled — enable it above to run this workflow autonomously and see its lifecycle here.
        </p>
      </div>
    );
  }

  const labelsAll = model.selector.labels_all ?? [];
  const stages: Stage[] = [
    {
      h: 'Selector',
      v: [model.selector.states?.join('/') || model.entity, labelsAll.join('+')].filter(Boolean).join(' · '),
      m: model.selector.limit !== undefined ? `≤ ${model.selector.limit}` : 'no limit',
    },
    {
      h: 'Author gate',
      v: model.selector.authors_from ?? 'anyone',
      m: model.selector.on_skip ?? '—',
    },
    {
      h: 'Claim',
      v: model.claim?.ttl ? `ttl ${model.claim.ttl}` : '—',
      m: `key: ${model.claim?.key ?? '—'}`,
    },
    {
      h: 'Run workflow',
      v: model.workspace?.strategy ?? 'worktree',
      m: model.workspace?.branch ?? '—',
    },
    {
      h: 'Reconcile',
      v: model.reconcile_every ? `every ${model.reconcile_every}` : '—',
      m: `${model.wake_on.length} wake event${model.wake_on.length === 1 ? '' : 's'}`,
    },
    {
      h: 'Outcome',
      v: model.outcome?.output ?? '—',
      m: 'stops when emitted',
    },
  ];

  return (
    <div className="wfx-lc" data-testid="lifecycle-ribbon">
      <div className="wfx-lc-title">Autoflow lifecycle — this workflow, run autonomously over {model.entity}s</div>
      <div className="wfx-lc-flow">
        {stages.map((s, i) => (
          <Fragment key={s.h}>
            <div className="wfx-lc-step">
              <div className="wfx-lc-h">{s.h}</div>
              <div className="wfx-lc-v">{s.v || '—'}</div>
              <div className="wfx-lc-m">{s.m}</div>
            </div>
            {i < stages.length - 1 && (
              <div className="wfx-lc-arrow" aria-hidden="true">
                {i === stages.length - 2 ? '↺' : '→'}
              </div>
            )}
          </Fragment>
        ))}
      </div>
    </div>
  );
}
