// The event rail (styles/events.css): Live Events' stream and the run and
// agentiflow Events tabs. Newest first under a "now" head, in five-minute
// groups whose separators stick under the Needs-you deck; rows of one run
// joined by a thread in its crew tint.
//
// Arrivals: at the top, a new row draws the rail down to its marker while the
// rows below glide down to make room (FLIP). Scrolled down, new rows wait
// behind an "N new" pill and nothing under the reader moves (the deck's own
// growth is paid back in scroll). Answering a deck card sends a shell of it
// home into its row, which blooms.

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowUp } from 'lucide-react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import { crewTint } from '../../lib/codename';
import { useThemeMode } from '../codename/useThemeMode';
import {
  deckOf,
  gateOutcomes,
  groupByBucket,
  relAge,
  type GateOutcome,
  type Mark,
  type RailScope,
  type Resolved,
} from '../../lib/events/rail';
import { useSeen } from '../../lib/events/seen';
import EventRow, { clock } from './EventRow';
import NeedsYouDeck from './NeedsYouDeck';
import type { EventGate } from './GateButtons';
import { Rolling, useNow } from './parts';

const HOLD_AT = 24;
const ENTRANCE_ROWS = 16;
const FLIP_ROWS = 80;

export interface EventRailProps {
  /** Newest first. */
  cards: StreamCard[];
  scope: RailScope;
  width?: 'wide' | 'narrow';
  resolve?: (card: StreamCard) => Resolved;
  gate?: EventGate;
  /** A run the page knows has ended (its gates are answered). */
  terminal?: (runId: string) => string | undefined;
  /** Whether a run is still working (its newest row's marker spins). */
  isRunLive?: (runId: string) => boolean;
  /** The head breathes while something is live. */
  live: boolean;
  nowSub: string;
  emptyText: string;
  hasMoreOlder?: boolean;
  loadingOlder?: boolean;
  onLoadOlder?: () => void;
  footer?: ReactNode;
  className?: string;
}

const reducedMotion = () =>
  typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

