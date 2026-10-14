// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { FindingOut, FindingsResponse } from '../../lib/api';
import { FindingsExplorer, type FindingsExplorerProps } from './FindingsExplorer';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const ROW: FindingOut = {
  codename: 'cobalt-harbor/heron#1',
  codename_derived: false,
  id: 'f1',
  scope: null,
  summary: 'Default admin token accepted',
  severity: 'high',
  evidence: { rationale: '' },
  declared_by: null,
  declared_at: '2026-10-01T00:00:00Z',
  ws_id: 'ws-1',
  project: 'shop-web',
  target_id: 'tgt',
};

const BODY: FindingsResponse = {
  findings: [ROW],
  summary: { total: 4, critical: 1, high: 3, medium: 0, low: 0, info: 0 },
  facets: { severity: [{ value: 'critical', count: 1 }, { value: 'high', count: 3 }] },
  tags_unavailable: [],
};

function stubFetch() {
  const fetchMock = vi.fn(async (_path: string) => new Response(JSON.stringify(BODY), { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

const urls = (m: ReturnType<typeof stubFetch>) => m.mock.calls.map((c) => c[0]);

function renderExplorer(entry: string, props: Partial<FindingsExplorerProps> & Pick<FindingsExplorerProps, 'scope'>) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <FindingsExplorer exportTitle="Report" empty={{ title: 'No findings', hint: 'none' }} {...props} />
    </MemoryRouter>,
  );
}

describe('FindingsExplorer', () => {
  it('asks for a project scope together with the query, under a query bar', async () => {
    const fetchMock = stubFetch();
    renderExplorer('/projects/p/findings?tab=x&q=severity%3Ahigh', { scope: { wsId: 'ws-1' } });
    await screen.findByText('Default admin token accepted');
    expect(urls(fetchMock)).toEqual(['/api/findings?ws_id=ws-1&q=severity%3Ahigh']);
    expect(screen.getByRole('combobox', { name: 'Filter findings' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove severity: high' })).toBeInTheDocument();
  });

  it('asks for a run scope with the query, and reports the scope count', async () => {
    const fetchMock = stubFetch();
    const onCount = vi.fn();
    renderExplorer('/runs/run_1?q=tag%3Aneeds-poc', { scope: { runId: 'run_1' }, onCount });
    await screen.findByText('Default admin token accepted');
    expect(urls(fetchMock)).toEqual(['/api/findings?run_id=run_1&q=tag%3Aneeds-poc']);
    expect(onCount).toHaveBeenLastCalledWith(4);
  });

  it('reloads the scope on its poll interval', async () => {
    const fetchMock = stubFetch();
    renderExplorer('/agentiflows/af_1', { scope: { runId: 'af_1' }, pollMs: 20 });
    await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(3));
    expect(new Set(urls(fetchMock))).toEqual(new Set(['/api/findings?run_id=af_1']));
    expect(screen.getByText('Default admin token accepted')).toBeInTheDocument();
  });
});
