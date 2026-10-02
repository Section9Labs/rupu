// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { emptySlice, type HostSlice } from '../../lib/perHost/types';
import { PagingFailures, PerHostFooter, PerHostStrip, perHostFooterText } from './PerHostStatus';

afterEach(cleanup);

const s = (id: string, over: Partial<HostSlice<unknown>> = {}): HostSlice<unknown> => ({
  ...emptySlice({ id, name: id, transport_kind: 'ssh' }),
  state: 'ok',
  ...over,
});

describe('PerHostStrip', () => {
  it('renders one chip per host, and nothing for a single host', () => {
    const { container, rerender } = render(<PerHostStrip slices={[s('local')]} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<PerHostStrip slices={[s('local'), s('mini', { state: 'loading' })]} />);
    expect(screen.getByText('mini')).toBeInTheDocument();
    expect(screen.getByText('loading…')).toBeInTheDocument();
  });
});

describe('perHostFooterText', () => {
  const base = { loading: false, hasMore: false, ended: true, count: 7 };
  it('ends honestly, naming hosts that were not included', () => {
    expect(perHostFooterText({ ...base, slices: [s('local')] })).toBe('— end of 7 —');
    expect(perHostFooterText({ ...base, slices: [s('local'), s('mini', { state: 'offline' })] })).toBe(
      '— end of 7 — · not included: mini (offline)',
    );
  });
  it('says who it is waiting on instead of claiming the end', () => {
    expect(perHostFooterText({ ...base, ended: false, slices: [s('local'), s('kuki', { state: 'loading' })] })).toBe(
      'waiting on kuki…',
    );
  });
  it('leaves the waiting line blank when the page already shows it, but only that line', () => {
    const waiting = [s('local'), s('kuki', { state: 'loading' })];
    expect(perHostFooterText({ ...base, ended: false, waitingShown: true, slices: waiting })).toBe('');
    // Everything else the footer says is still said.
    expect(perHostFooterText({ ...base, ended: false, hasMore: true, waitingShown: true, slices: waiting })).toBe(
      'scroll for more',
    );
    expect(perHostFooterText({ ...base, loading: true, waitingShown: true, slices: waiting })).toBe('loading more…');
    expect(perHostFooterText({ ...base, waitingShown: true, slices: [s('local')] })).toBe('— end of 7 —');
  });
  it('does not claim the end while a host\'s older rows could not load', () => {
    expect(perHostFooterText({ ...base, slices: [s('local'), s('mini', { pagingFailed: true })] })).toBe('7 loaded');
    expect(
      perHostFooterText({
        ...base,
        slices: [s('local', { pagingFailed: true }), s('kuki', { state: 'offline' })],
      }),
    ).toBe('7 loaded · not included: kuki (offline)');
    // Another host can still page: scrolling is still the honest cue.
    expect(perHostFooterText({ ...base, hasMore: true, ended: false, slices: [s('mini', { pagingFailed: true })] })).toBe(
      'scroll for more',
    );
  });
  it('keeps the existing loading / scroll copy', () => {
    expect(perHostFooterText({ ...base, loading: true, slices: [] })).toBe('loading more…');
    expect(perHostFooterText({ ...base, hasMore: true, ended: false, slices: [] })).toBe('scroll for more');
  });
});

describe('PerHostFooter', () => {
  it('keeps the sentinel mounted when the page leaves the line blank, so paging keeps working', () => {
    const sentinelRef = vi.fn();
    render(<PerHostFooter sentinelRef={sentinelRef} text="" slices={[s('local')]} onRetry={vi.fn()} />);
    expect(sentinelRef).toHaveBeenCalledWith(expect.any(HTMLDivElement));
  });
});

describe('PagingFailures', () => {
  it('offers a retry per failed host', () => {
    const onRetry = vi.fn();
    render(<PagingFailures slices={[s('mini', { pagingFailed: true })]} onRetry={onRetry} />);
    expect(screen.getByText(/older rows from mini couldn't load/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry mini' }));
    expect(onRetry).toHaveBeenCalledWith('mini');
  });
});
