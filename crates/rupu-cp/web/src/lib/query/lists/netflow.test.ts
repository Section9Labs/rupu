import { describe, expect, it } from 'vitest';
import { EMPTY_NETFLOW_FILTERS } from '../../netflow';
import {
  defaultWindow,
  defaultWindowText,
  netflowRequest,
  netflowWindowTerms,
  toggleNetflowValue,
  withNetflowWindow,
  withNetflowWindowTerms,
} from './netflow';

const NOW = Date.parse('2026-10-06T12:00:00Z');

describe('netflowRequest', () => {
  it('names no window for an empty query', () => {
    expect(netflowRequest('', NOW)).toEqual({ ok: true, window: 'default', range: {}, filters: EMPTY_NETFLOW_FILTERS });
  });

  it('resolves since: and until: spans, days and times against now', () => {
    expect(netflowRequest('since:2h until:30m', NOW)).toEqual({
      ok: true,
      window: 'query',
      range: { from: '2026-10-06T10:00:00.000Z', to: '2026-10-06T11:30:00.000Z' },
      filters: EMPTY_NETFLOW_FILTERS,
    });
    expect(netflowRequest('since:2026-10-01 until:2026-10-01', NOW)).toMatchObject({
      range: { from: '2026-10-01T00:00:00.000Z', to: '2026-10-01T23:59:59.999Z' },
    });
  });

  it("ORs a time term's values (the widest) and ANDs its terms (the narrowest)", () => {
    expect(netflowRequest('since:1h,3h since:2h', NOW)).toMatchObject({ range: { from: '2026-10-06T10:00:00.000Z' } });
    expect(netflowRequest('until:1h,3h until:2h', NOW)).toMatchObject({ range: { to: '2026-10-06T10:00:00.000Z' } });
  });

  it('fills the keep and leave-out lists from positive and negated terms', () => {
    const r = netflowRequest('workflow:a,b origin:provider:anthropic -org:unknown -org:as15169 -host:x.com:443', NOW);
    expect(r).toEqual({
      ok: true,
      window: 'default',
      range: {},
      filters: {
        ...EMPTY_NETFLOW_FILTERS,
        workflows: ['a', 'b'],
        origins: ['provider:anthropic'],
        not_orgs: ['unknown', 'as15169'],
        not_hosts: ['x.com:443'],
      },
    });
  });

  it('window:all asks for every flow; a since: wins over it', () => {
    expect(netflowRequest('window:all', NOW)).toMatchObject({ ok: true, window: 'all', range: {} });
    expect(netflowRequest('window:all since:1h', NOW)).toMatchObject({ window: 'query', range: { from: '2026-10-06T11:00:00.000Z' } });
  });

  it('refuses what the reads cannot take', () => {
    expect(netflowRequest('anthropic', NOW)).toEqual({
      ok: false,
      error: '`anthropic`: the network view takes keyed terms only (since:, until:, workflow:, origin:, org:, host:)',
    });
    expect(netflowRequest('-since:1h', NOW)).toEqual({ ok: false, error: "`-since:` can't be negated; use until:" });
    expect(netflowRequest('-window:all', NOW)).toEqual({ ok: false, error: "`-window:` can't be negated" });
    expect(netflowRequest('workflow:a workflow:b', NOW)).toEqual({
      ok: false,
      error: 'one `workflow:` term at a time; list several values with commas (workflow:a,b)',
    });
    expect(netflowRequest('since:soon', NOW)).toMatchObject({ ok: false });
  });
});

