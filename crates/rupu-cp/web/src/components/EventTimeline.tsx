// Global Live Events timeline — a polished, grouped, filterable feed adopting
// Okesu's EventTimeline UX (rolling-window grouping of repeated same-(type,
// run) events, lifecycle/error events that never group, a text filter, a
// bottom sentinel that lazy-loads older history). Consumed ONLY by the
// Events page (the global feed); per-run views keep using the flat
// EventTimelineList via RunEventFeed, unchanged.
//
// This component is presentational plumbing around a `SeqEvent[]` the caller
// already assembled (history + live, newest-first) — it owns filtering,
// grouping, day-sectioning, follow/jump-to-latest scrolling, and the lazy
// "load older" control, but no data fetching or SSE subscription of its own.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowUp, ChevronDown, ChevronRight, Radio, Search } from 'lucide-react';
import { cn } from '../lib/cn';
import { shortId } from '../lib/shortId';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';
import { isKnownRunEvent, type RunEvent } from '../lib/api';
import { titleFor, detailFor, labelFor, visualFor, UNKNOWN_VISUAL, type Visual } from './EventTimelineList';
import { Badge } from './ui/Badge';
import { type SeqEvent } from './RunEventFeed';

// Pixel distance from the top within which we consider the user "at the top"
// and resume following (matches the previous Events.tsx behavior).
const FOLLOW_THRESHOLD = 48;

// Rolling-window grouping: same-(type, run_id) events whose most recent
// member lands within this many ms of the next candidate merge into one
// collapsible row. 20s comfortably folds a for_each step's burst of
// unit_started/unit_completed events without merging across unrelated,
// widely-spaced activity.
const GROUP_WINDOW_MS = 20_000;

// Lifecycle and error-ish event types NEVER group, regardless of volume —
// each is significant enough on its own that folding it into a count-only
// row would hide signal a fleet operator needs.
const NEVER_GROUP_TYPES = new Set([
  'run_started',
  'run_completed',
  'run_failed',
  'step_failed',
  'step_awaiting_approval',
  'run_paused',
  'run_resumed',
]);

// --------------------------------------------------------------------------
// Pure helpers — exported for unit tests.
// --------------------------------------------------------------------------

/** The `ts` (unix-ms) stamped onto every event this component receives — see
 *  `TimedRunEvent` in lib/api.ts. Falls back to 0 (renders as "—") for a
 *  malformed/legacy row rather than throwing. */
export function tsOf(e: SeqEvent): number {
  const raw = e.event as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : 0;
}

function rowTitle(ev: RunEvent): string {
  return isKnownRunEvent(ev) ? titleFor(ev) : ev.type;
}

function rowDetail(ev: RunEvent): string | undefined {
  return isKnownRunEvent(ev) ? detailFor(ev) : undefined;
}

function rowVisual(ev: RunEvent): Visual {
  return isKnownRunEvent(ev) ? visualFor(ev) : UNKNOWN_VISUAL;
}

export function matchesFilter(ev: RunEvent, needle: string): boolean {
  if (!needle) return true;
  const n = needle.toLowerCase();
  const hay = [ev.type, ev.run_id, rowTitle(ev), rowDetail(ev)];
  return hay.some((h) => typeof h === 'string' && h.toLowerCase().includes(n));
}

export type TimelineRow =
  | { kind: 'single'; item: SeqEvent }
  | {
      kind: 'group';
      key: string;
      type: string;
      runId: string;
      /** Newest-first. */
      members: SeqEvent[];
      count: number;
      firstTs: number;
      lastTs: number;
    };

function rowTs(r: TimelineRow): number {
  return r.kind === 'single' ? tsOf(r.item) : r.lastTs;
}

/**
 * Rolling-window grouping, ported from Okesu's `EventTimeline.groupRollingWindow`
 * (simplified: no high-volume threshold / smart-promote — fleet-scale storm
 * detection isn't needed at rupu's per-workflow-run scale). Walks events
 * chronologically and merges consecutive same-(type, run_id) events whose
 * most-recent member is within `windowMs` of the event under consideration.
 * `NEVER_GROUP_TYPES` always emit as their own singleton row. Returned rows
 * are newest-first, matching the input ordering convention.
 */
