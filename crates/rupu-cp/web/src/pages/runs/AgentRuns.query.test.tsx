// @vitest-environment jsdom
// AgentRuns on the query line: the server filters by `q`, the page sends it
// on every request (standalone runs when it names no source, running ones
// when it names no status), `host:` picks the hosts asked, a bad query is the
// bar's error, and host health is in the suggestions and one line under the
// bar.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import AgentRuns from './AgentRuns';
import type { AgentRunRow } from '../../lib/api';
import { REG_LOCAL, REG_PROD } from '../../lib/perHost/testUtils';
import { listCalls, parseError, stubListFetch, type StubAnswer } from '../../lib/query/listFetchStub';
import { withCustomerScope } from '../../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const PATH = '/api/runs/agents';

function run(over: Partial<AgentRunRow>): AgentRunRow {
  return {
    codename: 'cobalt-harbor/heron#1',
    codename_derived: false,
    run_id: 'run_1',
    source: 'standalone',
    agent: 'fix-bug',
    status: 'running',
    started_at: '2026-10-14T10:00:00Z',
    turns: 1,
    usage: { input_tokens: 1, output_tokens: 1, cached_tokens: 0, total_tokens: 2, cost_usd: 0, priced: true, runs: 1 },
    host_id: 'local',
    ...over,
  };
}

function mount(search = '') {
  return render(
    <MemoryRouter initialEntries={[`/runs/agents${search}`]}>{withCustomerScope(<AgentRuns />)}</MemoryRouter>,
  );
}

const sentQ = (urls: URL[], host: string) =>
  listCalls(urls, PATH)
    .filter((u) => u.searchParams.get('host') === host)
    .map((u) => u.searchParams.get('q'));

const offline: StubAnswer = { status: 502, body: { error: 'host unreachable: timed out' } };

describe('AgentRuns — the query reaches the server', () => {
  it.each([
    ['', 'source:standalone'],
    ['?q=source:session', 'source:session'],
    ['?q=status:error', 'status:error source:standalone'],
    ['?q=source:session status:ok', 'source:session status:ok'],
  ])('%s sends q=%s to every host', async (search, q) => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount(search);
    await waitFor(() => expect(sentQ(urls, 'host_prod')).toHaveLength(1));
    expect(sentQ(urls, 'local')).toEqual([q]);
    expect(sentQ(urls, 'host_prod')).toEqual([q]);
  });

  it('a new query is a new list, read from the first page', async () => {
    const urls = stubListFetch([REG_LOCAL], () => ({ body: [run({})] }));
    mount();
    await screen.findByText(/fix-bug/);
    const input = screen.getByRole('combobox', { name: 'Filter agent runs' });
    fireEvent.change(input, { target: { value: 'status:error' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(sentQ(urls, 'local')).toContain('status:error source:standalone'));
    const calls = listCalls(urls, PATH);
    const last = calls[calls.length - 1];
    expect(last.searchParams.get('offset')).toBe('0');
  });
});

describe('AgentRuns — host: picks the hosts asked', () => {
  it('a positive host term asks only that host', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [run({})] }));
    mount('?q=host:local');
    await screen.findByText(/fix-bug/);
    expect(sentQ(urls, 'local')).toEqual(['host:local source:standalone']);
    expect(sentQ(urls, 'host_prod')).toEqual([]);
  });

  it('a negated host term leaves that host out', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount('?q=-host:local');
    await waitFor(() => expect(sentQ(urls, 'host_prod')).toHaveLength(1));
    expect(sentQ(urls, 'local')).toEqual([]);
  });
});

describe('AgentRuns — a query the server rejects', () => {
  it('shows the server’s parse error under the bar, not as a page error', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => parseError('`trigger` takes cli, cron or session_turn'));
    mount('?q=trigger:cli');
    await waitFor(() => expect(screen.getByText('`trigger` takes cli, cron or session_turn')).toBeInTheDocument());
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByText('No agent runs match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No hosts answered')).not.toBeInTheDocument();
  });

  it('shows a client parse error at once, and asks no host', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount('?q=source:remote');
    expect(await screen.findByText('`remote` is not a value of `source:` (one of standalone, session)')).toBeInTheDocument();
    await waitFor(() => expect(urls.some((u) => u.pathname === '/api/hosts/registered')).toBe(true));
    // Give a request that would follow the host list time to go out.
    await new Promise((r) => setTimeout(r, 50));
    expect(listCalls(urls, PATH)).toEqual([]);
  });
});

describe('AgentRuns — facets and host health', () => {
  it('asks each host for facets on its first page only, and suggests every host with its state', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? u.searchParams.get('facets') === '1'
          ? { body: { rows: [run({})], facets: { host: { local: 7 }, status: { error: 2 } }, facets_partial: false } }
          : { body: [run({})] }
        : offline,
    );
    mount();
    await screen.findByText(/fix-bug/);
    const localCalls = () => listCalls(urls, PATH).filter((u) => u.searchParams.get('host') === 'local');
    expect(localCalls().map((u) => u.searchParams.get('facets'))).toEqual(['1']);

    const input = screen.getByRole('combobox', { name: 'Filter agent runs' });
    fireEvent.change(input, { target: { value: 'status:error' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(localCalls()).toHaveLength(2));
    expect(localCalls()[1].searchParams.get('facets')).toBeNull();

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'host:' } });
    const options = await screen.findAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual(['localLocal · live · 7', 'host_prodprod · offline']);
  });

  it('says in one line which hosts are missing and why', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? { body: [run({})], headers: { 'X-Rupu-Hosts-Unfilterable': 'mini:trigger' } }
        : offline,
    );
    mount();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent(
      "prod offline: its runs are missing · mini can't filter by trigger: its runs are left out",
    );
  });

  it('shows no line while every host answers', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [run({})] }));
    mount();
    await screen.findAllByText(/fix-bug/);
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });
});
