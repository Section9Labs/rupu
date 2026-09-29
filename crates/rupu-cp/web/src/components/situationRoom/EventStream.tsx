// Situation Room — the center live stream. A newest-first column of editorial
// EventCards merged from the SSE/history event firehose and the REST findings
// list. A search box + filter chips (Findings / Agent activity / Awaiting /
// Errors) narrow the stream via the pure `filterStreamCards`. Follows the top
// as new events land unless the operator scrolls down to read history; a
// "Load older events" sentinel pages the event backlog.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import type { StreamCard } from '../../lib/situationRoom/cards';
import { filterStreamCards, type StreamFilter } from '../../lib/situationRoom/filter';
import { SearchInput } from '../ui/SearchInput';
import EventCard from './EventCard';

const FILTERS: { key: StreamFilter; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'finding', label: 'Findings' },
  { key: 'activity', label: 'Activity' },
  { key: 'await', label: 'Awaiting' },
  { key: 'error', label: 'Errors' },
];

export default function EventStream({
  cards,
  freshKeys,
  resolve,
  onApprove,
  onReject,
  hasMoreOlder,
  loadingOlder,
  onLoadOlder,
}: {
  cards: StreamCard[];
  freshKeys: ReadonlySet<string>;
  resolve: (card: StreamCard) => { label?: string; branch?: string; workflow?: string };
  onApprove: (runId: string) => Promise<void>;
  onReject: (runId: string) => Promise<void>;
  hasMoreOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
}) {
  const [filter, setFilter] = useState<StreamFilter>('all');
  const [query, setQuery] = useState('');
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [follow, setFollow] = useState(true);

  const counts = useMemo(() => {
    const c: Record<string, number> = { finding: 0, await: 0, error: 0, activity: 0 };
    for (const card of cards) if (card.group in c) c[card.group] += 1;
    return c;
  }, [cards]);

  const shown = useMemo(() => filterStreamCards(cards, filter, query), [cards, filter, query]);

  // Pin to the top on new events while following.
  useLayoutEffect(() => {
    if (follow && scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [cards.length, follow]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      setFollow(el.scrollTop < 48);
      // Bottom sentinel → page older events.
      if (hasMoreOlder && !loadingOlder && el.scrollTop + el.clientHeight >= el.scrollHeight - 120) {
        onLoadOlder();
      }
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [hasMoreOlder, loadingOlder, onLoadOlder]);

  return (
    <div className="flex flex-1 flex-col min-h-0 min-w-0">
      <div className="border-b border-border px-5 py-2.5">
        <div className="flex items-center gap-3">
          <h2 className="m-0 text-ui font-semibold uppercase tracking-[0.14em] text-ink-dim">Live stream</h2>
          <span className="font-mono text-note tabular-nums text-ink-mute">{cards.length} events</span>
          <div className="ml-auto w-44 sm:w-56">
            <SearchInput value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search events…" />
          </div>
        </div>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {FILTERS.map((ff) => {
            const d = ff.key === 'all' ? undefined : counts[ff.key];
            const active = filter === ff.key;
            return (
              <button
                key={ff.key}
                type="button"
                aria-pressed={active}
                onClick={() => setFilter(ff.key)}
                className={cn(
                  'inline-flex items-center gap-1 rounded-md border px-2.5 py-1 text-meta font-medium transition-colors',
                  active
                    ? 'border-brand-500/40 bg-brand-500/10 text-brand-700'
                    : 'border-border text-ink-dim hover:border-ink-mute hover:text-ink',
                )}
              >
                {ff.label}
                {d != null && <span className="font-mono tabular-nums opacity-70">{d}</span>}
              </button>
            );
          })}
        </div>
      </div>

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-auto px-5 py-4">
        <div className="mx-auto flex max-w-[820px] flex-col gap-2.5">
          {shown.length === 0 ? (
            <div className="p-10 text-center text-note text-ink-dim">
              {cards.length === 0
                ? 'Waiting for events…'
                : query.trim()
                  ? `No events match "${query.trim()}".`
                  : 'Nothing matches this filter.'}
            </div>
          ) : (
            shown.map((card) => {
              const r = resolve(card);
              return (
                <EventCard
                  key={card.key}
                  card={card}
                  projectLabel={r.label}
                  branch={r.branch}
                  workflow={r.workflow}
                  fresh={freshKeys.has(card.key)}
                  onApprove={onApprove}
                  onReject={onReject}
                />
              );
            })
          )}
          {loadingOlder && <div className="py-4 text-center text-note text-ink-mute">Loading older events…</div>}
          {hasMoreOlder && !loadingOlder && cards.length > 0 && (
            <button
              type="button"
              onClick={onLoadOlder}
              className="mx-auto my-2 rounded-full border border-border px-4 py-1.5 text-note text-ink-dim transition-colors hover:border-ink-mute hover:text-ink"
            >
              Load older events
            </button>
          )}
          {!hasMoreOlder && cards.length > 0 && (
            <div className="py-4 text-center text-meta uppercase tracking-wide text-ink-mute">Beginning of history</div>
          )}
        </div>
      </div>
    </div>
  );
}