export function groupEvents(events: SeqEvent[], windowMs: number = GROUP_WINDOW_MS): TimelineRow[] {
  const asc = [...events].sort((a, b) => tsOf(a) - tsOf(b));

  type Pending = { key: string; type: string; runId: string; members: SeqEvent[]; lastTs: number };
  const open: Pending[] = [];
  const closed: Pending[] = [];

  const closeOlderThan = (threshold: number) => {
    for (let i = open.length - 1; i >= 0; i--) {
      if (threshold - open[i].lastTs > windowMs) {
        closed.push(open[i]);
        open.splice(i, 1);
      }
    }
  };

  for (const e of asc) {
    const t = tsOf(e);
    closeOlderThan(t);
    const { type, run_id: runId } = e.event;
    if (NEVER_GROUP_TYPES.has(type)) {
      closed.push({ key: `single-${e.seq}`, type, runId, members: [e], lastTs: t });
      continue;
    }
    const key = `${type}|${runId}`;
    let g = open.find((p) => p.key === key);
    if (!g) {
      g = { key, type, runId, members: [], lastTs: t };
      open.push(g);
    }
    g.members.push(e);
    g.lastTs = t;
  }
  closed.push(...open);

  const rows: TimelineRow[] = closed.map((g) => {
    if (g.members.length === 1) {
      return { kind: 'single', item: g.members[0] };
    }
    return {
      kind: 'group',
      key: `${g.key}@${g.lastTs}`,
      type: g.type,
      runId: g.runId,
      members: [...g.members].sort((a, b) => tsOf(b) - tsOf(a)),
      count: g.members.length,
      firstTs: tsOf(g.members[0]),
      lastTs: g.lastTs,
    };
  });

  rows.sort((a, b) => rowTs(b) - rowTs(a));
  return rows;
}

function fmtTime(ts: number): string {
  if (!ts) return '—';
  return new Date(ts).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  });
}

function fmtFull(ts: number): string {
  return ts ? new Date(ts).toLocaleString() : '';
}

function formatSpan(firstTs: number, lastTs: number): string {
  const sec = Math.max(0, Math.round((lastTs - firstTs) / 1000));
  if (sec < 60) return `${sec}s`;
  return `${Math.round(sec / 60)}m`;
}

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

function dayLabel(ts: number): string {
  const d = new Date(ts);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (sameDay(d, today)) return 'Today';
  if (sameDay(d, yesterday)) return 'Yesterday';
  return d.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
}

function groupRowsByDay(rows: TimelineRow[]): Array<{ label: string; items: TimelineRow[] }> {
  const groups: Array<{ label: string; items: TimelineRow[] }> = [];
  for (const r of rows) {
    const label = dayLabel(rowTs(r));
    let g = groups.find((x) => x.label === label);
    if (!g) {
      g = { label, items: [] };
      groups.push(g);
    }
    g.items.push(r);
  }
  return groups;
}

// --------------------------------------------------------------------------
// Component
// --------------------------------------------------------------------------

export interface EventTimelineProps {
  /** Newest-first. History (from `GET /api/events`) plus live SSE frames the
   *  caller has already merged (prepending new arrivals, appending
   *  lazy-loaded older pages). */
  events: SeqEvent[];
  /** `seq`s of recently-arrived live events — drives the tick-in animation. */
  liveIDs: ReadonlySet<number>;
  /** Whether an older page is known to exist (drives the sentinel/control). */
  hasMoreOlder: boolean;
  /** Whether a `loadOlder` fetch is in flight. */
  loadingOlder: boolean;
  /** Fetch and append the next older page (cursor by the oldest loaded `ts`). */
  onLoadOlder: () => void | Promise<void>;
  className?: string;
  /** Override the empty-state hint shown when there is truly no history. */
  emptyHint?: string;
}

