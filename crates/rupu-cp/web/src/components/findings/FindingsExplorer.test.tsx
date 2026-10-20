// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
  it('imports reports next to Export, then reloads the list', async () => {
    const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
      if (path === '/api/findings/import') {
        const dry = JSON.parse(init?.body as string).dry_run as boolean;
        return new Response(
          JSON.stringify({ dry_run: dry, files: [{ path: '/r/a.md', status: 'attached', finding_id: 'f1' }], backups: [], attached: 1, skipped: 0, failed: 0 }),
        );
      }
      return new Response(JSON.stringify(BODY), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderExplorer('/security?tab=findings', { scope: {} });
    await screen.findByText('Default admin token accepted');
    expect(screen.getByRole('button', { name: 'Export report' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Import' }));
    const dialog = screen.getByRole('dialog', { name: 'Import finding reports' });
    fireEvent.change(within(dialog).getByLabelText('Files or directories on this machine'), { target: { value: '/r' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Dry run' }));
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Import 1 report' }));

    await waitFor(() => expect(urls(fetchMock).filter((u) => u === '/api/findings' || u.startsWith('/api/findings?'))).toHaveLength(2));
  });

  it('offers no import on a run scope', async () => {
    stubFetch();
    renderExplorer('/runs/r1', { scope: { runId: 'r1' } });
    await screen.findByText('Default admin token accepted');
    expect(screen.queryByRole('button', { name: 'Import' })).not.toBeInTheDocument();
  });

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
    await waitFor(() => expect(onCount).toHaveBeenLastCalledWith(4));
  });

  it('reloads the scope on its poll interval', async () => {
    const fetchMock = stubFetch();
    renderExplorer('/agentiflows/af_1', { scope: { runId: 'af_1' }, pollMs: 20 });
    await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(3));
    expect(new Set(urls(fetchMock))).toEqual(new Set(['/api/findings?run_id=af_1']));
    expect(screen.getByText('Default admin token accepted')).toBeInTheDocument();
  });
});
