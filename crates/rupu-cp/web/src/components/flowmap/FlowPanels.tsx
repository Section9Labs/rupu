// The agentiflow map's panels around the canvas (FlowMap): the header strip
// (round, goals met, spend, tokens, time), the summary chip over the canvas,
// and the Goals panel under it.

import { CheckCircle2, Circle } from 'lucide-react';
import type { AgentiflowDetail, RunChild } from '../../lib/api';
import type { buildFlowModel } from '../../lib/flow/flowModel';
import type { StepState } from '../../lib/runGraphModel';
import { formatSpendTokens, formatSpendUsd, goalViews } from '../../lib/agentiflow';
import { formatDuration } from '../../lib/duration';
import { childState } from './ChildNode';
import CollapsiblePanel from './CollapsiblePanel';

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

/** Goals met after the latest assessed round (the record's once the flow
 *  completes), and the run's spend. */
function latestGoals(detail: AgentiflowDetail, model: ReturnType<typeof buildFlowModel>) {
  if (detail.record.status === 'completed' && detail.record.goals.length > 0) return detail.record.goals;
  for (let i = model.rounds.length - 1; i >= 0; i--) if (model.rounds[i].goalsAfter) return model.rounds[i].goalsAfter!;
  return detail.record.goals;
}

function Meter({ value, tone }: { value: number; tone: 'ok' | 'brand' }) {
  return (
    <span className="afx-meter" data-tone={tone} aria-hidden>
      <i style={{ transform: `scaleX(${Math.min(1, Math.max(0, value))})` }} />
    </span>
  );
}

export function HeaderStrip({
  detail,
  model,
  running,
}: {
  detail: AgentiflowDetail;
  model: ReturnType<typeof buildFlowModel>;
  running: boolean;
}) {
  const { record, def } = detail;
  const goals = latestGoals(detail, model);
  const total = def?.goals.length ?? record.goals_total;
  const met = goals.filter((g) => g.met).length;
  const capUsd = def?.budget?.usd ?? null;
  const capRounds = def?.budget?.rounds ?? null;
  const started = Date.parse(record.started_at);
  const ended = record.ended_at ? Date.parse(record.ended_at) : Date.now();
  return (
    <div className="afx-head" data-testid="flow-head">
      <span>
        <span className="k">round</span> <b>{model.rounds.length}</b>
        {capRounds ? ` / ${capRounds}` : ''}
      </span>
      <span>
        <span className="k">goals</span> {total > 0 && <Meter value={met / total} tone="ok" />}
        <b>{met}</b> / {total} met
      </span>
      <span>
        <span className="k">spend</span> {capUsd ? <Meter value={(record.spent_usd ?? 0) / capUsd} tone="brand" /> : null}
        <b>{formatSpendUsd(record.spent_usd)}</b>
        {capUsd ? ` / ${formatSpendUsd(capUsd)}` : ''}
      </span>
      <span>
        <b>{formatSpendTokens(record.spent_tokens)}</b>
      </span>
      {Number.isFinite(started) && (
        <span>
          <span className="k">{running ? 'running' : 'ran'}</span> {formatDuration(Math.max(0, ended - started))}
        </span>
      )}
    </div>
  );
}

export function GoalsPanel({
  detail,
  model,
  list,
  spaces,
}: {
  detail: AgentiflowDetail;
  model: ReturnType<typeof buildFlowModel>;
  list: RunChild[];
  spaces: { participants: { live: boolean }[] }[];
}) {
  const after = new Map(latestGoals(detail, model).map((g) => [g.id, g]));
  const views = goalViews(detail.record, detail.def);
  const count = (s: StepState) => list.filter((c) => childState(c) === s).length;
  const participants = spaces.flatMap((s) => s.participants);
  const metNow = views.filter((g) => after.get(g.id)?.met ?? g.met).length;
  return (
    <CollapsiblePanel
      name="flow-goals"
      title="Goals"
      count={views.length > 0 ? `${metNow}/${views.length}` : undefined}
      sub={detail.def?.description && <span className="afx-panel-sub">{detail.def.description}</span>}
    >
      <ul className="afx-goals">
        {views.map((g) => {
          const now = after.get(g.id);
          const met = now?.met ?? g.met;
          return (
            <li key={g.id}>
              {met ? (
                <CheckCircle2 size={14} className="text-ok" aria-label="met" />
              ) : (
                <Circle size={14} className="text-ink-mute" aria-label="not met" />
              )}
              <span className="o">{g.objective}</span>
              <span className="c">
                {now?.current ?? g.current}
                {g.target > 0 ? `/${g.target}` : ''}
              </span>
            </li>
          );
        })}
        {views.length === 0 && <li className="text-ink-mute">No goals recorded.</li>}
      </ul>
      <dl className="afx-kv">
        <dt>children</dt>
        <dd>
          {count('done')} done · {count('running')} running · {count('failed')} failed
        </dd>
        <dt>participants</dt>
        <dd>
          {participants.filter((p) => p.live).length} live of {participants.length}
        </dd>
      </dl>
    </CollapsiblePanel>
  );
}
