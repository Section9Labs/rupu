// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ImportFindingsDialog } from './ImportFindingsDialog';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

const DIR = '/srv/reports/northwind';
const FILES = [
  { path: `${DIR}/FND-004-auth-bypass.md`, status: 'attached', finding_id: 'fnd_01HZAAAA' },
  { path: `${DIR}/FND-007-idor.md`, status: 'attached', finding_id: 'fnd_01J0BBBB' },
  { path: `${DIR}/README.md`, status: 'skipped', reason: 'not a finding report' },
  { path: `${DIR}/FND-012-xss.md`, status: 'failed', reason: 'no finding with id FND-012', details: ['line 3: no Finding ID'] },
];

function report(dryRun: boolean) {
  return { dry_run: dryRun, files: FILES, backups: dryRun ? [] : ['/w/.rupu/coverage/findings.jsonl.pre-import-1'], attached: 2, skipped: 1, failed: 1 };
}

function server(answer: (url: string, body: Record<string, unknown> | null) => Response) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) =>
    answer(String(input), init?.body ? (JSON.parse(init.body as string) as Record<string, unknown>) : null),
  );
}

const sent = (m: ReturnType<typeof vi.spyOn>, n: number) => JSON.parse((m.mock.calls[n][1] as RequestInit).body as string);

function renderDialog(onImported = vi.fn()) {
  render(<ImportFindingsDialog onClose={() => {}} onImported={onImported} />);
  return onImported;
}

describe('ImportFindingsDialog', () => {
  it('runs a dry run first and lists each file with its outcome', async () => {
    const fetchMock = server((_u, body) => jsonResponse(report(Boolean(body?.dry_run))));
    renderDialog();
    expect(screen.queryByRole('button', { name: /^Import \d/ })).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Files or directories on this machine'), { target: { value: ` ${DIR} \n\n/srv/one.md` } });
    fireEvent.click(screen.getByRole('button', { name: 'Dry run' }));

    const list = await screen.findByRole('list', { name: 'Files' });
    expect(sent(fetchMock, 0)).toEqual({ paths: [DIR, '/srv/one.md'], dry_run: true });
    expect(screen.getByRole('status')).toHaveTextContent('Dry run: 2 attach · 1 skipped · 1 failed. Nothing has been written yet.');
    const rows = within(list).getAllByRole('listitem');
    expect(rows[0]).toHaveTextContent('attach');
    expect(rows[0]).toHaveTextContent('FND-004-auth-bypass.md');
    expect(rows[0]).toHaveTextContent('fnd_01HZAAAA');
    expect(rows[2]).toHaveTextContent('not a finding report');
    expect(rows[3]).toHaveTextContent('fail');
    expect(rows[3]).toHaveTextContent('line 3: no Finding ID');
    expect(screen.getByRole('button', { name: 'Import 2 reports' })).toBeEnabled();
  });

  it('imports for real, shows the result and tells the owner to reload', async () => {
    const fetchMock = server((_u, body) => jsonResponse(report(Boolean(body?.dry_run))));
    const onImported = renderDialog();
    fireEvent.change(screen.getByLabelText('Files or directories on this machine'), { target: { value: DIR } });
    fireEvent.click(screen.getByRole('button', { name: 'Dry run' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Import 2 reports' }));

    expect(await screen.findByText(/Imported: 2 attached · 1 skipped · 1 failed\./)).toBeInTheDocument();
    expect(sent(fetchMock, 1)).toEqual({ paths: [DIR], dry_run: false });
    expect(screen.getByText(/findings\.jsonl\.pre-import-1/)).toBeInTheDocument();
    expect(onImported).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Close' })).toBeInTheDocument();
  });

  it('has nothing to import when the dry run attaches nothing', async () => {
    server(() => jsonResponse({ dry_run: true, files: [FILES[2]], backups: [], attached: 0, skipped: 1, failed: 0 }));
    renderDialog();
    fireEvent.change(screen.getByLabelText('Files or directories on this machine'), { target: { value: DIR } });
    fireEvent.click(screen.getByRole('button', { name: 'Dry run' }));

    expect(await screen.findByRole('button', { name: 'Import 0 reports' })).toBeDisabled();
  });

  it('shows a refusal inline and forgets the dry run when the paths change', async () => {
    server(() => jsonResponse({ error: 'srv/x: a path must be absolute' }, 400));
    renderDialog();
    const paths = screen.getByLabelText('Files or directories on this machine');
    fireEvent.change(paths, { target: { value: 'srv/x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Dry run' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('a path must be absolute');
    fireEvent.change(paths, { target: { value: '/srv/x' } });
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('opens the report schema read-only and comes back to the import', async () => {
    server((url) => (url === '/api/findings/schema' ? jsonResponse({ title: 'FindingReport', type: 'object' }) : jsonResponse({}, 500)));
    renderDialog();
    fireEvent.change(screen.getByLabelText('Files or directories on this machine'), { target: { value: DIR } });

    fireEvent.click(screen.getByRole('button', { name: 'Report schema ↗' }));

    const schema = await screen.findByLabelText('Finding report schema');
    await waitFor(() => expect(schema).toHaveTextContent('"title": "FindingReport"'));
    fireEvent.click(screen.getByRole('button', { name: 'Back to import' }));
    expect(screen.getByLabelText('Files or directories on this machine')).toHaveValue(DIR);
  });
});
