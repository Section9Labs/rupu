// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { usePagedList } from './usePagedList';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/** Mounts the sentinel so `useInfiniteScroll` can auto-drive pagination —
 *  exercises the real scroll-sentinel wiring, not a stand-in. */
function ScrollHarness({
  fetch,
}: {
  fetch: (p: { offset: number; limit: number }) => Promise<string[]>;
}) {
  const { rows, ended, hasMore, sentinelRef } = usePagedList({ fetch, deps: [] });
  return (
    <div>
      <div data-testid="count">{rows.length}</div>
      <div data-testid="ended">{String(ended)}</div>
      <div data-testid="hasMore">{String(hasMore)}</div>
      <div ref={sentinelRef} data-testid="sentinel" />
    </div>
  );
}

function ResetHarness({
  source,
  deps,
  onCall,
}: {
  source: string[];
  deps: unknown[];
  onCall: () => void;
}) {
  // A fresh closure every render, by construction — this is the exact shape
  // that has caused fresh-object dep loops elsewhere in this codebase; the
  // hook must key off `deps` alone, never off this identity.
  const fetchFn = async (_p: { offset: number; limit: number }) => {
    onCall();
    return source;
  };
  const { rows, loading, error } = usePagedList({ fetch: fetchFn, deps });
  return (
    <div>
      <div data-testid="rows">{rows.join(',')}</div>
      <div data-testid="loading">{String(loading)}</div>
      <div data-testid="error">{error ?? ''}</div>
    </div>
  );
}

function PollHarness({
  fetch,
  poll,
}: {
  fetch: (p: { offset: number; limit: number }) => Promise<string[]>;
  poll?: boolean;
}) {
  const { rows } = usePagedList({ fetch, deps: [], poll });
  return <div data-testid="rows">{rows.join(',')}</div>;
}

describe('usePagedList', () => {
  it('appends pages via the infinite-scroll sentinel until a short page ends the list', async () => {
    const dataset = Array.from({ length: 45 }, (_, i) => `r${i}`);
    const fetchFn = async ({ offset, limit }: { offset: number; limit: number }) =>
      dataset.slice(offset, offset + limit);

    render(<ScrollHarness fetch={fetchFn} />);

    await waitFor(() => expect(screen.getByTestId('count').textContent).toBe('45'), {
      timeout: 3000,
    });
    expect(screen.getByTestId('ended').textContent).toBe('true');
    expect(screen.getByTestId('hasMore').textContent).toBe('false');
  });

  // REGRESSION (coordinator review, Task B fix 1): `useInfiniteScroll` fires
  // its rAF-driven `checkAndLoad()` as soon as the sentinel mounts, gated
  // only on its own internal refs — nothing tells it the initial fetch is
  // still in flight. A consumer that mounts the sentinel unconditionally
  // (this harness does — no `hidden while loading` caller discipline) while
  // page 0 is still resolving used to fire a SECOND concurrent offset-0
  // fetch, which appended (not replaced) on top of the first and
  // permanently duplicated page 0. Confirmed this test FAILS (settles at 45
  // rows, not 25) against the hook with a bare `hasMore` passed to
  // `useInfiniteScroll`, and PASSES with `hasMore && !loading`.
  it('does not duplicate page 0 when the sentinel mounts while the initial fetch is still in flight', async () => {
    const dataset = Array.from({ length: 25 }, (_, i) => `r${i}`);
    const fetchFn = ({ offset, limit }: { offset: number; limit: number }) =>
      new Promise<string[]>((resolve) => {
        setTimeout(() => resolve(dataset.slice(offset, offset + limit)), 25);
      });

    render(<ScrollHarness fetch={fetchFn} />);

    await waitFor(() => expect(screen.getByTestId('ended').textContent).toBe('true'), {
      timeout: 3000,
    });
    // The buggy hook settles at 45 (20 duplicated + 20 + 5 remainder short
    // page) — it never self-corrects once the duplicate lands.
    expect(screen.getByTestId('count').textContent).toBe('25');
  });

  it('resets rows and re-fetches from offset 0 when deps changes (by index)', async () => {
    const onCall = vi.fn();
    const { rerender } = render(
      <ResetHarness source={['x']} deps={[1]} onCall={onCall} />,
    );
    await waitFor(() => expect(screen.getByTestId('rows').textContent).toBe('x'));
    expect(onCall).toHaveBeenCalledTimes(1);

    // A fresh `fetch` closure (source changed) but the SAME `deps` must NOT
    // trigger a refetch — this is the guard against the fresh-object loop.
    rerender(<ResetHarness source={['y']} deps={[1]} onCall={onCall} />);
    await new Promise((r) => setTimeout(r, 20));
    expect(onCall).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('rows').textContent).toBe('x');

    // Changing `deps` resets rows and re-fetches, reading the LATEST closure.
    rerender(<ResetHarness source={['y']} deps={[2]} onCall={onCall} />);
    await waitFor(() => expect(screen.getByTestId('rows').textContent).toBe('y'));
    expect(onCall).toHaveBeenCalledTimes(2);
  });

  it('captures a fetch rejection as `error` without throwing', async () => {
    const fetchFn = async (): Promise<string[]> => {
      throw new Error('boom');
    };
    render(<ResetHarness2Wrapper fetch={fetchFn} />);
    await waitFor(() => expect(screen.getByTestId('err').textContent).toBe('boom'));
  });

  it('polls page 0 every 5s and splices it back in when poll is true', async () => {
    vi.useFakeTimers();
    const fetchSpy = vi.fn(async () => ['a']);

    render(<PollHarness fetch={fetchSpy} poll />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('rows').textContent).toBe('a');

    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(fetchSpy).toHaveBeenCalledTimes(2);

    await act(async () => {
      vi.advanceTimersByTime(10000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(fetchSpy).toHaveBeenCalledTimes(4);
  });

  it('never polls when poll is false (the default)', async () => {
    vi.useFakeTimers();
    const fetchSpy = vi.fn(async () => ['a']);

    render(<PollHarness fetch={fetchSpy} />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(fetchSpy).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(20000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });
});

// Small helper harness for the rejection test — kept separate so it doesn't
// force an unused `fetch` param shape onto `ResetHarness`.
function ResetHarness2Wrapper({
  fetch,
}: {
  fetch: (p: { offset: number; limit: number }) => Promise<string[]>;
}) {
  const { error } = usePagedList({ fetch, deps: [1] });
  return <div data-testid="err">{error ?? ''}</div>;
}
