// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import fixture from '../../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { api, type FindingDetail, type FindingOut, type FindingRecord } from '../../lib/api';
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
  evidence: { rationale: 'Rationale prose for the card.' },
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
    expect(await screen.findByText(/owner id from the session is never part/)).toBeInTheDocument();
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.getByRole('tab', { name: 'Root cause' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tablist')).toBeInTheDocument();
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual([
      'Root cause', 'Call chain', 'Evidence', 'Patch', 'Repro',
    ]);
  });

  it('switches to the Call chain tab and shows the hop labels', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByText(/owner id from the session/);
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
    await screen.findByText(/owner id from the session/);
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
    await screen.findByText(/owner id from the session/);
    fireEvent.click(screen.getByRole('tab', { name: 'Patch' }));
    expect(screen.getByText('Not provided: no fix yet')).toBeInTheDocument();
  });

  it('links to the full report page', async () => {
    vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByText(/owner id from the session/);
    expect(screen.getByRole('link', { name: /Open full report/ })).toHaveAttribute('href', '/findings/f-full');
  });

  it('shows a visible message when the detail fetch fails', async () => {
    vi.spyOn(api, 'getFinding').mockRejectedValue(new Error('boom: 500'));
    view(FULL);
    fireEvent.click(header());
    expect(await screen.findByText(/boom: 500/)).toBeInTheDocument();
    expect(screen.queryByRole('status')).toBeNull();
    // the full report link is still reachable so the user can retry there
    expect(screen.getByRole('link', { name: /Open full report/ })).toBeInTheDocument();
  });

  it('does not set state or refetch after collapse / unmount', async () => {
    let resolve!: (d: FindingDetail) => void;
    const spy = vi.spyOn(api, 'getFinding').mockReturnValue(new Promise((r) => { resolve = r; }));
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { unmount } = view(FULL);
    fireEvent.click(header());
    unmount();
    resolve(detailOf());
    await Promise.resolve();
    expect(errSpy).not.toHaveBeenCalled();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('keeps the fetched detail across collapse / re-expand (no refetch)', async () => {
    const spy = vi.spyOn(api, 'getFinding').mockResolvedValue(detailOf());
    view(FULL);
    fireEvent.click(header());
    await screen.findByText(/owner id from the session/);
    fireEvent.click(header());
    fireEvent.click(header());
    expect(await screen.findByText(/owner id from the session/)).toBeInTheDocument();
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
    await screen.findByText(/owner id from the session/);
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
