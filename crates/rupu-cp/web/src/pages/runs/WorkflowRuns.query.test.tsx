// @vitest-environment jsdom
// WorkflowRuns on the query line: the server filters by `q`, the page sends
// it on every request (with the running statuses when it names no status),
// `host:` picks the hosts asked, a bad query is the bar's error, and host
// health is in the suggestions and one line under the bar.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import WorkflowRuns from './WorkflowRuns';
import type { RunListRow } from '../../lib/api';
import { REG_LOCAL, REG_PROD } from '../../lib/perHost/testUtils';
import { listCalls, parseError, stubListFetch, type StubAnswer } from '../../lib/query/listFetchStub';
import { withCustomerScope } from '../../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const PATH = '/api/runs/workflows';

function run(over: Partial<RunListRow>): RunListRow {
  return {
    codename: 'cobalt-harbor/heron#1',
    codename_derived: false,
    id: 'run_1',
    workflow_name: 'deploy-prod',
    status: 'running',
    started_at: '2026-10-14T10:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: { input_tokens: 1, output_tokens: 1, cached_tokens: 0, total_tokens: 2, cost_usd: 0, priced: true, runs: 1 },
    host_id: 'local',
    ...over,
  };
}

function mount(search = '') {
  return render(
    <MemoryRouter initialEntries={[`/runs/workflows${search}`]}>{withCustomerScope(<WorkflowRuns />)}</MemoryRouter>,
  );
}

const sentQ = (urls: URL[], host: string) =>
  listCalls(urls, PATH)
    .filter((u) => u.searchParams.get('host') === host)
    .map((u) => u.searchParams.get('q'));

const offline: StubAnswer = { status: 502, body: { error: 'host unreachable: timed out' } };

describe('WorkflowRuns — the query reaches the server', () => {
  it.each([
    ['', null],
    ['?q=trigger:cron', 'trigger:cron'],
    ['?q=status:failed', 'status:failed'],
    ['?q=-status:completed', '-status:completed'],
    ['?q=is:archived', 'is:archived'],
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
    await screen.findByText('deploy-prod');
    const input = screen.getByRole('combobox', { name: 'Filter workflow runs' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(sentQ(urls, 'local')).toContain('status:failed'));
    const calls = listCalls(urls, PATH);
    const last = calls[calls.length - 1];
    expect(last.searchParams.get('offset')).toBe('0');
  });
});

describe('WorkflowRuns — host: picks the hosts asked', () => {
  it('a positive host term asks only that host', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [run({})] }));
    mount('?q=host:local');
    await screen.findByText('deploy-prod');
    expect(sentQ(urls, 'local')).toEqual(['host:local']);
    expect(sentQ(urls, 'host_prod')).toEqual([]);
  });

  it('a negated host term leaves that host out', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [] }));
    mount('?q=-host:local');
    await waitFor(() => expect(sentQ(urls, 'host_prod')).toHaveLength(1));
    expect(sentQ(urls, 'local')).toEqual([]);
  });
});

describe('WorkflowRuns — a query the server rejects', () => {
  it('shows the server’s parse error under the bar, not as a page error', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => parseError('`trigger` takes manual, cron or event'));
    mount('?q=trigger:cron');
    await waitFor(() => expect(screen.getByText('`trigger` takes manual, cron or event')).toBeInTheDocument());
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByText('No workflow runs match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No hosts answered')).not.toBeInTheDocument();
  });

  it('asks no host for a query the browser can’t parse, and shows why at once', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [run({})] }));
    mount('?q=status:done');
    expect(
      await screen.findByText('`done` is not a value of `status:` (one of pending, running, awaiting_approval, paused, completed, failed, rejected, cancelled)'),
    ).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('No workflow runs match the filter.')).toBeInTheDocument());
    expect(listCalls(urls, PATH)).toEqual([]);
  });
});

describe('WorkflowRuns — facets and host health', () => {
  it('asks each host for facets on its first page only, and suggests every host with its state', async () => {
    const urls = stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? u.searchParams.get('facets') === '1'
          ? { body: { rows: [run({})], facets: { host: { local: 7 }, status: { failed: 2 } }, facets_partial: false } }
          : { body: [run({})] }
        : offline,
    );
    mount();
    await screen.findByText('deploy-prod');
    const localCalls = () => listCalls(urls, PATH).filter((u) => u.searchParams.get('host') === 'local');
    expect(localCalls().map((u) => u.searchParams.get('facets'))).toEqual(['1']);

    const input = screen.getByRole('combobox', { name: 'Filter workflow runs' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
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
        ? { body: [run({})], headers: { 'X-Rupu-Hosts-Unfilterable': 'mini:status' } }
        : offline,
    );
    mount();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent(
      'prod offline: its runs are missing · mini can’t filter by status: its runs are left out'.replace('’', "'"),
    );
  });

  it('names a host that can’t list the archive', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], (u) =>
      u.searchParams.get('host') === 'local'
        ? { body: [run({ archived: true, status: 'completed' })] }
        : { status: 501, body: { error: "host host_prod can't list archived runs" } },
    );
    mount('?q=is:archived');
    expect(await screen.findByTestId('host-alert')).toHaveTextContent("prod can't list archived runs: they are left out");
    expect(screen.getByRole('button', { name: 'Restore run run_1' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Archive run run_1' })).not.toBeInTheDocument();
  });

  it('shows no line while every host answers', async () => {
    stubListFetch([REG_LOCAL, REG_PROD], () => ({ body: [run({})] }));
    mount();
    await screen.findAllByText('deploy-prod');
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });
});
