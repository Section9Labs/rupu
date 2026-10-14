// @vitest-environment jsdom
// Sessions on the query line: the server filters by `q`, the page sends it
// on every request (with `is:active` when it names no `is:`), `host:` picks
// the hosts asked, a bad query is the bar's error, and host health is in the
// suggestions and one line under the bar.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Sessions from './Sessions';
import type { SessionSummary } from '../lib/api';
import { REG_LOCAL, REG_PROD } from '../lib/perHost/testUtils';
import { listCalls, parseError, stubListFetch, type StubAnswer } from '../lib/query/listFetchStub';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const PATH = '/api/sessions';

function session(over: Partial<SessionSummary>): SessionSummary {
  return {
    codename: 'cobalt-harbor/heron#1',
    codename_derived: false,
    session_id: 'sess_1',
    agent_name: 'fix-bug',
    model: 'claude-opus',
    status: 'idle',
    total_turns: 1,
    created_at: '2026-10-14T10:00:00Z',
    updated_at: '2026-10-14T10:05:00Z',
    scope: 'active',
    host_id: 'local',
    ...over,
  };
}

function mount(search = '') {
  return render(<MemoryRouter initialEntries={[`/sessions${search}`]}>{withCustomerScope(<Sessions />)}</MemoryRouter>);
}

const sentQ = (urls: URL[], host: string) =>
  listCalls(urls, PATH)
    .filter((u) => u.searchParams.get('host') === host)
    .map((u) => u.searchParams.get('q'));

const offline: StubAnswer = { status: 502, body: { error: 'host unreachable: timed out' } };

describe('Sessions — the query reaches the server', () => {
  it.each([
    ['', 'is:active'],
    ['?q=agent:recon', 'agent:recon is:active'],
    ['?q=is:archived', 'is:archived'],
    ['?q=-is:active', '-is:active'],
  ])('%s sends q=%s to every host', async (search, q) => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount(search);
    await waitFor(() => expect(sentQ(urls, 'host_prod')).toHaveLength(1));
    expect(sentQ(urls, 'local')).toEqual([q]);
    expect(sentQ(urls, 'host_prod')).toEqual([q]);
  });

  it('a new query is a new list, read from the first page', async () => {
    const urls = stubListFetch([REG_LOCAL], () => ({ body: [session({})] }));
    mount();
    await screen.findAllByText(/fix-bug/);
    const input = screen.getByRole('combobox', { name: 'Filter sessions' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(sentQ(urls, 'local')).toContain('status:failed is:active'));
    const calls = listCalls(urls, PATH);
    expect(calls[calls.length - 1].searchParams.get('offset')).toBe('0');
  });
});

describe('Sessions — host: picks the hosts asked', () => {
  it('a positive host term asks only that host', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [session({})] }));
    mount('?q=host:local');
    await screen.findAllByText(/fix-bug/);
    expect(sentQ(urls, 'local')).toEqual(['host:local is:active']);
    expect(sentQ(urls, 'host_prod')).toEqual([]);
  });

  it('a negated host term leaves that host out', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount('?q=-host:local');
    await waitFor(() => expect(sentQ(urls, 'host_prod')).toHaveLength(1));
    expect(sentQ(urls, 'local')).toEqual([]);
  });
});

describe('Sessions — a query the server rejects', () => {
  it('shows the server’s parse error under the bar, not as a page error', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => parseError('`provider` takes anthropic or openai'));
    mount('?q=provider:anthropic');
    await waitFor(() => expect(screen.getByText('`provider` takes anthropic or openai')).toBeInTheDocument());
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByText('No sessions match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No hosts answered')).not.toBeInTheDocument();
  });

  it('shows a client parse error at once and asks no host', async () => {
    const urls = stubListFetch([REG_LOCAL], () => ({ body: [] }));
    mount('?q=is:closed');
    expect(await screen.findByText('`closed` is not a value of `is:` (one of active, archived)')).toBeInTheDocument();
    expect(listCalls(urls, PATH)).toEqual([]);
  });
});

describe('Sessions — facets and host health', () => {
  it('asks each host for facets on its first page only, and suggests every host with its state', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? u.searchParams.get('facets') === '1'
          ? { body: { rows: [session({})], facets: { host: { local: 4 }, status: { idle: 4 } }, facets_partial: false } }
          : { body: [session({})] }
        : offline,
    );
    mount();
    await screen.findAllByText(/fix-bug/);
    const localCalls = () => listCalls(urls, PATH).filter((u) => u.searchParams.get('host') === 'local');
    expect(localCalls().map((u) => u.searchParams.get('facets'))).toEqual(['1']);

    const input = screen.getByRole('combobox', { name: 'Filter sessions' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(localCalls()).toHaveLength(2));
    expect(localCalls()[1].searchParams.get('facets')).toBeNull();

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'host:' } });
    const options = await screen.findAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual(['localLocal · live · 4', 'host_prodprod · offline']);
  });

  it('says in one line which hosts are missing and why', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? { body: [session({})], headers: { 'X-Rupu-Hosts-Unfilterable': 'mini:model' } }
        : offline,
    );
    mount();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent(
      "prod offline: its sessions are missing · mini can't filter by model: its sessions are left out",
    );
  });

  it('shows no line while every host answers', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [session({})] }));
    mount();
    await screen.findAllByText(/fix-bug/);
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });
});
