// What the Messages tab shows for one page of a space: participants, the
// query bar and kind pills, pinned directives, the legacy note, and the rows
// with day dividers. Filtering is one query (`lib/messageQuery`); the
// participant chips and kind pills are shortcuts that toggle an `involving:`
// or `kind:` term in it. Pure over its props; `MessageFeed` feeds it from the
// API and holds the query.

import { useEffect, useLayoutEffect, useMemo, useRef, type ReactNode } from 'react';
import { History } from 'lucide-react';
import type { SpaceMessage, SpaceParticipant } from '../../lib/api';
import { foldDirectives, KIND_FILTERS } from '../../lib/messages';
import { filterMessages, MESSAGE_FIELDS, messageFacets } from '../../lib/messageQuery';
import { soleValues, withTerm } from '../../lib/findingQuery/shortcut';
import { cn } from '../../lib/cn';
import { ListQueryBar } from '../query/ListQueryBar';
import { useWindowVirtualRows } from '../lists/useWindowVirtualRows';
import { DirectivesPanel } from './DirectivesPanel';
import { MessageRow } from './MessageRow';
import { ParticipantsBar } from './ParticipantsBar';

export type FeedItem = { type: 'divider'; key: string; label: string } | { type: 'msg'; key: string; m: SpaceMessage };