describe('defaultWindow', () => {
  const run = (started_at: string, finished_at: string | null) => ({ run_id: 'run_1', workflow: 'nightly', started_at, finished_at });

  it('is the last hour with no run', () => {
    expect(defaultWindow(NOW, { latest: null })).toEqual({ kind: 'hour', range: { from: '2026-10-06T11:00:00.000Z' } });
  });

  it("is the latest run's span, padded five minutes, when it ran under an hour", () => {
    const r = run('2026-10-03T09:00:00Z', '2026-10-03T09:40:00Z');
    expect(defaultWindow(NOW, { latest: r })).toEqual({
      kind: 'run',
      run: r,
      range: { from: '2026-10-03T08:55:00.000Z', to: '2026-10-03T09:45:00.000Z' },
    });
  });

  it('is the last hour when the latest run ran an hour or more', () => {
    expect(defaultWindow(NOW, { latest: run('2026-10-06T08:00:00Z', '2026-10-06T09:00:00Z') }).kind).toBe('hour');
  });

  it('measures a live run to now and leaves its window open', () => {
    const live = run('2026-10-06T11:30:00Z', null);
    expect(defaultWindow(NOW, { latest: live })).toEqual({ kind: 'run', run: live, range: { from: '2026-10-06T11:25:00.000Z', to: undefined } });
    expect(defaultWindow(NOW, { latest: run('2026-10-06T10:30:00Z', null) }).kind).toBe('hour');
  });

  it("is the run's own span on its tab, or every flow without one", () => {
    expect(defaultWindow(NOW, { span: { from: 'a', to: 'b' } })).toEqual({ kind: 'span', range: { from: 'a', to: 'b' } });
    expect(defaultWindow(NOW, { span: {} })).toEqual({ kind: 'all', range: {} });
  });

  it('says which default it is', () => {
    const live = defaultWindow(NOW, { latest: run('2026-10-06T11:30:00Z', null) });
    expect(defaultWindowText(live, NOW)).toBe(
      "Showing the latest run's span (nightly, run_1: 30 min, still running), the default: it is shorter than the last hour. since: or until: picks another window; window:all shows every flow.",
    );
    const orphan = defaultWindow(NOW, { latest: { run_id: null, workflow: null, started_at: '2026-10-06T11:58:00Z', finished_at: '2026-10-06T11:59:00Z' } });
    expect(defaultWindowText(orphan, NOW)).toMatch(/^Showing the latest run's span \(a run with no record: 1 min\)/);
  });
});

describe('toggleNetflowValue', () => {
  it('adds a value to the positive term, and drops it on a second click', () => {
    expect(toggleNetflowValue('', 'workflow', 'review')).toBe('workflow:review');
    expect(toggleNetflowValue('since:1h workflow:a', 'workflow', 'review')).toBe('since:1h workflow:a,review');
    expect(toggleNetflowValue('since:1h workflow:a,review', 'workflow', 'review')).toBe('since:1h workflow:a');
    expect(toggleNetflowValue('workflow:review', 'workflow', 'review')).toBe('');
  });

  it('keeps a value a negated term left out by moving it to the positive term', () => {
    expect(toggleNetflowValue('-host:a.com:443,b.com:443', 'host', 'a.com:443')).toBe('host:a.com:443 -host:b.com:443');
  });

  it('quotes a value the grammar must quote', () => {
    expect(toggleNetflowValue('', 'workflow', 'two words')).toBe('workflow:"two words"');
  });
});

describe('withNetflowWindow', () => {
  it('replaces the window terms with the zoomed one', () => {
    expect(withNetflowWindow('since:1h origin:scm:github window:all', '2026-10-06T11:10:00.000Z', '2026-10-06T11:20:00.000Z')).toBe(
      'origin:scm:github since:2026-10-06T11:10:00.000Z until:2026-10-06T11:20:00.000Z',
    );
  });
});

describe('netflowWindowTerms / withNetflowWindowTerms', () => {
  it('reads the window terms as written, and puts them back over every other term', () => {
    expect(netflowWindowTerms('since:2h origin:scm:github until:10m')).toEqual(['since:2h', 'until:10m']);
    expect(netflowWindowTerms('origin:scm:github')).toEqual([]);
    expect(withNetflowWindowTerms('origin:a since:2026-10-06T11:10:00.000Z until:2026-10-06T11:20:00.000Z host:b.com:443', ['since:2h'])).toBe(
      'origin:a host:b.com:443 since:2h',
    );
    expect(withNetflowWindowTerms('since:1h org:x', [])).toBe('org:x');
  });
});
