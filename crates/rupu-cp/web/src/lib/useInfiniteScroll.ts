import { useCallback, useEffect, useRef, useState } from 'react';

// useInfiniteScroll wires the consumer's bottom-of-list sentinel to a
// scroll-event listener on the nearest scrollable ancestor. When the sentinel
// is within `threshold` pixels of the visible bottom of that ancestor (or of
// the window if the page itself scrolls), and `hasMore` is true, and we aren't
// already loading, it calls `loadMore`.
//
// Scroll listener (not IntersectionObserver): IO is unreliable for inner
// overflow:auto containers; getBoundingClientRect + a scroll listener behaves
// identically in every browser. Overhead is negligible for the few mounted
// sentinels.

function findScrollAncestor(el: Element | null): HTMLElement | null {
  let p: Element | null = el?.parentElement ?? null;
  while (p) {
    const style = getComputedStyle(p);
    const ovr = style.overflowY;
    if (ovr === 'auto' || ovr === 'scroll' || ovr === 'overlay') {
      if (p.scrollHeight > p.clientHeight) {
        return p as HTMLElement;
      }
    }
    p = p.parentElement;
  }
  return null;
}

export function useInfiniteScroll({
  hasMore,
  loadMore,
  threshold = 240,
}: {
  hasMore: boolean;
  loadMore: () => Promise<unknown> | unknown;
  /** Pixels above the scroll-viewport bottom at which the sentinel triggers.
   *  Default 240 so the next page starts loading before the user hits the end. */
  threshold?: number;
}) {
  const [loading, setLoading] = useState(false);

  const sentinelElRef = useRef<HTMLElement | null>(null);
  const scrollerElRef = useRef<HTMLElement | null>(null);

  const loadMoreRef = useRef(loadMore);
  const hasMoreRef = useRef(hasMore);
  const loadingRef = useRef(loading);
  loadMoreRef.current = loadMore;
  hasMoreRef.current = hasMore;
  loadingRef.current = loading;

  const checkAndLoad = useCallback(() => {
    if (!hasMoreRef.current || loadingRef.current) return;
    const sentinel = sentinelElRef.current;
    if (!sentinel) return;

    const sRect = sentinel.getBoundingClientRect();
    const scroller = scrollerElRef.current;

    let bottomEdge: number;
    if (scroller) {
      bottomEdge = scroller.getBoundingClientRect().bottom;
    } else {
      bottomEdge = window.innerHeight || document.documentElement.clientHeight;
    }

    if (sRect.top - bottomEdge < threshold) {
      // Lock synchronously to prevent re-entry from a burst of scroll events
      // before setState propagates back to loadingRef.
      loadingRef.current = true;
      setLoading(true);
      Promise.resolve(loadMoreRef.current())
        .catch(() => {
          /* swallow — the user can scroll again to retry */
        })
        .finally(() => {
          loadingRef.current = false;
          setLoading(false);
          // New content may have pushed the sentinel back into view; re-check.
          requestAnimationFrame(() => checkAndLoad());
        });
    }
  }, [threshold]);

  const sentinelRef = useCallback(
    (el: HTMLDivElement | null) => {
      const oldScroller = scrollerElRef.current;
      if (oldScroller) {
        oldScroller.removeEventListener('scroll', checkAndLoad);
      } else if (sentinelElRef.current) {
        window.removeEventListener('scroll', checkAndLoad);
      }

      sentinelElRef.current = el;
      scrollerElRef.current = null;
      if (!el) return;

      const newScroller = findScrollAncestor(el);
      scrollerElRef.current = newScroller;
      if (newScroller) {
        newScroller.addEventListener('scroll', checkAndLoad, { passive: true });
      } else {
        window.addEventListener('scroll', checkAndLoad, { passive: true });
      }
      requestAnimationFrame(() => checkAndLoad());
    },
    [checkAndLoad],
  );

  useEffect(() => {
    if (hasMore && sentinelElRef.current) {
      requestAnimationFrame(() => checkAndLoad());
    }
  }, [hasMore, checkAndLoad]);

  useEffect(() => {
    return () => {
      const scroller = scrollerElRef.current;
      if (scroller) {
        scroller.removeEventListener('scroll', checkAndLoad);
      } else if (sentinelElRef.current) {
        window.removeEventListener('scroll', checkAndLoad);
      }
      sentinelElRef.current = null;
      scrollerElRef.current = null;
    };
  }, [checkAndLoad]);

  return { sentinelRef, loading };
}