/** The rows, oldest first, with a divider at each new day. */
export function buildItems(messages: SpaceMessage[]): FeedItem[] {
  const items: FeedItem[] = [];
  let prevDay: string | null = null;
  for (const m of messages) {
    const day = new Date(m.ts).toDateString();
    if (day !== prevDay) {
      items.push({ type: 'divider', key: `d-${m.id}`, label: new Date(m.ts).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' }) });
      prevDay = day;
    }
    items.push({ type: 'msg', key: m.id, m });
  }
  return items;
}

export function DayDivider({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-3 py-1.5">
      <div className="h-px flex-1 bg-border" />
      <span className="text-meta font-medium text-ink-mute">{label}</span>
      <div className="h-px flex-1 bg-border" />
    </div>
  );
}

const sameSet = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((x) => b.includes(x));

/** Kind pills: each one selects its kinds in the query, counted over the whole log. */
export function KindPills({ messages, query, onQuery }: { messages: SpaceMessage[]; query: string; onQuery: (q: string) => void }) {
  const sole = soleValues(query, MESSAGE_FIELDS, 'kind');
  return (
    <div className="inline-flex flex-wrap items-center gap-1">
      {KIND_FILTERS.map((k) => {
        const n = messages.filter((m) => k.kinds.includes(m.kind)).length;
        const on = sole !== null && sameSet(sole, k.kinds);
        return (
          <button
            type="button"
            key={k.key}
            aria-pressed={on}
            data-testid={`kind-${k.key}`}
            onClick={() => onQuery(withTerm(query, MESSAGE_FIELDS, 'kind', on ? null : k.kinds))}
            className={cn(
              'rounded-full border px-2 py-0.5 text-meta font-medium transition-colors',
              on ? 'border-brand-500/50 bg-brand-50 text-brand-700' : 'border-border text-ink-dim hover:text-ink',
            )}
          >
            {k.label} <span className="tabular-nums opacity-70">{n}</span>
          </button>
        );
      })}
    </div>
  );
}

export function LegacyNote() {
  return (
    <div data-testid="legacy-note" className="flex items-center gap-1.5 rounded-lg border border-border bg-surface/50 px-3 py-1.5 text-meta text-ink-mute">
      <History size={12} />
      This flow predates the message log: board posts and directives only. Direct messages and broadcasts from then weren&apos;t kept.
    </div>
  );
}

/** Past this many rows only the ones near the viewport are mounted. */
export const WINDOW_THRESHOLD = 500;
/** How close to the bottom still counts as "at the bottom". */
const BOTTOM_SLACK_PX = 120;

/** The nearest ancestor that scrolls vertically. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if (overflowY === 'auto' || overflowY === 'scroll') return p;
  }
  return document.scrollingElement as HTMLElement | null;
}

export interface FeedViewProps {
  participants: SpaceParticipant[];
  messages: SpaceMessage[];
  legacy: boolean;
  /** The filter, in the message query language. */
  query: string;
  onQuery: (q: string) => void;
  /** How to open the sender's transcript at a message, when it can be. */
  openInTranscript?: (m: SpaceMessage) => (() => void) | null;
  /** The `#msg-` anchor: scrolled to and highlighted. */
  highlightId?: string | null;
  /** Above the participants bar (the refresh control). */
  toolbar?: ReactNode;
  /** Under the rows (the composer). */
  footer?: ReactNode;
}

export function FeedView({ participants, messages, legacy, query, onQuery, openInTranscript, highlightId, toolbar, footer }: FeedViewProps) {
  const addresses = useMemo(() => participants.map((p) => p.address), [participants]);
  const directives = useMemo(() => foldDirectives(messages), [messages]);
  const byId = useMemo(() => new Map(messages.map((m) => [m.id, m])), [messages]);
  const facets = useMemo(() => messageFacets(messages, participants), [messages, participants]);
  const { rows, error: queryError } = useMemo(() => filterMessages(messages, query, participants), [messages, query, participants]);
  const items = useMemo(() => buildItems(rows), [rows]);
  const involving = soleValues(query, MESSAGE_FIELDS, 'involving');
  const keys = useMemo(() => items.map((it) => it.key), [items]);
  const anchor = highlightId && keys.includes(highlightId) ? highlightId : null;
  const win = useWindowVirtualRows<HTMLDivElement, HTMLDivElement>(keys, keys.length > WINDOW_THRESHOLD, anchor);

  // Newest is at the bottom; follow it only while the reader is there. Only
  // the feed's own scroll container moves, never the page around it.
  const bottomRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useEffect(() => {
    const check = () => {
      const box = bottomRef.current && scrollParent(bottomRef.current);
      if (box) atBottom.current = box.scrollHeight - box.scrollTop - box.clientHeight <= BOTTOM_SLACK_PX;
    };
    document.addEventListener('scroll', check, { capture: true, passive: true });
    return () => document.removeEventListener('scroll', check, { capture: true });
  }, []);
  const scrolledTo = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (anchor) {
      if (scrolledTo.current === anchor) return;
      const el = document.getElementById(`msg-${anchor}`);
      if (!el) return;
      scrolledTo.current = anchor;
      atBottom.current = false;
      el.scrollIntoView?.({ block: 'center' });
      return;
    }
    const box = bottomRef.current && scrollParent(bottomRef.current);
    if (box && atBottom.current && items.length > 0) box.scrollTop = box.scrollHeight;
  }, [anchor, items.length]);

  const row = (it: FeedItem) =>
    it.type === 'divider' ? (
      <DayDivider label={it.label} />
    ) : (
      <MessageRow
        m={it.m}
        addresses={addresses}
        onOpenInTranscript={openInTranscript?.(it.m) ?? undefined}
        highlight={highlightId === it.m.id}
        retracted={it.m.refs?.retracts ? byId.get(it.m.refs.retracts) : undefined}
      />
    );

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-panel px-3 py-2 shadow-card">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <ParticipantsBar
              participants={participants}
              involving={involving?.length === 1 ? involving[0] : null}
              onInvolving={(address) => onQuery(withTerm(query, MESSAGE_FIELDS, 'involving', address ? [address] : null))}
            />
          </div>
          {toolbar}
        </div>
        <ListQueryBar
          value={query}
          onChange={onQuery}
          fields={MESSAGE_FIELDS}
          facets={facets}
          total={messages.length}
          shown={rows.length}
          error={queryError}
          className="mb-0"
          label="Filter messages"
          placeholder="Filter messages… e.g. from:pipit kind:direct is:undelivered"
        />
        <KindPills messages={messages} query={query} onQuery={onQuery} />
      </div>
      {legacy && <LegacyNote />}
      <DirectivesPanel directives={directives} addresses={addresses} />
      <div data-testid="feed-rows" className="rounded-xl border border-border bg-panel px-2 py-1 shadow-card">
        {items.length === 0 ? (
          <p className="px-2 py-6 text-center text-meta text-ink-mute">{query.trim() && messages.length > 0 ? 'No messages match the filter.' : 'No messages yet.'}</p>
        ) : (
          <div ref={win.bodyRef}>
            {win.slots.map((slot) =>
              slot.kind === 'spacer' ? (
                <div key={slot.key} aria-hidden style={{ height: slot.px }} />
              ) : (
                items.slice(slot.start, slot.end).map((it) => (
                  <div key={it.key} ref={win.measure(it.key)}>
                    {row(it)}
                  </div>
                ))
              ),
            )}
          </div>
        )}
      </div>
      {footer}
      <div ref={bottomRef} aria-hidden />
    </div>
  );
}
