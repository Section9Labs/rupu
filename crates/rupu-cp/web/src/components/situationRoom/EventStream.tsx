// Situation Room — the center live stream. A newest-first column of editorial
// EventCards merged from the SSE/history event firehose and the REST findings
// list, under the page's query line (the page filters the cards; this only
// lays them out). Follows the top as new events land unless the operator
// scrolls down to read history; a "Load older events" sentinel pages the
// event backlog.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import EventCard from './EventCard';

export default function EventStream({
  cards,
  filtered,
  queryBar,
  freshKeys,
  resolve,
  onApprove,
  onReject,
  hasMoreOlder,
  loadingOlder,
  onLoadOlder,
}: {
  /** The cards the query keeps, newest first. */
  cards: StreamCard[];
  /** Whether a query narrows them. */
  filtered: boolean;
  queryBar: ReactNode;
  freshKeys: ReadonlySet<string>;
  resolve: (card: StreamCard) => { label?: string; branch?: string; workflow?: string };
  onApprove: (runId: string) => Promise<void>;
  onReject: (runId: string) => Promise<void>;
  hasMoreOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [follow, setFollow] = useState(true);

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
        <div className="mb-2 flex items-center gap-3">
          <h2 className="m-0 text-ui font-semibold uppercase tracking-[0.14em] text-ink-dim">Live stream</h2>
          <span className="font-mono text-note tabular-nums text-ink-mute">{cards.length} events</span>
        </div>
        {queryBar}
      </div>

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-auto px-5 py-4">
        <div className="mx-auto flex max-w-[820px] flex-col">
          {cards.length === 0 ? (
            <div className="p-10 text-center text-note text-ink-dim">
              {filtered ? 'No events match the filter.' : 'Waiting for events…'}
            </div>
          ) : (
            cards.map((card) => {
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
