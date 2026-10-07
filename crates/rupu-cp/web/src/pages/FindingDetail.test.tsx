// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
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
    codename: 'amber-delta/heron',
    codename_derived: false,
    evidence_status: [],
    tag_history: [],
    tags_editable: true,
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

  it('never fetches a Markdown image from an agent-written report field', async () => {
    const img = (n: string) => `![${n}](https://example.invalid/${n}.png)`;
    vi.spyOn(api, 'getFinding').mockResolvedValue(
      base({
        profile: 'full',
        evidence_status: ['current'],
        report: {
          ...report,
          description: img('desc'),
          impact: img('impact'),
          root_cause: `${img('root')} <img src="https://example.invalid/raw.png" onerror="alert(1)">`,
          remediation: img('fix'),
        },
      }),
    );
    const { container } = renderAt();
    await screen.findByRole('heading', { level: 1 });
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('[onerror]')).toBeNull();
    for (const n of ['desc', 'impact', 'root', 'fix']) {
      expect(screen.getByRole('link', { name: `[image: ${n}]` })).toHaveAttribute('href', `https://example.invalid/${n}.png`);
    }
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

  describe('per-finding export buttons', () => {
    let clicked: HTMLAnchorElement[];
    let createObjectURL: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      clicked = [];
      createObjectURL = vi.fn(() => 'blob:mock-url');
      Object.defineProperty(URL, 'createObjectURL', { value: createObjectURL, configurable: true, writable: true });
      Object.defineProperty(URL, 'revokeObjectURL', { value: vi.fn(), configurable: true, writable: true });
      vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
        clicked.push(this);
      });
    });

    async function renderFull() {
      vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'full', report, evidence_status: ['current'] }));
      renderAt();
      return screen.findByRole('group', { name: 'Export finding' });
    }

    it('offers Markdown / HTML / PDF buttons on a full report', async () => {
      const group = await renderFull();
      for (const name of ['Markdown', 'HTML', 'PDF']) {
        expect(within(group).getByRole('button', { name })).toBeEnabled();
      }
      // Buttons, not plain download links: a failure must be shown, not saved.
      expect(within(group).queryAllByRole('link')).toHaveLength(0);
    });

    it('offers the same buttons on the summary layout', async () => {
      vi.spyOn(api, 'getFinding').mockResolvedValue(base({ profile: 'summary', report: null }));
      renderAt();
      const group = await screen.findByRole('group', { name: 'Export finding' });
      for (const name of ['Markdown', 'HTML', 'PDF']) {
        expect(within(group).getByRole('button', { name })).toBeInTheDocument();
      }
    });

    it('keeps the buttons above the report header', async () => {
      const group = await renderFull();
      const h1 = screen.getByRole('heading', { level: 1 });
      expect(group.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    });

    it("fetches the format and saves it under the server's filename", async () => {
      const spy = vi.spyOn(api, 'downloadFindingExport').mockResolvedValue(new File(['x'], 'SEC-001-notes-idor.pdf'));
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'PDF' }));

      await waitFor(() => expect(clicked).toHaveLength(1));
      expect(spy).toHaveBeenCalledWith('fnd_1', 'pdf', { signal: expect.any(AbortSignal) });
      expect(clicked[0].download).toBe('SEC-001-notes-idor.pdf');
      expect(screen.queryByRole('alert')).toBeNull();
    });

    it('falls back to <id>.<ext> when the server names no file', async () => {
      vi.spyOn(api, 'downloadFindingExport').mockResolvedValue(new Blob(['x']));
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'Markdown' }));
      await waitFor(() => expect(clicked).toHaveLength(1));
      expect(clicked[0].download).toBe('fnd_1.md');
    });

    it('shows a 501 as a message next to the buttons and saves nothing', async () => {
      const body = JSON.stringify({ error: 'this build was compiled without PDF support' });
      vi.spyOn(api, 'downloadFindingExport').mockRejectedValue(new ApiError(501, body, body));
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'PDF' }));

      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent('this build was compiled without PDF support');
      expect(alert).not.toHaveTextContent('{');
      expect(group.parentElement).toContainElement(alert);
      expect(clicked).toHaveLength(0);
      expect(createObjectURL).not.toHaveBeenCalled();
      // Buttons are usable again.
      expect(within(group).getByRole('button', { name: 'PDF' })).toBeEnabled();
    });

    it('clears a stale error when another export starts', async () => {
      const spy = vi.spyOn(api, 'downloadFindingExport').mockRejectedValueOnce(new Error('first failed'));
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'HTML' }));
      await screen.findByRole('alert');

      spy.mockResolvedValue(new Blob(['x']));
      fireEvent.click(within(group).getByRole('button', { name: 'Markdown' }));
      await waitFor(() => expect(clicked).toHaveLength(1));
      expect(screen.queryByRole('alert')).toBeNull();
    });

    it('disables all three buttons while a download is in flight', async () => {
      let resolve!: (b: Blob) => void;
      const spy = vi.spyOn(api, 'downloadFindingExport').mockReturnValue(new Promise<Blob>((r) => { resolve = r; }));
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'HTML' }));

      for (const name of ['Markdown', 'HTML', 'PDF']) {
        expect(within(group).getByRole('button', { name })).toBeDisabled();
      }
      fireEvent.click(within(group).getByRole('button', { name: 'Markdown' }));
      expect(spy).toHaveBeenCalledTimes(1);

      resolve(new Blob(['x']));
      await waitFor(() => expect(within(group).getByRole('button', { name: 'HTML' })).toBeEnabled());
      expect(clicked).toHaveLength(1);
    });

    it('aborts an in-flight download when the page is left', async () => {
      let signal: AbortSignal | undefined;
      vi.spyOn(api, 'downloadFindingExport').mockImplementation((_id, _fmt, opts) => {
        signal = opts?.signal;
        return new Promise<Blob>(() => {});
      });
      const group = await renderFull();
      fireEvent.click(within(group).getByRole('button', { name: 'PDF' }));
      cleanup();
      expect(signal?.aborted).toBe(true);
    });
  });

  describe('tags', () => {
    const removed = {
      workspaces: [{ ws_id: 'ws1', outcomes: [{ finding_id: 'fnd_1', before: ['needs-poc'], after: [] }] }],
      unknown: [],
    };

    beforeEach(() => {
      vi.spyOn(api, 'getTagsInUse').mockResolvedValue([]);
    });

    it('removes a tag through the API and refetches the finding', async () => {
      const get = vi.spyOn(api, 'getFinding').mockResolvedValue(
        base({ profile: 'summary', report: null, tags: ['needs-poc'], tags_editable: true }),
      );
      const tag = vi.spyOn(api, 'tagFindings').mockResolvedValue(removed);
      renderAt();

      expect(await screen.findByText('needs-poc')).toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: 'Remove tag needs-poc' }));

      await waitFor(() => expect(tag).toHaveBeenCalledWith(['fnd_1'], { remove: ['needs-poc'] }));
      await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    });

    it('shows tags read-only, with no remove button, when the tag log is unreadable', async () => {
      vi.spyOn(api, 'getFinding').mockResolvedValue(
        base({ profile: 'summary', report: null, tags: ['needs-poc'], tags_editable: false }),
      );
      renderAt();

      expect(await screen.findByText('needs-poc')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /Remove tag/ })).toBeNull();
      expect(screen.getByText('tags read-only')).toBeInTheDocument();
    });

    it('shows a workspace error as an alert and does not refetch', async () => {
      const get = vi.spyOn(api, 'getFinding').mockResolvedValue(
        base({ profile: 'full', report, evidence_status: ['current'], tags: ['needs-poc'], tags_editable: true }),
      );
      vi.spyOn(api, 'tagFindings').mockResolvedValue({
        workspaces: [{ ws_id: 'ws1', error: 'tag log is locked' }],
        unknown: [],
      });
      renderAt();

      fireEvent.click(await screen.findByRole('button', { name: 'Remove tag needs-poc' }));

      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent("demo's tags couldn't be changed: tag log is locked.");
      expect(get).toHaveBeenCalledTimes(1);
    });

    it("reports a saved change whose refetch failed as saved, not failed", async () => {
      const get = vi.spyOn(api, 'getFinding')
        .mockResolvedValueOnce(base({ profile: 'summary', report: null, tags: [], tags_editable: true }))
        .mockRejectedValueOnce(new Error('network down'));
      vi.spyOn(api, 'tagFindings').mockResolvedValue({
        workspaces: [{ ws_id: 'ws1', outcomes: [{ finding_id: 'fnd_1', before: [], after: ['triaged'] }] }],
        unknown: [],
      });
      renderAt();

      fireEvent.click(await screen.findByRole('button', { name: /tag$/ }));
      const box = screen.getByRole('combobox', { name: 'Add tag' });
      fireEvent.change(box, { target: { value: 'triaged' } });
      fireEvent.keyDown(box, { key: 'Enter' });

      const note = await screen.findByRole('status');
      expect(note).toHaveTextContent("Saved, but the page couldn't refresh: network down");
      expect(get).toHaveBeenCalledTimes(2);
      expect(screen.queryByRole('alert')).toBeNull();
      expect(screen.queryByRole('combobox', { name: 'Add tag' })).toBeNull();
      // The chips follow the write's own answer.
      expect(screen.getByText('triaged')).toBeInTheDocument();
    });

    it('refetches after a partly applied change and still shows its error', async () => {
      const get = vi.spyOn(api, 'getFinding')
        .mockResolvedValueOnce(base({ profile: 'summary', report: null, tags: ['needs-poc'], tags_editable: true }))
        .mockResolvedValueOnce(base({ profile: 'summary', report: null, tags: [], tags_editable: true }));
      vi.spyOn(api, 'tagFindings').mockResolvedValue({
        workspaces: [
          { ws_id: 'ws1', outcomes: [{ finding_id: 'fnd_1', before: ['needs-poc'], after: [] }] },
          { ws_id: 'ws2', error: 'tag log is locked' },
        ],
        unknown: [],
      });
      renderAt();

      fireEvent.click(await screen.findByRole('button', { name: 'Remove tag needs-poc' }));

      await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent("ws2's tags couldn't be changed: tag log is locked.");
      await waitFor(() => expect(screen.queryByText('needs-poc')).toBeNull());
    });

    it("drops a tag change's refetch once the page has moved to another finding", async () => {
      let resolveRefetch: ((d: Detail) => void) | null = null;
      let loadsOfA = 0;
      vi.spyOn(api, 'getFinding').mockImplementation((id: string) => {
        if (id === 'fnd_2') {
          return Promise.resolve(base({ id: 'fnd_2', summary: 'Second finding', profile: 'summary', report: null }));
        }
        loadsOfA += 1;
        if (loadsOfA === 1) {
          return Promise.resolve(base({ summary: 'First finding', profile: 'summary', report: null, tags: ['needs-poc'] }));
        }
        return new Promise<Detail>((r) => { resolveRefetch = r; });
      });
      vi.spyOn(api, 'tagFindings').mockResolvedValue(removed);
      function Nav() {
        const navigate = useNavigate();
        return <button type="button" onClick={() => navigate('/findings/fnd_2')}>go-b</button>;
      }
      render(
        <MemoryRouter initialEntries={['/findings/fnd_1']}>
          <Routes>
            <Route path="/findings/:id" element={<><Nav /><FindingDetail /></>} />
          </Routes>
        </MemoryRouter>,
      );

      fireEvent.click(await screen.findByRole('button', { name: 'Remove tag needs-poc' }));
      await waitFor(() => expect(resolveRefetch).not.toBeNull());
      fireEvent.click(screen.getByText('go-b'));
      expect(await screen.findByRole('heading', { level: 1, name: 'Second finding' })).toBeInTheDocument();

      await act(async () => {
        resolveRefetch!(base({ summary: 'First finding', profile: 'summary', report: null, tags: [] }));
      });
      expect(screen.getByRole('heading', { level: 1, name: 'Second finding' })).toBeInTheDocument();
      expect(screen.queryByText('First finding')).toBeNull();
    });

    it('lists the tag history in a disclosure', async () => {
      vi.spyOn(api, 'getFinding').mockResolvedValue(
        base({
          profile: 'summary',
          report: null,
          tags: ['needs-poc'],
          tag_history: [{
            id: 'tev_1',
            finding_id: 'fnd_1',
            op: 'add',
            tag: 'needs-poc',
            by: { kind: 'agent', run_id: 'run_42', model: 'claude-x', surface: 'agent', codename: 'cobalt-harbor/heron', agent: 'recon' },
            at: '2026-08-02T00:00:00Z',
          }],
        }),
      );
      renderAt();

      const summary = await screen.findByText('Tag history (1)');
      const details = summary.closest('details')!;
      expect(within(details).getByText('+ needs-poc')).toBeInTheDocument();
      expect(within(details).getByText('cobalt-harbor/heron (recon)')).toBeInTheDocument();
    });

    it("loads the project's tags in use as suggestions", async () => {
      vi.spyOn(api, 'getFinding').mockResolvedValue(
        base({ profile: 'summary', report: null, tags: [], tags_editable: true }),
      );
      const inUse = vi.spyOn(api, 'getTagsInUse').mockResolvedValue([{ tag: 'needs-poc', count: 3 }]);
      renderAt();

      fireEvent.click(await screen.findByRole('button', { name: /tag$/ }));
      await waitFor(() => expect(inUse).toHaveBeenCalledWith({ wsId: 'ws1' }));
      fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: 'needs' } });
      expect(await screen.findByText('needs-poc')).toBeInTheDocument();
    });
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