function esc(s: string): string {
  return typeof CSS !== 'undefined' && CSS.escape ? CSS.escape(s) : s.replace(/["\\]/g, '\\$&');
}

/** Replay a cue on an element (a bloom, a flash): never React state. */
function cue(el: Element | null | undefined, name: string, ms: number) {
  if (!(el instanceof HTMLElement)) return;
  delete el.dataset.cue;
  void el.offsetWidth;
  el.dataset.cue = name;
  setTimeout(() => {
    if (el.dataset.cue === name) delete el.dataset.cue;
  }, ms);
}

export default function EventRail(props: EventRailProps) {
  const { cards, scope, gate, terminal, isRunLive } = props;
  const mode = useThemeMode();
  const { isSeen, markSeen, version: seenVersion } = useSeen();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const deckBoxRef = useRef<HTMLDivElement | null>(null);
  const [holding, setHolding] = useState(false);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const knownRef = useRef<Set<string>>(new Set());
  const arrivedRef = useRef<Set<string>>(new Set());
  const initialRef = useRef(true);
  const posRef = useRef<Map<string, number>>(new Map());
  const homeRef = useRef<Map<string, { shell: HTMLElement; rect: DOMRect; cue: string }>>(new Map());
  const kinRef = useRef<string | null>(null);
  const resolve = props.resolve ?? (() => ({}));

  // ── what is shown, and what waits behind the pill ──
  const known = knownRef.current;
  const overlap = cards.some((c) => known.has(c.key));
  if (!overlap && known.size > 0) {
    // A new list altogether (a query, another run): start over.
    knownRef.current = new Set();
    arrivedRef.current = new Set();
    posRef.current = new Map();
    initialRef.current = true;
  }
  const firstKnown = cards.findIndex((c) => knownRef.current.has(c.key));
  const heldKeys = new Set<string>();
  if (holding && firstKnown > 0) for (const c of cards.slice(0, firstKnown)) heldKeys.add(c.key);
  const shown = heldKeys.size ? cards.filter((c) => !heldKeys.has(c.key)) : cards;
  if (!initialRef.current && firstKnown > 0 && !holding) {
    for (const c of cards.slice(0, firstKnown)) if (!knownRef.current.has(c.key)) arrivedRef.current.add(c.key);
  }

  // ── gates, the deck ──
  const outcomes = useMemo(() => gateOutcomes(cards, terminal), [cards, terminal]);
  const now = useNow(60_000);
  const deck = useMemo(
    () =>
      deckOf(cards, {
        now,
        isSeen,
        gates: outcomes,
        answered: gate ? (c) => gate.state(c).decision != null : undefined,
      }),
    // seenVersion changes with every mark
    [cards, now, isSeen, seenVersion, outcomes, gate],
  );

  // ── threads: runs, and the crew tint they are drawn in ──
  const threadOf = useMemo(() => {
    const byCrew = new Map<string, string>();
    for (const c of cards) if (c.runId && c.crew && !byCrew.has(c.crew)) byCrew.set(c.crew, c.runId);
    return (c: StreamCard): string | undefined => c.runId ?? (c.crew ? (byCrew.get(c.crew) ?? c.crew) : undefined);
  }, [cards]);
  const crewOf = useCallback((c: StreamCard) => (c.crew ? crewTint(c.crew, mode) : undefined), [mode]);

  const timed = cards.some((c) => c.ts > 0);
  const groups = useMemo(() => (timed ? groupByBucket(shown) : [{ bucket: null, newest: 0, cards: shown }]), [timed, shown]);

  // The newest row of each live run is its frontier: its marker spins.
  const frontier = useMemo(() => {
    const out = new Set<string>();
    const seenThreads = new Set<string>();
    for (const c of shown) {
      const t = threadOf(c);
      if (!t || seenThreads.has(t)) continue;
      seenThreads.add(t);
      if (c.form === 'activity' && c.runId && isRunLive?.(c.runId)) out.add(c.key);
    }
    return out;
  }, [shown, threadOf, isRunLive]);

  // ── after each render: remember, measure, glide ──
  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    for (const c of shown) knownRef.current.add(c.key);
    initialRef.current = false;
    if (!scroll) return;
    // Layout positions (offsets ignore transforms), so a glide still in
    // flight never skews the next measurement; its live offset is added back.
    const els = [...scroll.querySelectorAll<HTMLElement>('[data-evkey]')].slice(0, FLIP_ROWS);
    const next = new Map<string, number>();
    const moved: [HTMLElement, number][] = [];
    const flip = !reducedMotion();
    for (const el of els) {
      const key = el.dataset.evkey!;
      const top = layoutTop(el, scroll);
      next.set(key, top);
      const prev = posRef.current.get(key);
      if (flip && prev != null && Math.abs(prev - top) > 0.5) moved.push([el, prev - top + liveTranslateY(el)]);
    }
    posRef.current = next;
    if (moved.length) {
      for (const [el, d] of moved) {
        el.style.transition = 'none';
        el.style.transform = `translateY(${d}px)`;
      }
      void scroll.offsetWidth;
      for (const [el] of moved) {
        el.style.transition = 'transform var(--evx-flip) var(--ease-out)';
        el.style.transform = '';
      }
      setTimeout(() => {
        for (const [el] of moved) el.style.transition = '';
      }, 420);
    }
  });

  // Arrivals that land or fail say so once they have settled.
  const cuedRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    for (const key of arrivedRef.current) {
      if (cuedRef.current.has(key)) continue;
      cuedRef.current.add(key);
      const card = cards.find((c) => c.key === key);
      const el = scroll.querySelector(`[data-evkey="${esc(key)}"]`);
      if (!card || !el) continue;
      if (card.form === 'complete' && card.accent !== 'error') setTimeout(() => cue(el, 'done', 1100), 380);
      if (card.form === 'error') setTimeout(() => cue(el, 'fail', 500), 300);
    }
  });

  // ── the deck's height: sticky offsets, and scroll paid back while reading ──
  useLayoutEffect(() => {
    const box = deckBoxRef.current;
    const scroll = scrollRef.current;
    if (!box || !scroll || typeof ResizeObserver === 'undefined') return;
    let last = box.offsetHeight;
    scroll.style.setProperty('--deck-h', `${last}px`);
    const ro = new ResizeObserver(() => {
      const h = box.offsetHeight;
      const d = h - last;
      last = h;
      scroll.style.setProperty('--deck-h', `${h}px`);
      if (d && scroll.scrollTop > HOLD_AT) scroll.scrollTop += d;
    });
    ro.observe(box);
    return () => ro.disconnect();
  }, []);

  // ── scrolling: hold arrivals while reading, release at the top, page older ──
  const { hasMoreOlder, loadingOlder, onLoadOlder } = props;
  useEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const onScroll = () => {
      const top = scroll.scrollTop;
      if (top > 2) scroll.dataset.scrolled = '';
      else delete scroll.dataset.scrolled;
      if (top > HOLD_AT) setHolding(true);
      else if (top <= 2) setHolding(false);
      if (hasMoreOlder && !loadingOlder && onLoadOlder && top + scroll.clientHeight >= scroll.scrollHeight - 160) onLoadOlder();
    };
    scroll.addEventListener('scroll', onScroll, { passive: true });
    return () => scroll.removeEventListener('scroll', onScroll);
  }, [hasMoreOlder, loadingOlder, onLoadOlder]);

  const toTop = useCallback(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    if (scroll.scrollTop <= 2) setHolding(false);
    else scroll.scrollTo({ top: 0, behavior: reducedMotion() ? 'auto' : 'smooth' });
  }, []);

  // ── kin: hovering a row lights its run's other rows ──
  const setKin = useCallback((thread: string | null) => {
    if (kinRef.current === thread) return;
    const scroll = scrollRef.current;
    kinRef.current = thread;
    if (!scroll) return;
    for (const el of scroll.querySelectorAll<HTMLElement>('[data-kin]')) delete el.dataset.kin;
    if (thread) for (const el of scroll.querySelectorAll<HTMLElement>(`[data-thread="${esc(thread)}"]`)) el.dataset.kin = '';
  }, []);

  // ── the deck's answers travel home ──
  const keepShell = useCallback((card: StreamCard, el: HTMLElement | null, name: string) => {
    if (!el) return;
    homeRef.current.set(card.key, { shell: el.cloneNode(true) as HTMLElement, rect: el.getBoundingClientRect(), cue: name });
  }, []);
  const deckKeys = deck.map((c) => c.key).join('\n');
  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll || homeRef.current.size === 0) return;
    const inDeck = new Set(deckKeys.split('\n'));
    for (const [key, home] of [...homeRef.current]) {
      if (inDeck.has(key)) continue;
      homeRef.current.delete(key);
      const row = scroll.querySelector<HTMLElement>(`[data-evkey="${esc(key)}"]`);
      const decision = gate && cards.find((c) => c.key === key);
      const name = decision && decision.form === 'await' ? (gate!.state(decision).decision === 'rejected' ? 'fail' : 'done') : home.cue;
      sendHome(scroll, home.shell, home.rect, row, () => cue(row, name, 1100));
    }
  }, [deckKeys, cards, gate]);

  const onSeen = useCallback(
    (card: StreamCard, el: HTMLElement | null) => {
      keepShell(card, el, 'seen');
      markSeen(card.key);
    },
    [keepShell, markSeen],
  );
  const onAnswer = useCallback((card: StreamCard, el: HTMLElement | null) => keepShell(card, el, 'done'), [keepShell]);

  const onToggle = useCallback((key: string) => setOpenKey((k) => (k === key ? null : key)), []);

  // Open a deck card's row in the stream: release what waits, scroll it in under the deck.
  const pendingOpen = useRef<string | null>(null);
  const onOpen = useCallback((card: StreamCard) => {
    setHolding(false);
    setOpenKey(card.key);
    pendingOpen.current = card.key;
  }, []);
  useEffect(() => {
    const key = pendingOpen.current;
    const scroll = scrollRef.current;
    if (!key || !scroll) return;
    const row = scroll.querySelector<HTMLElement>(`[data-evkey="${esc(key)}"]`);
    if (!row) return;
    pendingOpen.current = null;
    const deckH = deckBoxRef.current?.offsetHeight ?? 0;
    const top = scroll.scrollTop + row.getBoundingClientRect().top - scroll.getBoundingClientRect().top - deckH - 72;
    scroll.scrollTo({ top: Math.max(0, top), behavior: reducedMotion() ? 'auto' : 'smooth' });
  });

  // ── rows, in order, with their threads ──
  const flat = groups.flatMap((g) => g.cards);
  const indexOf = new Map(flat.map((c, i) => [c.key, i]));
  const threadAt = (i: number) => (i >= 0 && i < flat.length ? threadOf(flat[i]) : undefined);

  const held = cards.length - shown.length;
  const empty = cards.length === 0;

  return (
    <div
      ref={scrollRef}
      className={`evx-scroll ${props.className ?? ''}`}
      data-width={props.width ?? 'wide'}
      data-untimed={timed ? undefined : ''}
      onMouseOver={(e) => {
        const row = (e.target as HTMLElement).closest<HTMLElement>('[data-evkey]');
        setKin(row?.dataset.thread ?? null);
      }}
      onMouseLeave={() => setKin(null)}
    >
      <div ref={deckBoxRef} className="evx-deckbox">
        <NeedsYouDeck
          items={deck}
          scope={scope}
          resolve={resolve}
          gate={gate}
          onSeen={onSeen}
          onAnswer={onAnswer}
          onOpen={onOpen}
          onKin={setKin}
          crewOf={crewOf}
        />
      </div>
      {held > 0 && (
        <div className="evx-newpill">
          <button type="button" key={held} data-bump={held > 1 ? '' : undefined} onClick={toTop}>
            <ArrowUp size={12} aria-hidden />
            <Rolling value={held} /> new
          </button>
        </div>
      )}
      <div className="evx-stream">
        <NowHead live={props.live} sub={props.nowSub} timed={timed} />
        {empty ? (
          <div className="evx-empty">{props.emptyText}</div>
        ) : (
          groups.map((g, gi) => {
            const first = g.cards[0];
            const i0 = indexOf.get(first.key) ?? 0;
            const above = threadAt(i0 - 1);
            const joined = !!above && above === threadOf(first);
            const arrived = g.cards.every((c) => arrivedRef.current.has(c.key));
            return (
              <div key={g.bucket ?? `g${gi}`} className="evx-grp" data-arrived={arrived ? '' : undefined}>
                {g.bucket !== null && (
                  <Separator
                    bucket={g.bucket}
                    newest={g.newest}
                    first={gi === 0}
                    thread={joined ? crewOf(first) ?? 'rgb(var(--c-border))' : undefined}
                    delay={i0 < ENTRANCE_ROWS ? i0 * 40 : undefined}
                  />
                )}
                {g.cards.map((card) => {
                  const i = indexOf.get(card.key) ?? 0;
                  const t = threadOf(card);
                  const mark: Mark | undefined = frontier.has(card.key) ? 'running' : undefined;
                  return (
                    <EventRow
                      key={card.key}
                      card={card}
                      scope={scope}
                      resolved={resolveCached(resolve, card)}
                      mark={mark}
                      gateOutcome={card.form === 'await' ? (outcomes.get(card.key) as GateOutcome | undefined) : undefined}
                      gate={card.form === 'await' ? gate : undefined}
                      seen={(card.form === 'finding' || card.form === 'error') && isSeen(card.key)}
                      crew={crewOf(card)}
                      threadUp={!!t && threadAt(i - 1) === t}
                      threadDn={!!t && threadAt(i + 1) === t}
                      open={openKey === card.key}
                      arrived={arrivedRef.current.has(card.key)}
                      delay={!arrivedRef.current.has(card.key) && i < ENTRANCE_ROWS ? i * 40 + 60 : undefined}
                      timed={timed}
                      onToggle={onToggle}
                    />
                  );
                })}
              </div>
            );
          })
        )}
        {loadingOlder && <div className="evx-foot-note">Loading older events…</div>}
        {hasMoreOlder && !loadingOlder && !empty && onLoadOlder && (
          <button type="button" className="evx-older" onClick={onLoadOlder}>
            Load older events
          </button>
        )}
        {props.footer}
      </div>
    </div>
  );
}

