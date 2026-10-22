/**
 * TranscriptOutline — a thin clickable rail: one tick per turn (plus
 * command / finding sub-ticks). Scroll-spy highlights the turn nearest the
 * top of the nearest scrolling ancestor of the turns; clicking a tick
 * scrolls to that turn's anchor (`#turn-<n>`, 1-based). It has no scroll
 * area of its own — it sticks and sizes to its content.
 */

import { useEffect, useMemo, useState } from 'react';
import { cn } from '../../lib/cn';
import { blockEventKind, blockMatchesFilter } from './eventKind';
import type { FilterKind } from './eventKind';
import type { TurnView } from './transcriptView';

const RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:ring-offset-1';

/** Nearest ancestor that scrolls, or null for the viewport. */
export function scrollParent(el: HTMLElement | null): HTMLElement | null {
  for (let p = el?.parentElement ?? null; p; p = p.parentElement) {
    const oy = getComputedStyle(p).overflowY;
    if (oy === 'auto' || oy === 'scroll' || oy === 'overlay') return p;
  }
  return null;
}

function reducedMotion(): boolean {
  return typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
}

interface Tick {
  /** 1-based turn number — matches `#turn-<n>`. */
  n: number;
  commands: number;
  findings: number;
}

export default function TranscriptOutline({
  turns,
  filter,
  only,
}: {
  turns: TurnView[];
  filter: FilterKind;
  /** Only these 1-based turns are on the page (a window over the run). */
  only?: ReadonlySet<number>;
}) {
  const ticks = useMemo<Tick[]>(() => {
    const out: Tick[] = [];
    turns.forEach((t, i) => {
      if (only && !only.has(i + 1)) return;
      const shown = t.blocks.filter((b) => blockMatchesFilter(b, filter));
      if (shown.length === 0) return;
      out.push({
        n: i + 1,
        commands: shown.filter((b) => blockEventKind(b) === 'command').length,
        findings: shown.filter((b) => blockEventKind(b) === 'finding').length,
      });
    });
    return out;
  }, [turns, filter, only]);

  const [active, setActive] = useState<number | null>(null);
  const [rail, setRail] = useState<HTMLElement | null>(null);

  useEffect(() => {
    if (!rail || typeof IntersectionObserver === 'undefined') return;
    const targets = ticks
      .map((t) => document.getElementById(`turn-${t.n}`))
      .filter((e): e is HTMLElement => e !== null);
    if (targets.length === 0) return;
    const visible = new Set<number>();
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          const n = Number(e.target.id.slice('turn-'.length));
          if (e.isIntersecting) visible.add(n);
          else visible.delete(n);
        }
        // The turn nearest the top of the scroll area wins.
        if (visible.size > 0) setActive(Math.min(...visible));
      },
      // Only the top band of the scroll area counts as "current".
      { root: scrollParent(targets[0]), rootMargin: '0px 0px -70% 0px', threshold: 0 },
    );
    targets.forEach((t) => io.observe(t));
    return () => io.disconnect();
  }, [rail, ticks]);

  if (ticks.length === 0) return null;

  const go = (n: number) => {
    const el = document.getElementById(`turn-${n}`);
    if (!el) return;
    el.scrollIntoView({ behavior: reducedMotion() ? 'auto' : 'smooth', block: 'start' });
    setActive(n);
  };

  return (
    <nav
      ref={setRail}
      aria-label="Transcript outline"
      data-testid="transcript-outline"
      className="sticky top-12 flex w-5 shrink-0 flex-col items-center gap-1 self-start py-2"
    >
      {ticks.map((t) => (
        <div key={t.n} className="flex flex-col items-center gap-0.5">
          <a
            href={`#turn-${t.n}`}
            data-testid="outline-tick"
            aria-label={`Turn ${t.n}`}
            aria-current={active === t.n ? 'true' : undefined}
            title={`Turn ${t.n}`}
            onClick={(e) => {
              e.preventDefault();
              go(t.n);
            }}
            className={cn(
              'block h-1.5 w-3 rounded-full',
              RING,
              active === t.n ? 'bg-brand-500' : 'bg-border hover:bg-ink-mute',
            )}
          />
          {t.commands > 0 && <span aria-hidden data-testid="outline-sub" className="h-1 w-1 rounded-full bg-ink-mute" />}
          {t.findings > 0 && <span aria-hidden data-testid="outline-sub" className="h-1 w-1 rounded-full bg-brand-500" />}
        </div>
      ))}
    </nav>
  );
}
