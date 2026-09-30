// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import fixture from '../../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { api, ApiError, type FindingDetail, type FindingOut, type FindingRecord } from '../../lib/api';
import type { FindingReport } from '../../lib/findingReport';
import InlineFindingCard from './InlineFindingCard';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const report = fixture as unknown as FindingReport;

const FULL = {
  id: 'f-full',
  ws_id: 'ws_1',
  file_path: 'src/routes/notes.rs',
  line_range: [40, 58],
  summary: 'IDOR on notes read',
  severity: 'high',
  profile: 'full',
  // The server writes `evidence.rationale = report.root_cause` for full-profile
  // findings, so the card's rationale block and the Root cause tab carry the
  // same text.
  evidence: { rationale: report.root_cause },
} as unknown as FindingOut;

const SUMMARY = {
  id: 'f-sum',
  file_path: 'src/billing.rs',
  line_range: [17, 17],
  summary: 'Missing tenant check',
  severity: 'medium',
  profile: 'summary',
  evidence: { rationale: 'Summary rationale.' },
} as unknown as FindingRecord;

function detailOf(over: Partial<FindingDetail> = {}): FindingDetail {
  return { ...FULL, report, evidence_status: ['current'], ...over } as FindingDetail;
}

function view(f: FindingRecord) {
  return render(
    <MemoryRouter>
      <InlineFindingCard finding={f} stale={false} />
    </MemoryRouter>,
  );
}

const header = () => screen.getByRole('button', { name: /IDOR on notes read/ });
const ROOT_CAUSE = /owner id from the session is never part/;