/** An element's top within the scroll's content, from layout offsets. */
function layoutTop(el: HTMLElement, root: HTMLElement): number {
  let top = 0;
  let node: HTMLElement | null = el;
  while (node && node !== root) {
    top += node.offsetTop;
    const parent = node.offsetParent as HTMLElement | null;
    if (parent === root || !parent || !root.contains(parent)) {
      if (parent !== root) top -= root.offsetTop;
      break;
    }
    node = parent;
  }
  return top;
}

/** The vertical offset a running glide currently adds. */
function liveTranslateY(el: HTMLElement): number {
  const t = getComputedStyle(el).transform;
  if (!t || t === 'none') return 0;
  const m = /matrix\(([^)]+)\)/.exec(t);
  return m ? Number(m[1].split(',')[5]) || 0 : 0;
}

// Resolved context is a plain object per call; keep one per card so the
// memoised rows don't re-render on every pass.
const resolvedCache = new WeakMap<StreamCard, { fn: unknown; value: Resolved; key: string }>();
function resolveCached(fn: (c: StreamCard) => Resolved, card: StreamCard): Resolved {
  const value = fn(card);
  const key = `${value.project ?? ''}\u0000${value.workflow ?? ''}`;
  const hit = resolvedCache.get(card);
  if (hit && hit.key === key) return hit.value;
  resolvedCache.set(card, { fn, value, key });
  return value;
}

