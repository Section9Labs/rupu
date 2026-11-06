// The run page's turn window, set by selecting turns on the Token flow: what
// the window covers (turns, when, which steps), how each tab follows it, and a
// Clear. The Transcript shows only the window's turns; Events the events of
// its steps (run events carry no time of their own); Findings and Messages
// what was recorded inside its time span.

import { X } from 'lucide-react';
import type { TurnWindow } from '../../lib/run/turnWindow';

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function day(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

/** `14:02:10–14:31:55`, with the day on each side when they differ. */
export function spanLabel(span: [number, number]): string {
  const sameDay = day(span[0]) === day(span[1]);
  return sameDay
    ? `${clock(span[0])}–${clock(span[1])}`
    : `${day(span[0])} ${clock(span[0])} – ${day(span[1])} ${clock(span[1])}`;
}

export default function TurnWindowBar({ window: w, onClear }: { window: TurnWindow; onClear: () => void }) {
  const steps = w.labels.filter(Boolean);
  return (
    <div
      role="status"
      data-testid="turn-window"
      className="gx-window flex flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-brand-500/30 bg-brand-500/5 px-3 py-1.5 text-note text-ink-dim"
    >
      <span className="font-medium text-ink">
        Turns {w.turns[0].toLocaleString()}–{w.turns[1].toLocaleString()}
      </span>
      {w.span && <span className="tabular-nums">{spanLabel(w.span)}</span>}
      {w.partlyTimed && <span className="text-ink-mute">(times from the turns that recorded one)</span>}
      {steps.length > 0 && (
        <span className="min-w-0 truncate">
          {steps.length === 1 ? 'step' : 'steps'} <span className="font-mono text-ink">{steps.join(', ')}</span>
        </span>
      )}
      <span className="text-ink-mute" data-testid="turn-window-scope">
        {w.span
          ? 'transcript: these turns · events: these steps · findings, messages: this time'
          : 'no times recorded for these turns · transcript: these turns · events: these steps'}
      </span>
      <button
        type="button"
        onClick={onClear}
        className="ml-auto inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-medium text-ink transition-colors hover:bg-surface-hover"
      >
        <X size={12} aria-hidden />
        Clear
      </button>
    </div>
  );
}
