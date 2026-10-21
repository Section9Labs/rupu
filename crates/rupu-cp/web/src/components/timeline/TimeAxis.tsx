// The time axis: round / step bands, message ticks coloured by kind, a
// draggable scrubber, a "live" pill and a Replay button. Pure presentation
// over useTimeline's state; motion classes are in styles/graph-motion.css.

import { useRef } from 'react';
import { useThemeColors } from '../../lib/useThemeColors';
import type { TimelineBand, TimelineItem } from '../../lib/timeline/timeline';
import { messagePaint } from '../../lib/flow/messagePaint';

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
  const cur = t ?? bounds.end;

  const onKey = (e: React.KeyboardEvent) => {
    const step = span * (e.shiftKey ? 0.1 : 0.01);
    let next: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') next = cur - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') next = cur + step;
    else if (e.key === 'Home') next = bounds.start;
    else if (e.key === 'End') next = bounds.end;
    if (next === null) return;
    // Live / whole: only a move away from the end starts scrubbing.
    if (t === null && (e.key === 'End' || e.key === 'ArrowRight' || e.key === 'ArrowUp')) return;
    e.preventDefault();
    onScrub(Math.min(bounds.end, Math.max(bounds.start, next)));
  };

  return (
    <div className="tl-axis">
      <div
        ref={track}
        data-testid="time-track"
        className="tl-track"
        role="slider"
        tabIndex={0}
        aria-label="Timeline position"
        aria-valuemin={bounds.start}
        aria-valuemax={bounds.end}
        aria-valuenow={cur}
        aria-valuetext={clock(cur)}
        onKeyDown={onKey}
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
              data-row={b.row ?? 'round'}
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
              style={{ left: pct(i.at), background: messagePaint(i.sub ?? 'post', colors).stroke }}
            />
          ))}
        {t !== null && (
          <div
            className="tl-scrubber"
            data-testid="time-scrubber"
            style={{ transform: `translateX(${((t - bounds.start) / span) * 100}%)` }}
            aria-hidden
          >
            <span className="tl-scrubber-line" />
          </div>
        )}
      </div>
      <div className="tl-controls">
        {live && t === null && (
          <span className="tl-live">
            <span>live</span> <span className="tl-live-time">{clock(bounds.end)}</span>
          </span>
        )}
        {t !== null && (
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
