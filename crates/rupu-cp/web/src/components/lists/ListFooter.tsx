// The sentinel line under a paged list: scrolling it into view loads the
// next page. Pages keep it mounted whenever rows have loaded.
export function ListFooter({
  sentinelRef,
  loading,
  hasMore,
  ended,
  count,
}: {
  sentinelRef: (el: HTMLDivElement | null) => void;
  loading: boolean;
  hasMore: boolean;
  ended: boolean;
  count: number;
}) {
  const text = loading ? 'loading more…' : hasMore ? 'scroll for more' : ended ? `— end of ${count} —` : `${count} loaded`;
  return (
    <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
      {text}
    </div>
  );
}