/** The shell of a deck card glides onto its row's card and fades as the row
 *  blooms; a row out of view gets the shell sliding away downward instead. */
function sendHome(scroll: HTMLElement, shell: HTMLElement, a: DOMRect, row: HTMLElement | null, done: () => void) {
  if (reducedMotion() || a.width === 0) {
    done();
    return;
  }
  for (const k of ['front', 'beyond', 'arrived', 'rising']) delete shell.dataset[k];
  shell.classList.add('evx-ghost');
  Object.assign(shell.style, {
    left: `${a.left}px`,
    top: `${a.top}px`,
    width: `${a.width}px`,
    height: `${a.height}px`,
    translate: 'none',
    scale: 'none',
    opacity: '1',
    animation: 'none',
    transform: 'translate(0px, 0px) scale(1, 1)',
    transitionDelay: '0ms',
  });
  document.body.appendChild(shell);
  void shell.offsetWidth;
  requestAnimationFrame(() => {
    const target = row?.querySelector<HTMLElement>('.evx-card') ?? row;
    const sr = scroll.getBoundingClientRect();
    const deckH = parseFloat(getComputedStyle(scroll).getPropertyValue('--deck-h')) || 0;
    const b = target?.getBoundingClientRect();
    const visible = !!b && b.height > 0 && b.top >= sr.top + deckH - 6 && b.bottom <= sr.bottom + 6;
    shell.dataset.go = '';
    if (visible && b) {
      shell.style.transform = `translate(${(b.left - a.left).toFixed(1)}px, ${(b.top - a.top).toFixed(1)}px) scale(${(b.width / a.width).toFixed(4)}, ${(b.height / a.height).toFixed(4)})`;
      setTimeout(() => {
        shell.style.opacity = '0';
        done();
      }, 360);
      setTimeout(() => shell.remove(), 560);
    } else {
      shell.style.transition = 'transform 280ms var(--ease-in-out), opacity 220ms ease';
      shell.style.transform = 'translate(0px, 36px) scale(0.98, 0.98)';
      shell.style.opacity = '0';
      setTimeout(() => {
        shell.remove();
        done();
      }, 320);
    }
  });
}

