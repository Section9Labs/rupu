import { describe, it, expect, vi, afterEach } from 'vitest';
import { ApiError } from '../api';
import { compactionLine, isAgentRunRecord, isRetryable, modeLabel, runsApi } from './runs';

afterEach(() => vi.restoreAllMocks());

function stubFetch(body: unknown, status = 202) {
  return vi
    .spyOn(globalThis, 'fetch')
    .mockImplementation(async () => new Response(JSON.stringify(body), { status }));
}

describe('runsApi.retryRun', () => {
  it('POSTs restart_interrupted and no mode when the recorded mode is kept', async () => {
    const f = stubFetch({ run_id: 'run_1', retrying: true, restart_interrupted: false, host_id: 'local' });
    const res = await runsApi.retryRun('run_1', { restartInterrupted: false });
    const [url, init] = f.mock.calls[0];
    expect(url).toBe('/api/runs/run_1/retry');
    expect(init?.method).toBe('POST');
    expect(init?.body).toBe('{"restart_interrupted":false}');
    expect(res.retrying).toBe(true);
  });

  it('sends a chosen mode and the restart flag', async () => {
    const f = stubFetch({ run_id: 'run_1', retrying: true, restart_interrupted: true, host_id: 'local' });
    await runsApi.retryRun('run_1', { mode: 'bypass', restartInterrupted: true });
    expect(f.mock.calls[0][1]?.body).toBe('{"restart_interrupted":true,"mode":"bypass"}');
  });

  it('escapes the run id', async () => {
    const f = stubFetch({});
    await runsApi.retryRun('run a/b', {});
    expect(f.mock.calls[0][0]).toBe('/api/runs/run%20a%2Fb/retry');
  });

  it('throws the 409 as an ApiError carrying the server message', async () => {
    stubFetch({ error: 'run run_1 is `running`: only a failed, rejected or cancelled run can be retried' }, 409);
    await expect(runsApi.retryRun('run_1', {})).rejects.toBeInstanceOf(ApiError);
  });
});

describe('runsApi.continueAgentRun', () => {
  it('POSTs an empty body when nothing changes', async () => {
    const f = stubFetch({ run_id: 'run_2', continues: 'run_1', host_id: 'local' });
    const res = await runsApi.continueAgentRun('run_1', {});
    const [url, init] = f.mock.calls[0];
    expect(url).toBe('/api/runs/agents/run_1/continue');
    expect(init?.method).toBe('POST');
    expect(init?.body).toBe('{}');
    expect(res.run_id).toBe('run_2');
  });

  it('sends only the non-blank model and provider, trimmed', async () => {
    const f = stubFetch({ run_id: 'run_2', continues: 'run_1', host_id: 'local' });
    await runsApi.continueAgentRun('run_1', { model: ' claude-sonnet-5-5 ', provider: '  ' });
    expect(f.mock.calls[0][1]?.body).toBe('{"model":"claude-sonnet-5-5"}');
  });
});

describe('session ops', () => {
  it('stopSession POSTs to /stop', async () => {
    const f = stubFetch({ ok: true, id: 'ses_1' }, 200);
    await runsApi.stopSession('ses_1');
    expect(f.mock.calls[0][0]).toBe('/api/sessions/ses_1/stop');
    expect(f.mock.calls[0][1]?.method).toBe('POST');
  });

  it('compactSession sends the window only when given', async () => {
    const f = stubFetch({ ok: true, id: 'ses_1', compaction_id: 'cmp_1', log: '/l' });
    await runsApi.compactSession('ses_1');
    expect(f.mock.calls[0][0]).toBe('/api/sessions/ses_1/compact');
    expect(f.mock.calls[0][1]?.body).toBe('{}');
    await runsApi.compactSession('ses_1', 120000);
    expect(f.mock.calls[1][1]?.body).toBe('{"window":120000}');
  });
});

describe('isRetryable', () => {
  it('is true only for failed, rejected and cancelled', () => {
    expect(['failed', 'rejected', 'cancelled'].every(isRetryable)).toBe(true);
    expect(['running', 'pending', 'paused', 'awaiting_approval', 'completed'].some(isRetryable)).toBe(false);
  });
});

describe('modeLabel', () => {
  it('names the three modes', () => {
    expect(modeLabel('ask')).toBe('Ask');
    expect(modeLabel('bypass')).toBe('Bypass');
    expect(modeLabel('readonly')).toBe('Read-only');
  });
});

describe('compactionLine', () => {
  const base = { id: 'cmp_1', finished_at: '2026-10-20T10:00:00Z' };

  it('reads a compaction from the summary it wrote', () => {
    expect(
      compactionLine({
        ...base,
        outcome: 'compacted',
        detail: 'compacted session ses_1: summarized 30 messages, history 34 → 5 messages',
      }),
    ).toBe('Compacted 30 messages into 1: history 34 → 5 messages');
  });

  it('falls back to the detail when a compaction line has another shape', () => {
    expect(compactionLine({ ...base, outcome: 'compacted', detail: 'compacted session ses_1' })).toBe(
      'Compacted the conversation',
    );
  });

  it('says why a discarded compaction was dropped', () => {
    expect(
      compactionLine({
        ...base,
        outcome: 'discarded',
        detail:
          'compaction of session ses_1 discarded: the session changed while it was summarized (its spend is still recorded)',
      }),
    ).toBe('Compaction discarded: the session changed while it was summarized (its spend is still recorded)');
  });

  it('says there was nothing to compact', () => {
    expect(
      compactionLine({ ...base, outcome: 'nothing_to_compact', detail: 'nothing to compact: history is too short or already compact' }),
    ).toBe('Nothing to compact: history is too short or already compact');
  });

  it('gives a failure its detail', () => {
    expect(compactionLine({ ...base, outcome: 'failed', detail: 'provider anthropic returned 529' })).toBe(
      'Compaction failed: provider anthropic returned 529',
    );
  });
});

describe('isAgentRunRecord', () => {
  it('names the run store records of standalone agent runs', () => {
    expect(isAgentRunRecord('agent:code-reviewer')).toBe(true);
    expect(isAgentRunRecord('security-review')).toBe(false);
  });
});
