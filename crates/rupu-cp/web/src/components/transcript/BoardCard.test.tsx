// @vitest-environment jsdom
/**
 * Tests for BoardCard — the fleet coordination board view. Parser cases use
 * the real cellgw board payloads.
 */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { parseBoard, parseBoardLog, BoardSummary, BoardBody } from './BoardCard';
import type { ToolView } from './transcriptView';

afterEach(cleanup);

function tv(tool: string, overrides: Partial<ToolView>): ToolView {
  return { tool, kind: 'board', input: {}, ...overrides };
}

describe('parseBoard — claim / release', () => {
  it('claim granted', () => {
    const v = parseBoard(tv('board.claim', { input: { work_unit: 'host:39.124.191.61' }, output: 'granted: host:39.124.191.61' }));
    expect(v.verb).toBe('claim');
    expect(v.workUnit).toBe('host:39.124.191.61');
    expect(v.outcome).toEqual({ label: 'granted', tone: 'ok' });
  });

  it('release', () => {
    const v = parseBoard(tv('board.release', { input: { work_unit: 'host:84.232.96.77' }, output: 'released: host:84.232.96.77' }));
    expect(v.outcome).toEqual({ label: 'released', tone: 'neutral' });
  });

  it('contended claim is a warn outcome with the full text', () => {
    const v = parseBoard(tv('board.claim', { input: { work_unit: 'host:1.2.3.4' }, output: 'already held by pipit#8' }));
    expect(v.outcome).toEqual({ label: 'already held by pipit#8', tone: 'warn' });
  });

  it('errored claim', () => {
    const v = parseBoard(tv('board.claim', { input: { work_unit: 'host:1.2.3.4' }, error: 'boom' }));
    expect(v.outcome).toEqual({ label: 'boom', tone: 'err' });
  });
});

describe('parseBoardLog', () => {
  const log =
    '2026-10-15T18:17:23.646246+00:00 pipit#2 [note]: claimed batch_ab, adopting prior output.\n' +
    '2026-10-15T19:10:17.637203+00:00 salmon#3 [answer] -> lead: completed batch_ac: 194 live hosts.';

  it('parses timestamp, author, kind, addressee, body', () => {
    const entries = parseBoardLog(log);
    expect(entries).toHaveLength(2);
    expect(entries[0]).toEqual({ time: '18:17:23', who: 'pipit#2', kind: 'note', to: null, body: 'claimed batch_ab, adopting prior output.' });
    expect(entries[1]).toEqual({ time: '19:10:17', who: 'salmon#3', kind: 'answer', to: 'lead', body: 'completed batch_ac: 194 live hosts.' });
  });

  it('continuation lines append to the current post body', () => {
    const entries = parseBoardLog('2026-10-15T20:00:00.0+00:00 a#1 [note]: line one\nline two');
    expect(entries).toHaveLength(1);
    expect(entries[0].body).toBe('line one\nline two');
  });

  it('read with no posts is marked empty', () => {
    const v = parseBoard(tv('board.read', { input: { limit: 10 }, output: '(no posts)' }));
    expect(v.empty).toBe(true);
    expect(v.entries).toHaveLength(0);
  });
});

describe('render', () => {
  it('claim summary shows work unit + outcome', () => {
    const v = parseBoard(tv('board.claim', { input: { work_unit: 'host:10.0.0.1' }, output: 'granted: host:10.0.0.1' }));
    render(<BoardSummary view={v} />);
    expect(screen.getByText('host:10.0.0.1')).toBeTruthy();
    expect(screen.getByText('granted')).toBeTruthy();
  });

  it('read body renders a log feed, not JSON', () => {
    const v = parseBoard(tv('board.read', { input: { limit: 80 }, output: '2026-10-15T18:17:23.6+00:00 pipit#2 [note]: hello world' }));
    render(<BoardBody view={v} />);
    expect(screen.getByText('pipit#2')).toBeTruthy();
    expect(screen.getByText('hello world')).toBeTruthy();
    expect(screen.getByText('note')).toBeTruthy();
  });

  it('directive summary flags authority', () => {
    const v = parseBoard(tv('board.directive', { input: { body: 'stay off 10.0.0.0/8', to: 'broadcast' } }));
    render(<BoardSummary view={v} />);
    expect(screen.getByText('directive')).toBeTruthy();
  });
});