const NowHead = memo(function NowHead({ live, sub, timed }: { live: boolean; sub: string; timed: boolean }) {
  const now = useNow(1000);
  return (
    <div className="evx-now" data-quiet={live ? undefined : ''}>
      <i className="evx-rl evx-railx" aria-hidden />
      <div className="evx-t">{timed ? clock(now) : ''}</div>
      <div className="evx-mk">
        <span className="evx-nowmk">
          <i className="evx-ring" />
          <i className="evx-core" />
        </span>
      </div>
      <div className="evx-c">
        <span className="evx-lbl">now</span>
        <span className="evx-sub">{sub}</span>
      </div>
    </div>
  );
});

const Separator = memo(function Separator({
  bucket,
  newest,
  first,
  thread,
  delay,
}: {
  bucket: number;
  newest: number;
  first: boolean;
  thread?: string;
  delay?: number;
}) {
  const now = useNow(15_000);
  const age = now - newest;
  const text = first ? relAge(age) : relAge(Math.max(age, 60_000));
  const style = { ...(thread ? { '--crew': thread } : {}), ...(delay != null ? { '--delay': `${delay}ms` } : {}) } as React.CSSProperties;
  return (
    <div className="evx-sep" data-thread={thread ? '' : undefined} style={style}>
      <i className="evx-rl evx-railx" aria-hidden />
      <span className="evx-t">{clock(bucket).slice(0, 5)}</span>
      <span className="evx-mk" aria-hidden />
      <span className="evx-c">
        <span className="evx-rel">{text}</span>
        <i className="evx-hair" aria-hidden />
      </span>
    </div>
  );
});
