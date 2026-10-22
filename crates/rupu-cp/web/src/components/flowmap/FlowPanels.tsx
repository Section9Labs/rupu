// The summary chip over the agentiflow map's canvas (FlowMap). The run's
// goals and budget are not here: they sit once, in the page's Status panel.

import type { RunChild } from '../../lib/api';
import type { StepState } from '../../lib/runGraphModel';
import { childState } from './ChildNode';

/** Over the canvas's top-right corner: the children at a glance. */
export function MapSummary({ list, messages }: { list: RunChild[]; messages: number }) {
  const count = (s: StepState) => list.filter((c) => childState(c) === s).length;
  const items: [StepState, number, string][] = [
    ['done', count('done'), 'done'],
    ['running', count('running'), 'running'],
    ['failed', count('failed'), 'failed'],
  ];
  return (
    <div className="gx-summary" data-testid="flow-summary">
      {items
        .filter(([, n]) => n > 0)
        .map(([s, n, w]) => (
          <span key={s} data-state={s} className="gx-summary-item gx-s">
            <i className="afx-dot" aria-hidden />
            <b>{n}</b> {w}
          </span>
        ))}
      <span className="gx-summary-item">
        <b>{messages}</b> messages
      </span>
    </div>
  );
}
