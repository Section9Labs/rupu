// The time axis: round / step bands, message ticks coloured by kind, a
// draggable scrubber, a "live" pill and a Replay button. Pure presentation
// over useTimeline's state; motion classes are in styles/graph-motion.css.

import { useRef } from 'react';
import { useThemeColors } from '../../lib/useThemeColors';
import type { TimelineBand, TimelineItem } from '../../lib/timeline/timeline';

// Message row kind -> theme token (Task 11 swaps this for messagePaint).
type Token = Parameters<ReturnType<typeof useThemeColors>['get']>[0];
const TICK_TOKEN: Record<string, Token> = {
  post: 'chart.5',
  mention: 'chart.5',
  direct: 'chart.4',
  broadcast: 'chart.3',
  directive: 'chart.1',
  retract: 'chart.1',
  operator: 'status.awaiting',
};

export function TimeAxis({
  bands,
  items,
  bounds,
  t,
  live,
  onScrub,
  onReplay,
  onRelease,
}: {
  bands: TimelineBand[];
  items: TimelineItem[];
  bounds: { start: number; end: number };
  t: number | null;
  live: boolean;
  onScrub: (t: number) => void;
  onReplay: () => void;
  onRelease: () => void;
}) {
  const colors = useThemeColors();
  const track = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const span = Math.max(1, bounds.end - bounds.start);
  const pct = (at: number) => `${Math.min(100, Math.max(0, ((at - bounds.start) / span) * 100))}%`;

  const timeAt = (clientX: number) => {
    const r = track.current!.getBoundingClientRect();
    const f = r.width > 0 ? Math.min(1, Math.max(0, (clientX - r.left) / r.width)) : 0;
    return bounds.start + f * span;
  };

  const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  return (
    <div className="tl-axis">
      <div
        ref={track}
        data-testid="time-track"
        className="tl-track"
        onPointerDown={(e) => {
          dragging.current = true;
          e.currentTarget.setPointerCapture?.(e.pointerId);
          onScrub(timeAt(e.clientX));
        }}
        onPointerMove={(e) => {
          if (dragging.current) onScrub(timeAt(e.clientX));
        }}
        onPointerUp={() => {
          dragging.current = false;
        }}
        onPointerCancel={() => {
          dragging.current = false;
        }}
      >
        {bands.map((b, i) => {
          const to = b.to ?? bounds.end;
          return (
            <div
              key={b.key}
              className="tl-band"
              data-alt={i % 2}
              title={b.label}
              style={{ left: pct(b.from), width: `calc(${pct(to)} - ${pct(b.from)})` }}
            >
              <span className="tl-band-label">{b.label}</span>
            </div>
          );
        })}
        {items
          .filter((i) => i.kind === 'message')
          .map((i) => (
            <span
              key={i.key}
              className="tl-tick"
              style={{ left: pct(i.at), background: colors.get(TICK_TOKEN[i.sub ?? 'post'] ?? 'chart.5') }}
            />
          ))}
        {t !== null && <span className="tl-scrubber" style={{ left: pct(t) }} aria-hidden />}
      </div>
      <div className="tl-controls">
        {live && t === null ? (
          <span className="tl-live" aria-live="polite">
            <span>live</span> <span className="tl-live-time">{clock(bounds.end)}</span>
          </span>
        ) : (
          <button type="button" className="tl-btn" onClick={onRelease}>
            {live ? 'Back to live' : 'Show all'}
          </button>
        )}
        <button type="button" className="tl-btn" onClick={onReplay}>
          Replay
        </button>
      </div>
    </div>
  );
}