export default function EventTimeline({
  events,
  liveIDs,
  hasMoreOlder,
  loadingOlder,
  onLoadOlder,
  className,
  emptyHint,
}: EventTimelineProps) {
  const [filter, setFilter] = useState('');
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [follow, setFollow] = useState(true);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(
    () => (filter ? events.filter((e) => matchesFilter(e.event, filter)) : events),
    [events, filter],
  );
  const rows = useMemo(() => groupEvents(filtered), [filtered]);
  const grouped = useMemo(() => groupRowsByDay(rows), [rows]);

  const { sentinelRef } = useInfiniteScroll({ hasMore: hasMoreOlder, loadMore: onLoadOlder });

  // Pin to the top of the container when following (new events prepend).
  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [events.length, follow]);

  // Detect scroll-away-from-top (pause follow) and scroll-back-to-top (resume).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => setFollow(el.scrollTop < FOLLOW_THRESHOLD);
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, []);

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  return (
    <div className={cn('relative flex-1 min-h-0 flex flex-col', className)}>
      <div className="mb-3 flex items-center gap-3">
        <div className="relative w-full max-w-xs">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-mute pointer-events-none" />
          <input
            type="search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter events…"
            aria-label="Filter events"
            className="w-full pl-7 pr-2.5 py-1.5 text-xs rounded-md border border-border bg-panel text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-500/30"
          />
        </div>
        {filter && (
          <span className="text-meta text-ink-mute tabular-nums shrink-0">
            {filtered.length} of {events.length}
          </span>
        )}
      </div>

      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-border bg-panel shadow-card px-4 py-3"
      >
        {rows.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-ink-mute">
            <Radio size={24} className="mb-3 opacity-40" />
            <p className="text-sm">
              {events.length === 0 ? 'No events yet.' : 'No events match your filter.'}
            </p>
            {events.length === 0 && (
              <p className="text-xs mt-1 text-center max-w-sm">
                {emptyHint ?? 'Runs will post their step-by-step events here as they execute.'}
              </p>
            )}
          </div>
        ) : (
          <div className="relative">
            <div className="absolute left-[19px] top-0 bottom-0 w-px bg-border" aria-hidden />
            {grouped.map((day) => (
              <section key={day.label} className="mb-5">
                <div className="sticky top-0 z-10 mb-2">
                  <span className="inline-block bg-panel border border-border text-[11px] uppercase tracking-wide font-medium text-ink-mute px-2 py-0.5 rounded-md ml-2">
                    {day.label}
                  </span>
                </div>
                <ol className="timeline-list space-y-0.5">
                  {day.items.map((row) =>
                    row.kind === 'single' ? (
                      <TimelineRowItem key={`s-${row.item.seq}`} item={row.item} live={liveIDs.has(row.item.seq)} />
                    ) : (
                      <TimelineGroupRow
                        key={row.key}
                        row={row}
                        expanded={expanded.has(row.key)}
                        onToggle={() => toggle(row.key)}
                        liveIDs={liveIDs}
                      />
                    ),
                  )}
                </ol>
              </section>
            ))}
          </div>
        )}

        {events.length > 0 && (
          <div ref={sentinelRef} className="flex items-center justify-center pt-2 pb-1 min-h-[28px]">
            {loadingOlder ? (
              <span className="text-meta text-ink-mute">Loading older events…</span>
            ) : hasMoreOlder ? (
              <button
                type="button"
                onClick={() => onLoadOlder()}
                className="text-meta text-ink-mute hover:text-ink-dim hover:underline"
              >
                Load older events
              </button>
            ) : (
              <span className="text-meta text-ink-mute">— end of history ({events.length} events) —</span>
            )}
          </div>
        )}
      </div>

      {!follow && events.length > 0 && (
        <button
          onClick={() => {
            scrollRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
            setFollow(true);
          }}
          className="absolute bottom-4 right-4 inline-flex items-center gap-1.5 bg-brand-600 hover:bg-brand-700 text-white text-xs font-medium px-3 py-2 rounded-full shadow-card"
        >
          <ArrowUp size={12} />
          Jump to latest
        </button>
      )}
    </div>
  );
}

// --------------------------------------------------------------------------
// Rows
// --------------------------------------------------------------------------

function RunLink({ runId }: { runId: string }) {
  return (
    <Link
      to={`/runs/${encodeURIComponent(runId)}`}
      className="ml-auto shrink-0 text-meta font-mono text-ink-mute hover:text-brand-600 hover:underline"
      title={`Open run ${runId}`}
      onClick={(e) => e.stopPropagation()}
    >
      {shortId(runId)}
    </Link>
  );
}

