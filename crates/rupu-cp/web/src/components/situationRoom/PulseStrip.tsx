// Situation Room — the header "pulse strip": fleet vitals as instrument
// readouts. Every number is real: active runs / awaiting from the dashboard
// aggregate, findings-by-severity from the findings summary, events/min +
// errors derived from the live stream by the page. A KPI briefly flashes when
// its value climbs.

import { useEffect, useRef, useState } from 'react';
import { Radio } from 'lucide-react';
import { cn } from '../../lib/cn';
import { ConnectionBadge, type ConnectionState } from '../RunEventFeed';
import type { Vitals } from '../../lib/situationRoom/roster';

/** Adds a transient `flash` class whenever `value` increases. */
function useFlash(value: number): boolean {
  const prev = useRef(value);
  const [flash, setFlash] = useState(false);
  useEffect(() => {
    if (value > prev.current) {
      setFlash(true);
      const t = setTimeout(() => setFlash(false), 600);
      prev.current = value;
      return () => clearTimeout(t);
    }
    prev.current = value;
  }, [value]);
  return flash;
}

function Kpi({
  label,
  value,
  accentVar,
  children,
  minWidth,
}: {
  label: string;
  value?: number;
  accentVar?: string;
  children?: React.ReactNode;
  minWidth?: number;
}) {
  const flash = useFlash(value ?? 0);
  return (
    <div className={cn('sr-kpi', flash && 'flash')} style={minWidth ? { minWidth } : undefined}>
      <div className="sr-kpi-lbl">{label}</div>
      {children ?? (
        <div className="sr-kpi-val" style={accentVar ? { color: `rgb(var(${accentVar}))` } : undefined}>
          {value}
        </div>
      )}
    </div>
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
  const max = Math.max(1, ...spark);
  return (
    <header className="flex items-stretch border-b border-border bg-panel/70 shrink-0">
      <div className="sr-brandcell flex items-center gap-2.5 px-5 border-r border-border">
        <Radio size={16} className="text-ink-dim" />
        <div>
          <div className="text-sm font-semibold leading-tight text-ink">Live Events</div>
          <div className="text-meta uppercase tracking-[0.16em] text-ink-mute">Situation Room</div>
        </div>
        <div className="ml-2">
          <ConnectionBadge state={connection} />
        </div>
      </div>

      <div className="flex flex-1 min-w-0 overflow-hidden">
        <Kpi label="Active runs" value={vitals.activeRuns} />
        <Kpi label="Projects live" minWidth={124}>
          <div className="sr-kpi-val">
            {vitals.projectsLive} <small>/ {vitals.projectsTotal}</small>
          </div>
        </Kpi>
        <Kpi label="Findings · open" minWidth={196}>
          <div className="sr-kpi-val">{f.total}</div>
          <div className="flex gap-1.5 mt-0.5">
            {f.critical > 0 && <span className="sr-sevpip sr-sev-critical">C {f.critical}</span>}
            {f.high > 0 && <span className="sr-sevpip sr-sev-high">H {f.high}</span>}
            {f.medium > 0 && <span className="sr-sevpip sr-sev-medium">M {f.medium}</span>}
            {f.low > 0 && <span className="sr-sevpip sr-sev-low">L {f.low}</span>}
            {f.total === 0 && <span className="text-meta text-ink-mute">none</span>}
          </div>
        </Kpi>
        <Kpi label="Awaiting you" value={vitals.awaiting} accentVar="--c-status-awaiting" />
        <Kpi label="Errors · session" value={vitals.errors} accentVar={vitals.errors > 0 ? '--c-status-failed' : undefined} />
        <Kpi label="Events / min" minWidth={150}>
          <div className="flex items-end gap-2">
            <div className="sr-kpi-val text-[18px]">{vitals.eventsPerMin}</div>
            <div className="sr-spark" aria-hidden>
              {spark.map((v, i) => (
                <i key={i} style={{ height: `${Math.max(4, Math.round((v / max) * 24))}px` }} />
              ))}
            </div>
          </div>
        </Kpi>
      </div>
    </header>
  );
}
