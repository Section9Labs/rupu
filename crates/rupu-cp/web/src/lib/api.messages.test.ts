import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, ApiError, boardOf, type MessagesView } from './api';

afterEach(() => vi.restoreAllMocks());

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}

const VIEW: MessagesView = {
  participants: [
    { address: 'lead', agent: 'lead', run_id: 'af_1_lead', kind: 'flow_lead', live: true },
  ],
  messages: [
    {
      id: 'msg_1',
      ts: '2026-10-07T22:15:00Z',
      kind: 'post',
      from: 'recon#1',
      to: null,
      post_kind: 'observation',
      body: 'port 22 open',
      delivery: { resolved: [] },
      seen: [],
    },
    {
      id: 'msg_2',
      ts: '2026-10-07T22:16:00Z',
      kind: 'directive',
      from: 'lead',
      to: null,
      body: 'stay in scope',
      delivery: { resolved: ['recon#1'] },
      seen: [{ by: 'recon#1', turn: 3, at: '2026-10-07T22:16:05Z' }],
    },
    {
      id: 'msg_3',
      ts: '2026-10-07T22:17:00Z',
      kind: 'retract',
      from: 'lead',
      to: null,
      body: '',
      refs: { retracts: 'msg_2' },
      delivery: { resolved: [] },
      seen: [],
    },
    {
      id: 'msg_4',
      ts: '2026-10-07T22:18:00Z',
      kind: 'operator',
      from: 'operator',
      to: 'lead',
      body: 'focus on auth',
      urgent: true,
      via: 'cp',
      delivery: { resolved: ['lead'] },
      seen: [],
    },
    {
      id: 'msg_5',
      ts: '2026-10-07T22:19:00Z',
      kind: 'directive',
      from: 'lead',
      to: 'recon#1',
      body: 'skip the printers',
      delivery: { resolved: ['recon#1'] },
      seen: [],
    },
  ],
  legacy: false,
  space: 'present',
  next: null,
};

describe('messages API', () => {
  it('getMessages reads a route with its query and returns the view', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(VIEW));

    const got = await api.getMessages('runs', 'run_01A', {
      host: 'host_x',
      after: 'msg_1',
      limit: 50,
      kinds: ['direct', 'operator'],
    });

    expect(fetchMock.mock.calls[0][0]).toBe(
      '/api/runs/run_01A/messages?host=host_x&after=msg_1&limit=50&kind=direct%2Coperator',
    );
    expect(got).toEqual({ kind: 'ok', view: VIEW });
  });

  it('a 501 is unavailable with the server reason, on reads and sends', async () => {
    const reason = 'operator messages: a tunnel host has no messaging transport';
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      jsonResponse({ error: reason }, 501),
    );

    expect(await api.getMessages('runs', 'run_01A', { host: 'host_t' })).toEqual({
      kind: 'unavailable',
      reason,
    });
    expect(await api.sendMessage('sessions', 'ses_01A', { body: 'hi' }, { host: 'host_t' })).toEqual({
      kind: 'unavailable',
      reason,
    });
  });

  it('any other failure still throws, so a 409 reads as the run not running', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      jsonResponse({ error: 'workflow run `run_01A` is not running (completed)' }, 409),
    );

    const err = await api.sendMessage('runs', 'run_01A', { body: 'hi' }).catch((e: unknown) => e);

    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(409);
  });

  it('sendMessage posts the body and returns the delivery', async () => {
    const reply = { id: 'msg_9', delivery: { resolved: ['lead'] } };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(reply, 202));

    const got = await api.sendMessage('agentiflows', 'af_01A', { to: 'lead', body: 'go', urgent: true });

    expect(fetchMock.mock.calls[0][0]).toBe('/api/agentiflows/af_01A/messages');
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ to: 'lead', body: 'go', urgent: true });
    expect(got).toEqual({ kind: 'sent', reply });
  });

  it('messagesStreamUrl names the route', () => {
    expect(api.messagesStreamUrl('sessions', 'ses_01A')).toBe('/api/sessions/ses_01A/messages/stream');
  });

  it('the agentiflow board is the view’s posts and directives, retractions folded as {retract}', async () => {
    expect(boardOf(VIEW)).toEqual({
      posts: [
        {
          id: 'msg_1',
          kind: 'observation',
          author: 'recon#1',
          addressed_to: null,
          ts: '2026-10-07T22:15:00Z',
          body: 'port 22 open',
        },
      ],
      directives: [
        { id: 'msg_2', author: 'lead', addressed_to: null, ts: '2026-10-07T22:16:00Z', body: 'stay in scope' },
        { retract: 'msg_2' },
        {
          id: 'msg_5',
          author: 'lead',
          addressed_to: 'recon#1',
          ts: '2026-10-07T22:19:00Z',
          body: 'skip the printers',
        },
      ],
    });

    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(VIEW));
    expect(await api.getAgentiflowMessages('af_01A')).toEqual(boardOf(VIEW));
    expect(fetchMock.mock.calls[0][0]).toBe('/api/agentiflows/af_01A/messages');
  });
});
