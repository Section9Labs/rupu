// Situation Room — the center live stream. A newest-first column of editorial
// EventCards merged from the SSE/history event firehose and the REST findings
// list. Filter chips narrow to Findings / Agent activity / Awaiting / Errors.
// Follows the top as new events land unless the operator scrolls down to read
// history; a "Load older events" sentinel pages the event backlog.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import type { CardGroup, StreamCard } from '../../lib/situationRoom/cards';
import EventCard from './EventCard';

type Filter = 'all' | CardGroup;

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'finding', label: 'Findings' },
  { key: 'activity', label: 'Agent activity' },
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
  resolve: (card: StreamCard) => { label?: string; branch?: string };
  onApprove: (runId: string) => Promise<void>;
  onReject: (runId: string) => Promise<void>;
  hasMoreOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
}) {
  const [filter, setFilter] = useState<Filter>('all');
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [follow, setFollow] = useState(true);

  const counts = useMemo(() => {
    const c = { finding: 0, await: 0, error: 0 };
    for (const card of cards) if (card.group in c) c[card.group as keyof typeof c] += 1;
    return c;
  }, [cards]);

  const shown = useMemo(
    () => (filter === 'all' ? cards : cards.filter((c) => c.group === filter)),
    [cards, filter],
  );

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
    <div className="flex-1 flex flex-col min-h-0 min-w-0">
      <div className="flex items-center gap-3 px-5 py-2.5 border-b border-border">
        <h2 className="text-ui tracking-[0.14em] uppercase text-ink-dim font-semibold m-0">Live stream</h2>
        <span className="text-note text-ink-mute tabular-nums font-mono">{cards.length} events</span>
        <div className="flex gap-1.5 ml-auto flex-wrap">
          {FILTERS.map((ff) => {
            const d = ff.key === 'finding' ? counts.finding : ff.key === 'await' ? counts.await : ff.key === 'error' ? counts.error : undefined;
            const active = filter === ff.key;
            return (
              <button
                key={ff.key}
                type="button"
                aria-pressed={active}
                onClick={() => setFilter(ff.key)}
                className={cn(
                  'text-note px-2.5 py-1 rounded-full border transition-colors',
                  active ? 'bg-ink/90 text-bg border-transparent' : 'border-border text-ink-dim hover:text-ink',
                )}
              >
                {ff.label}
                {d != null && <span className="ml-1 font-mono opacity-70">{d}</span>}
              </button>
            );
          })}
        </div>
      </div>

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-auto px-5 py-4">
        <div className="max-w-[820px] mx-auto flex flex-col gap-2.5">
          {shown.length === 0 ? (
            <div className="p-10 text-center text-note text-ink-dim">
              {cards.length === 0 ? 'Waiting for events…' : 'Nothing matches this filter.'}
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
              className="mx-auto my-2 text-note text-ink-dim hover:text-ink border border-border rounded-full px-4 py-1.5 transition-colors"
            >
              Load older events
            </button>
          )}
          {!hasMoreOlder && cards.length > 0 && (
            <div className="py-4 text-center text-meta text-ink-mute uppercase tracking-wide">Beginning of history</div>
          )}
        </div>
      </div>
    </div>
  );
}
