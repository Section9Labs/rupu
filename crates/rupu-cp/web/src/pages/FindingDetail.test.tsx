// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import fixture from '../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { api, ApiError, type FindingDetail as Detail } from '../lib/api';
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
    expect(within(rail).getByText('Gaps: owner, cvss_v3')).toBeInTheDocument();
    expect(within(rail).queryByText(/Unknown:/)).toBeNull();
  });

  it('shows a compact completeness line that is hidden at lg and up (where the rail shows the meter)', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, evidence_status: ['current'] }));
    renderAt();
    const line = await screen.findByText('Report 9/11 · gaps: owner, cvss_v3');
    expect(line).toBe(screen.getByTestId('completeness-compact'));
    expect(line).toHaveClass('lg:hidden');
    // it lives in the article, not the rail (which is `hidden lg:block`)
    expect(within(screen.getByRole('navigation', { name: 'Report sections' })).queryByText(/^Report 9\/11/)).toBeNull();
  });

  it('omits the gaps clause from the compact line when the report is complete', async () => {
    const full = {
      ...report,
      ownership: { ...report.ownership, owner: 'Platform team' },
      rating: { ...report.rating, cvss_v3: '7.5' },
    };
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report: full, evidence_status: ['current'] }));
    renderAt();
    const line = await screen.findByTestId('completeness-compact');
    expect(line).toHaveTextContent('Report 11/11');
    expect(line).not.toHaveTextContent('gaps');
  });

  it('lists the PoC artifacts rail anchor only when the report has artifacts', async () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, evidence_status: ['current'] }));
    const first = renderAt();
    const rail = await screen.findByRole('navigation', { name: 'Report sections' });
    // The fixture has no artifacts: no dead anchor, and no section for it either.
    expect(within(rail).queryByRole('link', { name: 'PoC artifacts' })).toBeNull();
    expect(rail.querySelector('a[href="#s-artifacts"]')).toBeNull();
    expect(document.getElementById('s-artifacts')).toBeNull();
    first.unmount();

    spy.mockResolvedValue(base({
      profile: 'full',
      report: { ...report, artifacts: [{ path: 'poc/exploit.py', sha256: 'ab'.repeat(32), size: 12, kind: 'text', stored: 'copied' }] },
      evidence_status: ['current'],
    }));
    renderAt();
    const rail2 = await screen.findByRole('navigation', { name: 'Report sections' });
    expect(within(rail2).getByRole('link', { name: 'PoC artifacts' })).toHaveAttribute('href', '#s-artifacts');
    expect(document.getElementById('s-artifacts')).not.toBeNull();
  });

  it('has a back link to the findings list above the report header', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, evidence_status: ['current'] }));
    renderAt();
    const back = await screen.findByRole('link', { name: '← Findings' });
    expect(back).toHaveAttribute('href', '/findings');
    const h1 = screen.getByRole('heading', { level: 1 });
    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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

  it('says a full-profile finding with an unreadable report cannot be displayed, not that it is a summary', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report: null }));
    renderAt();

    expect(await screen.findByRole('heading', { level: 1 })).toHaveTextContent("Notes API returns another user's note by id");
    expect(screen.getByText(
      "This finding has a full report that this version of rupu can't display (it may have been written by a newer version).",
    )).toBeInTheDocument();
    expect(screen.queryByText(/recorded as a summary/)).toBeNull();
    // the summary + rationale still render
    expect(screen.getByText('The lookup ignores the owner id.')).toBeInTheDocument();
  });

  it('summary layout also has a back link to the findings list', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'summary', report: null }));
    renderAt();
    const back = await screen.findByRole('link', { name: '← Findings' });
    expect(back).toHaveAttribute('href', '/findings');
    const h1 = screen.getByRole('heading', { level: 1 });
    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('shows an alert with the error text when the API fails', async () => {
    vi.spyOn(api, 'getFinding').mockRejectedValue(new Error('404 finding not found'));
    renderAt('missing');

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('404 finding not found');
  });

  it('shows the message from a JSON API error body, not the JSON', async () => {
    const body = JSON.stringify({ error: 'finding x not found' });
    vi.spyOn(api, 'getFinding').mockRejectedValue(new ApiError(404, body, body));
    renderAt('x');

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('finding x not found');
    expect(alert).not.toHaveTextContent('"error"');
  });

  it('shows a loading indicator before the finding arrives', () => {
    vi.spyOn(api, 'getFinding').mockReturnValue(new Promise(() => {}));
    renderAt();
    expect(screen.getByText('Loading finding')).toBeInTheDocument();
  });
});