function TimelineRowItem({ item, live }: { item: SeqEvent; live: boolean }) {
  const ev = item.event;
  const v = rowVisual(ev);
  const Icon = v.icon;
  const title = rowTitle(ev);
  const detail = rowDetail(ev);
  const ts = tsOf(item);
  return (
    <li
      className={cn(
        'timeline-item relative flex gap-3 pl-1 pr-1 py-1.5 rounded-md hover:bg-surface/60 transition-colors',
        live && 'is-fresh',
      )}
    >
      <time className="w-16 shrink-0 pt-0.5 text-meta font-mono text-ink-mute text-right" title={fmtFull(ts)}>
        {fmtTime(ts)}
      </time>
      <span
        className={cn(
          'timeline-dot relative z-10 flex h-6 w-6 shrink-0 items-center justify-center rounded-full ring-1',
          v.ring,
          live && 'is-live',
        )}
      >
        {live && (
          <span className={cn('timeline-dot-ring absolute inline-block w-6 h-6 rounded-full', v.ring)} aria-hidden />
        )}
        <Icon size={12} className={cn(v.iconColor, v.spin && 'animate-spin')} />
      </span>
      <div className="min-w-0 flex-1 pt-0.5">
        <div className="flex items-baseline gap-2 flex-wrap">
          <span className="text-xs font-medium text-ink truncate">{title}</span>
          <span className="text-meta uppercase tracking-wide text-ink-mute shrink-0">{labelFor(ev)}</span>
          <RunLink runId={ev.run_id} />
        </div>
        {detail && <div className="text-note text-ink-dim mt-0.5 break-words">{detail}</div>}
      </div>
    </li>
  );
}

function TimelineGroupRow({
  row,
  expanded,
  onToggle,
  liveIDs,
}: {
  row: Extract<TimelineRow, { kind: 'group' }>;
  expanded: boolean;
  onToggle: () => void;
  liveIDs: ReadonlySet<number>;
}) {
  const newest = row.members[0];
  const ev = newest.event;
  const v = rowVisual(ev);
  const Icon = v.icon;
  const title = rowTitle(ev);
  // A <div role="button">, not a real <button> — the row also hosts a
  // RunLink (<a>), and nesting interactive content inside a <button> is
  // invalid HTML (and breaks the link's own click). role="button" +
  // tabIndex + onKeyDown keep it keyboard-toggleable.
  return (
    <li>
      <div
        role="button"
        tabIndex={0}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onToggle();
          }
        }}
        className={cn(
          'w-full text-left flex gap-3 pl-1 pr-1 py-1.5 rounded-md hover:bg-surface/60 transition-colors cursor-pointer',
          expanded && 'bg-surface/40',
        )}
      >
        <time className="w-16 shrink-0 pt-0.5 text-meta font-mono text-ink-mute text-right" title={fmtFull(row.lastTs)}>
          {fmtTime(row.lastTs)}
        </time>
        <span className={cn('relative z-10 flex h-6 w-6 shrink-0 items-center justify-center rounded-full ring-1', v.ring)}>
          <Icon size={12} className={v.iconColor} />
        </span>
        <div className="min-w-0 flex-1 pt-0.5">
          <div className="flex items-baseline gap-2 flex-wrap">
            {expanded ? (
              <ChevronDown size={12} className="text-ink-mute shrink-0" />
            ) : (
              <ChevronRight size={12} className="text-ink-mute shrink-0" />
            )}
            <span className="text-xs font-medium text-ink truncate">{title}</span>
            <Badge tone="neutral" className="tabular-nums">×{row.count}</Badge>
            <span className="text-meta uppercase tracking-wide text-ink-mute shrink-0">{labelFor(ev)}</span>
            <RunLink runId={row.runId} />
          </div>
          <div className="text-note text-ink-mute mt-0.5">
            {row.count} occurrences over {formatSpan(row.firstTs, row.lastTs)} · {expanded ? 'collapse' : 'expand'}
          </div>
        </div>
      </div>
      {expanded && (
        <ul className="ml-[76px] mt-0.5 mb-1.5 space-y-0.5 border-l-2 border-border pl-3">
          {row.members.map((m) => (
            <TimelineRowItem key={`g-${m.seq}`} item={m} live={liveIDs.has(m.seq)} />
          ))}
        </ul>
      )}
    </li>
  );
}
