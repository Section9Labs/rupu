// Live Events — the KPI strip: fleet vitals as instrument readouts. Every
// number is real: active runs / awaiting from the dashboard aggregate,
// findings-by-severity from the findings summary, events/min + errors derived
// from the live stream by the page. Numbers roll when they change; Awaiting
// and Errors flash once when they grow; events/min draws its sparkline with
// the charts' stylus and a breathing now-ring at its head.

import { useEffect, useMemo, useState } from 'react';
import { Radio } from 'lucide-react';
import { ConnectionBadge, type ConnectionState } from '../RunEventFeed';
import type { Vitals } from '../../lib/situationRoom/roster';
import { Rolling, useGrewFlash } from '../events/parts';

const SPARK_W = 132;
const SPARK_H = 34;

function Kpi({
  label,
  tone,
  flashOn,
  children,
}: {
  label: string;
  tone?: 'await' | 'fail';
  flashOn?: number;
  children: React.ReactNode;
}) {
  const flash = useGrewFlash(flashOn ?? 0);
  return (
    <div className="evx-kpi" data-tone={tone} data-flash={flash ? '' : undefined}>
      <div className="evx-lbl">{label}</div>
      {children}
    </div>
  );
}

const canGlide = () => typeof CSS !== 'undefined' && !!CSS.supports?.('offset-path', 'path("M0 0")');

/** The events/min line: drawn once with a stylus, then the line glides to each new sample. */
export function StylusSpark({ values }: { values: number[] }) {
  const [drawing, setDrawing] = useState(true);
  useEffect(() => {
    const t = setTimeout(() => setDrawing(false), 1000);
    return () => clearTimeout(t);
  }, []);
  const { line, area, head } = useMemo(() => {
    const max = Math.max(1, ...values);
    const step = values.length > 1 ? SPARK_W / (values.length - 1) : SPARK_W;
    const pts = values.map((v, i) => [i * step, SPARK_H - 3 - (v / max) * (SPARK_H - 6)] as const);
    const line = pts.map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(1)} ${y.toFixed(1)}`).join(' ');
    const last = pts[pts.length - 1] ?? [SPARK_W, SPARK_H - 3];
    return { line, area: `${line} L${SPARK_W} ${SPARK_H} L0 ${SPARK_H} Z`, head: last };
  }, [values]);
  return (
    <svg className="evx-spark" viewBox={`0 0 ${SPARK_W} ${SPARK_H}`} data-drawing={drawing ? '' : undefined} aria-hidden>
      <path className="evx-area" d={area} />
      <path className="evx-halo" d={line} pathLength={1} />
      <path className="evx-line" d={line} pathLength={1} />
      {drawing && canGlide() && (
        <circle className="evx-stylus" r="3" style={{ offsetPath: `path("${line}")` }} />
      )}
      <g className="evx-head" style={{ transform: `translate(${head[0]}px, ${head[1]}px)` }}>
        <circle className="evx-head-ring" r="3.5" />
        <circle className="evx-head-core" r="2" />
      </g>
    </svg>
  );
}

export default function PulseStrip({
  vitals,
  connection,
  spark,
}: {
  vitals: Vitals;
  connection: ConnectionState;
  spark: number[];
}) {
  const f = vitals.findings;
  return (
    <header className="flex shrink-0 flex-col items-stretch border-b border-border bg-panel sm:flex-row">
      <div className="flex shrink-0 items-center gap-2.5 border-b border-border px-4 py-2 sm:border-b-0 sm:border-r sm:py-0">
        <Radio size={16} className="text-ink-dim" aria-hidden />
        <div>
          <div className="text-sm font-semibold leading-tight text-ink">Live Events</div>
          <div className="mt-0.5">
            <ConnectionBadge state={connection} />
          </div>
        </div>
      </div>
      <div className="evx-kpis" data-testid="pulse-kpis">
        <Kpi label="Active runs">
          <div className="evx-val">
            <Rolling value={vitals.activeRuns} />
          </div>
        </Kpi>
        <Kpi label="Projects live">
          <div className="evx-val">
            <Rolling value={vitals.projectsLive} />
            <span className="evx-of">/ {vitals.projectsTotal}</span>
          </div>
        </Kpi>
        <Kpi label="Findings · open">
          <div className="evx-val">
            <Rolling value={f.total} />
          </div>
          <div className="evx-sevs">
            {(['critical', 'high', 'medium', 'low'] as const).map((s) =>
              f[s] > 0 ? (
                <span key={s} className="evx-sevchip evx-m" data-mark={s} title={`${f[s]} ${s}`}>
                  <i aria-hidden />
                  {s[0].toUpperCase()}
                  <b>
                    <Rolling value={f[s]} />
                  </b>
                </span>
              ) : null,
            )}
            {f.total === 0 && <span className="text-meta text-ink-mute">none</span>}
          </div>
        </Kpi>
        <Kpi label="Awaiting you" tone={vitals.awaiting > 0 ? 'await' : undefined} flashOn={vitals.awaiting}>
          <div className="evx-val">
            <Rolling value={vitals.awaiting} />
          </div>
        </Kpi>
        <Kpi label="Errors · session" tone={vitals.errors > 0 ? 'fail' : undefined} flashOn={vitals.errors}>
          <div className="evx-val">
            <Rolling value={vitals.errors} />
          </div>
        </Kpi>
        <Kpi label="Events / min">
          <div className="evx-val">
            <Rolling value={vitals.eventsPerMin} />
          </div>
          <StylusSpark values={spark} />
        </Kpi>
      </div>
    </header>
  );
}