describe('InlineFindingCard — full-profile report tabs', () => {
  it('does not fetch until the card is expanded', async () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    expect(spy).not.toHaveBeenCalled();
    fireEvent.click(header());
    await waitFor(() => expect(spy).toHaveBeenCalledWith('f-full'));
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('shows a spinner then the Root cause tab by default', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    expect(screen.getByRole('status')).toBeInTheDocument();
    // The tablist only exists once the report has loaded (the stored rationale
    // is on screen earlier as a placeholder, so it isn't a reliable signal).
    expect(await screen.findByRole('tabpanel', { name: 'Root cause' })).toHaveTextContent(ROOT_CAUSE);
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.getByRole('tab', { name: 'Root cause' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tablist')).toBeInTheDocument();
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual([
      'Root cause', 'Call chain', 'Evidence', 'Patch', 'Repro',
    ]);
  });

  it('shows the root cause exactly once: as the placeholder while loading, then only in the tab', async () => {
    let resolve!: (d: FindingDetail) => void;
    vi.spyOn(api, 'getFinding').mockReturnValue(new Promise((r) => { resolve = r; }));
    view(FULL);
    fireEvent.click(header());
    // Loading: the stored rationale stands in for the not-yet-loaded tab.
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getAllByText(ROOT_CAUSE)).toHaveLength(1);
    expect(screen.queryByRole('tablist')).toBeNull();
    resolve(detailOf());
    // Loaded: the rationale block is hidden; only the Root cause tab shows it.
    await screen.findByRole('tablist');
    expect(screen.getAllByText(ROOT_CAUSE)).toHaveLength(1);
    expect(screen.getByRole('tabpanel')).toHaveTextContent(ROOT_CAUSE);
  });

  it('gives the tabpanel an accessible name matching the selected tab, with resolvable ids', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    expect(screen.getByRole('tabpanel', { name: 'Root cause' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Call chain' }));
    expect(screen.getByRole('tabpanel', { name: 'Call chain' })).toBeInTheDocument();
    // ids are valid (no whitespace) and the tab controls the panel.
    const tab = screen.getByRole('tab', { name: 'Call chain' });
    const panel = screen.getByRole('tabpanel');
    expect(tab.id).not.toMatch(/\s/);
    expect(panel.id).not.toMatch(/\s/);
    expect(tab).toHaveAttribute('aria-controls', panel.id);
    expect(panel).toHaveAttribute('aria-labelledby', tab.id);
  });

  it('switches to the Call chain tab and shows the hop labels', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    fireEvent.click(screen.getByRole('tab', { name: 'Call chain' }));
    expect(screen.getByRole('tab', { name: 'Call chain' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('router: GET /api/notes/{id}')).toBeInTheDocument();
    expect(screen.getByText('get_note()')).toBeInTheDocument();
    expect(screen.getByText('NoteStore::find_by_id()')).toBeInTheDocument();
    // the hop file links jump into the project's Code tab
    expect(screen.getByRole('link', { name: 'src/routes/notes.rs:40-58' })).toHaveAttribute(
      'href',
      '/projects/ws_1/code?path=src%2Froutes%2Fnotes.rs&line=40',
    );
  });

  it('shows evidence claims, the patch diff, and repro steps in their tabs', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    fireEvent.click(screen.getByRole('tab', { name: 'Evidence' }));
    expect(screen.getByText('The handler looks the note up by id alone.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Patch' }));
    expect(screen.getByText(/find_by_id_for_owner/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Repro' }));
    expect(screen.getByText('Sign up as user B.')).toBeInTheDocument();
  });

  it('renders a sentinel patch as its label, not a diff', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(
      detailOf({ report: { ...report, recommended_patch: 'Not Provided — no fix yet' } }),
    );
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    fireEvent.click(screen.getByRole('tab', { name: 'Patch' }));
    expect(screen.getByText('Not provided: no fix yet')).toBeInTheDocument();
  });

  it('links to the full report page', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    expect(screen.getByRole('link', { name: /Open full report/ })).toHaveAttribute('href', '/findings/f-full');
  });

  it('shows a visible message when the detail fetch fails', async () => {
    vi.spyOn(api, 'getFinding').mockRejectedValue(new Error('boom: 500'));
    view(FULL);
    fireEvent.click(header());
    expect(await screen.findByText(/boom: 500/)).toBeInTheDocument();
    expect(screen.queryByRole('status')).toBeNull();
    // the stored rationale stays as the placeholder when the report can't load
    expect(screen.getAllByText(ROOT_CAUSE)).toHaveLength(1);
    // the full report link is still reachable so the user can retry there
    expect(screen.getByRole('link', { name: /Open full report/ })).toBeInTheDocument();
  });

  it('says the report cannot be displayed when a full-profile detail has no report', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf({ report: null }));
    view(FULL);
    fireEvent.click(header());
    expect(await screen.findByText(/can't display/)).toBeInTheDocument();
    expect(screen.queryByText(/no structured report body/)).toBeNull();
    expect(screen.queryByRole('tablist')).toBeNull();
  });

  it('shows the message from a JSON API error body, not the JSON', async () => {
    const body = JSON.stringify({ error: 'finding f-full not found' });
    vi.spyOn(api, 'getFinding').mockRejectedValue(new ApiError(404, body, body));
    view(FULL);
    fireEvent.click(header());
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Couldn’t load the report: finding f-full not found');
    expect(alert).not.toHaveTextContent('"error"');
  });

  it('drops a late response for a previous finding after the card is re-pointed at another id', async () => {
    const resolvers: Record<string, (d: FindingDetail) => void> = {};
    const spy = vi.spyOn(api, 'getFinding').mockImplementation(
      (id: string) => new Promise<FindingDetail>((r) => { resolvers[id] = r; }),
    );
    const A = { ...FULL, id: 'f-a', summary: 'Finding A' } as unknown as FindingRecord;
    const B = { ...FULL, id: 'f-b', summary: 'Finding B' } as unknown as FindingRecord;
    const reportFor = (root_cause: string) => ({ ...report, root_cause });
    const { rerender } = render(
      <MemoryRouter><InlineFindingCard finding={A} stale={false} /></MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: /Finding A/ }));
    expect(spy).toHaveBeenCalledWith('f-a');

    // Re-point the same card instance (still expanded) at B before A resolves.
    rerender(<MemoryRouter><InlineFindingCard finding={B} stale={false} /></MemoryRouter>);
    expect(spy).toHaveBeenCalledWith('f-b');
    expect(screen.getByRole('status')).toBeInTheDocument();

    // A's response lands late: B's card must not show A's content.
    await act(async () => {
      resolvers['f-a'](detailOf({ id: 'f-a', report: reportFor('ALPHA root cause text') }));
    });
    expect(screen.queryByText(/ALPHA root cause text/)).toBeNull();
    expect(screen.queryByRole('tablist')).toBeNull();
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open full report/ })).toHaveAttribute('href', '/findings/f-b');

    // B's own response then renders normally.
    await act(async () => {
      resolvers['f-b'](detailOf({ id: 'f-b', report: reportFor('BRAVO root cause text') }));
    });
    expect(await screen.findByText(/BRAVO root cause text/)).toBeInTheDocument();
    expect(screen.queryByText(/ALPHA root cause text/)).toBeNull();
  });

  it('keeps the fetched detail across collapse / re-expand (no refetch)', async () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByRole('tablist');
    fireEvent.click(header());
    fireEvent.click(header());
    expect(await screen.findByRole('tabpanel', { name: 'Root cause' })).toHaveTextContent(ROOT_CAUSE);
    expect(screen.getAllByText(ROOT_CAUSE)).toHaveLength(1);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('keeps the stale banner and permalink on a full card', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    const f = { ...FULL, permalink: 'https://github.com/o/r/blob/main/x#L1' } as unknown as FindingRecord;
    render(
      <MemoryRouter>
        <InlineFindingCard finding={f} stale={true} />
      </MemoryRouter>,
    );
    fireEvent.click(header());
    expect(screen.getByText(/code may have changed/i)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /view on repository/i })).toBeInTheDocument();
    await screen.findByRole('tablist');
  });
});

describe('InlineFindingCard — summary profile is unchanged', () => {
  it('never calls getFinding and renders no tabs', () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(SUMMARY);
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(screen.getByText('Summary rationale.')).toBeInTheDocument();
    expect(screen.queryByRole('tablist')).toBeNull();
    expect(screen.queryByRole('link', { name: /Open full report/ })).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it('treats a finding with no profile as summary', () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    const f = { ...SUMMARY, profile: undefined } as unknown as FindingRecord;
    view(f);
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(spy).not.toHaveBeenCalled();
  });
});
