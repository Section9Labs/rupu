// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import fixture from '../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { api, type FindingDetail as Detail } from '../lib/api';
import type { FindingReport } from '../lib/findingReport';
import FindingDetail from './FindingDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const report = fixture as unknown as FindingReport;

function base(over: Partial<Detail>): Detail {
  return {
    id: 'fnd_1',
    ws_id: 'ws1',
    project: 'demo',
    target_id: 't-1',
    file_path: 'src/store/notes.rs',
    line_range: null,
    scope: null,
    summary: 'Notes API returns another user\'s note by id',
    severity: 'high',
    concern_id: null,
    evidence: { rationale: 'The lookup ignores the owner id.' },
    declared_by: { run_id: 'run_42', model: 'claude-x', surface: 'agent' },
    declared_at: '2026-08-01T00:00:00Z',
    evidence_status: [],
    ...over,
  };
}

function renderAt(id = 'fnd_1') {
  return render(
    <MemoryRouter initialEntries={[`/findings/${id}`]}>
      <Routes>
        <Route path="/findings/:id" element={<FindingDetail />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('FindingDetail page', () => {
  it('renders the full report profile with section rail and completeness', async () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(
      base({ profile: 'full', report, evidence_status: ['current'] }),
    );
    renderAt();

    const h1 = await screen.findByRole('heading', { level: 1 });
    expect(h1).toHaveTextContent(report.title);
    expect(spy).toHaveBeenCalledWith('fnd_1');

    // Root cause is markdown (the symbol renders as <code>), so match the prose after it.
    expect(screen.getByText(/is called with only the note id/)).toBeInTheDocument();

    for (const title of ['Call chain', 'Evidence', 'Recommended patch', 'Regression test', 'Replication steps']) {
      expect(screen.getByRole('heading', { level: 2, name: title })).toBeInTheDocument();
    }

    const rail = screen.getByRole('navigation', { name: 'Report sections' });
    expect(within(rail).getByRole('link', { name: 'Root cause' })).toHaveAttribute('href', '#s-root');

    expect(within(rail).getByText('9/11')).toBeInTheDocument();
    expect(within(rail).getByText(/owner, cvss_v3/)).toBeInTheDocument();
  });

  it('links provenance to the originating run', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, evidence_status: ['current'] }));
    renderAt();
    const link = await screen.findByRole('link', { name: 'run_42' });
    expect(link).toHaveAttribute('href', '/runs/run_42');
    expect(screen.getByText('claude-x')).toBeInTheDocument();
  });

  it('degrades provenance when declared_by is absent', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, declared_by: null }));
    renderAt();
    await screen.findByRole('heading', { level: 1 });
    expect(screen.queryByRole('link', { name: /^run_/ })).toBeNull();
  });

  it('renders a summary-profile finding with its rationale and a note', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'summary', report: null }));
    renderAt();

    expect(await screen.findByRole('heading', { level: 1 })).toHaveTextContent("Notes API returns another user's note by id");
    expect(screen.getByText('The lookup ignores the owner id.')).toBeInTheDocument();
    expect(screen.getByText(/This finding was recorded as a summary/)).toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Report sections' })).toBeNull();
  });

  it('shows an alert with the error text when the API fails', async () => {
    vi.spyOn(api, 'getFinding').mockRejectedValue(new Error('404 finding not found'));
    renderAt('missing');

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('404 finding not found');
  });

  it('shows a loading indicator before the finding arrives', () => {
    vi.spyOn(api, 'getFinding').mockReturnValue(new Promise(() => {}));
    renderAt();
    expect(screen.getByText('Loading finding')).toBeInTheDocument();
  });
});
