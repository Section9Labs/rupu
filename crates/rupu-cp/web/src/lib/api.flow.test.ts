import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, roundEvents, type AgentiflowEvent, type BoardView } from './api';

afterEach(() => vi.restoreAllMocks());

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}

const BOARD: BoardView = {
  claims: [
    {
      key: 'host:10.0.0.5',
      key_known: true,
      owner: 'recon#1',
      acquired_at: '2026-10-21T10:00:00Z',
      lease_expires_at: '2026-10-21T11:00:00Z',
      ended_at: null,
      state: 'held',
    },
  ],
  directives: [],
  journal: true,
};

describe('agentiflow flow api', () => {
  it('roundEvents_types_old_and_new_events', () => {
    const events: AgentiflowEvent[] = [
      { kind: 'run_started', ts: '2026-10-21T09:00:00Z' },
      {
        kind: 'round', ts: '2026-10-21T09:05:00Z', round: 1, goals_met: 0, goals_total: 2,
        spent_usd: 0.1, spent_tokens: 100, outcome: 'ok',
      },
      {
        kind: 'round', ts: '2026-10-21T09:10:00Z', round: 2, started_at: '2026-10-21T09:06:00Z',
        decision: 'probe auth', goals: [{ id: 'g1', met: false, current: 1 }],
        warnings: ['slow'], goals_met: 1, goals_total: 2, spent_usd: 0.2, spent_tokens: 200,
        outcome: 'ok',
      },
    ];
    const rounds = roundEvents(events);
    expect(rounds.map((r) => r.round)).toEqual([1, 2]);
    expect(rounds[0].decision).toBeUndefined();
    expect(rounds[0].started_at).toBeUndefined();
    expect(rounds[0].goals).toBeUndefined();
    expect(rounds[1].decision).toBe('probe auth');
    expect(rounds[1].goals?.[0]).toEqual({ id: 'g1', met: false, current: 1 });
    expect(rounds[1].warnings).toEqual(['slow']);
  });

  it('getRunBoard_turns_501_into_unavailable', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ error: 'tunnel host' }, 501));
    const res = await api.getRunBoard('run_1', { host: 'h1' });
    expect(res).toEqual({ kind: 'unavailable', reason: 'tunnel host' });
  });

  it('getRunBoard_passes_host', async () => {
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(BOARD));
    const res = await api.getRunBoard('run_1', { host: 'h 1' });
    expect(res).toEqual({ kind: 'ok', board: BOARD });
    expect(String(spy.mock.calls[0][0])).toContain('/api/runs/run_1/board?host=h%201');
  });

  it('getAgentiflowBoard_and_generated_hit_their_routes', async () => {
    const spy = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse(BOARD))
      .mockResolvedValueOnce(jsonResponse({ handle: 'h', name: 'n', file_name: 'n.yaml', yaml: 'a: 1' }));
    expect(await api.getAgentiflowBoard('af_1')).toEqual({ kind: 'ok', board: BOARD });
    const g = await api.getAgentiflowGenerated('af_1', 'h#1');
    expect(g.yaml).toBe('a: 1');
    expect(String(spy.mock.calls[0][0])).toContain('/api/agentiflows/af_1/board');
    expect(String(spy.mock.calls[1][0])).toContain('/api/agentiflows/af_1/generated/h%231');
  });
});
