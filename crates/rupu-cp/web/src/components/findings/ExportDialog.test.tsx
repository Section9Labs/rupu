// @vitest-environment jsdom
// ExportDialog — the "Export report" modal over the currently filtered
// findings. `api.exportFindings` is spied; object URLs and the anchor click
// (which would navigate in jsdom) are stubbed so the download hand-off can be
// asserted.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { api, ApiError } from '../../lib/api';
import { ExportDialog } from './ExportDialog';

const ROWS = [
  { id: 'f1', profile: 'full' as const },
  { id: 'f2', profile: 'full' as const },
  { id: 'f3', profile: 'summary' as const },
  { id: 'f4' }, // legacy row without a profile: a summary, same as the list filter treats it
];

let clicked: HTMLAnchorElement[] = [];
let createObjectURL: ReturnType<typeof vi.fn>;
let revokeObjectURL: ReturnType<typeof vi.fn>;

beforeEach(() => {
  clicked = [];
  createObjectURL = vi.fn(() => 'blob:mock-url');
  revokeObjectURL = vi.fn();
  Object.defineProperty(URL, 'createObjectURL', { value: createObjectURL, configurable: true, writable: true });
  Object.defineProperty(URL, 'revokeObjectURL', { value: revokeObjectURL, configurable: true, writable: true });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    clicked.push(this);
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function setup(over: Partial<React.ComponentProps<typeof ExportDialog>> = {}) {
  const onClose = vi.fn();
  const utils = render(
    <ExportDialog open onClose={onClose} findings={ROWS} defaultTitle="Notebin findings" {...over} />,
  );
  return { onClose, ...utils };
}

const exportButton = () => screen.getByRole('button', { name: 'Export' }) as HTMLButtonElement;

describe('ExportDialog', () => {
  it('renders nothing while closed', () => {
    render(<ExportDialog open={false} onClose={() => {}} findings={ROWS} defaultTitle="T" />);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('is an accessible modal dialog with its first control focused', () => {
    setup();
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    const labelledBy = dialog.getAttribute('aria-labelledby');
    expect(labelledBy).toBeTruthy();
    expect(document.getElementById(labelledBy!)).toHaveTextContent('Export report');
    expect(screen.getByRole('radio', { name: 'Markdown' })).toHaveFocus();
  });

  it('exports the chosen format as one zip of full reports', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    const { onClose } = setup();

    fireEvent.click(screen.getByRole('radio', { name: 'PDF' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'One file per finding (zip)' }));
    fireEvent.click(exportButton());

    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    // Summary rows are left out while "Include summary findings" is off, so the
    // toggle means something: naming a summary id server-side would opt it in.
    expect(spy).toHaveBeenCalledWith({
      format: 'pdf',
      title: 'Notebin findings',
      ids: ['f1', 'f2'],
      include_summaries: false,
      split: true,
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('sends every row, summaries included, when "Include summary findings" is on', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup();

    fireEvent.click(screen.getByRole('checkbox', { name: /Include summary findings/ }));
    fireEvent.click(exportButton());

    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ format: 'md', ids: ['f1', 'f2', 'f3', 'f4'], include_summaries: true, split: false }),
    );
  });

  it('counts the summary rows in the label', () => {
    setup();
    expect(screen.getByRole('checkbox', { name: 'Include summary findings (2)' })).not.toBeChecked();
  });

  it('disables the summary toggle when the set has no summaries', () => {
    setup({ findings: [{ id: 'f1', profile: 'full' }] });
    expect(screen.getByRole('checkbox', { name: 'Include summary findings (0)' })).toBeDisabled();
  });

  it('sends the trimmed title, and omits it when blank', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup();

    const title = screen.getByLabelText('Title');
    expect(title).toHaveValue('Notebin findings');
    fireEvent.change(title, { target: { value: '  Q3 review  ' } });
    fireEvent.click(exportButton());
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].title).toBe('Q3 review');

    fireEvent.change(title, { target: { value: '   ' } });
    fireEvent.click(exportButton());
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy.mock.calls[1][0]).not.toHaveProperty('title');
  });

  it('scopes the request to the project when given a wsId', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup({ wsId: 'ws-1' });
    fireEvent.click(exportButton());
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy.mock.calls[0][0].ws_id).toBe('ws-1');
  });

  it('hands the Blob to a hidden download anchor, then revokes the URL', async () => {
    vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup();
    fireEvent.click(screen.getByRole('radio', { name: 'HTML' }));
    fireEvent.click(exportButton());

    await waitFor(() => expect(clicked).toHaveLength(1));
    expect(createObjectURL).toHaveBeenCalledTimes(1);
    const a = clicked[0];
    expect(a.getAttribute('href')).toBe('blob:mock-url');
    expect(a.download).toBe('findings-report.html');
    expect(a.hidden).toBe(true);
    await waitFor(() => expect(revokeObjectURL).toHaveBeenCalledWith('blob:mock-url'));
    // The anchor is not left behind in the document.
    expect(document.body.contains(a)).toBe(false);
  });

  it('names a split export .zip', async () => {
    vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup();
    fireEvent.click(screen.getByRole('checkbox', { name: 'One file per finding (zip)' }));
    fireEvent.click(exportButton());
    await waitFor(() => expect(clicked).toHaveLength(1));
    expect(clicked[0].download).toBe('findings-report.zip');
  });

  it("uses the server's filename when the response carries one", async () => {
    vi.spyOn(api, 'exportFindings').mockResolvedValue(new File(['x'], 'notebin-findings.pdf'));
    setup();
    fireEvent.click(screen.getByRole('radio', { name: 'PDF' }));
    fireEvent.click(exportButton());
    await waitFor(() => expect(clicked).toHaveLength(1));
    expect(clicked[0].download).toBe('notebin-findings.pdf');
  });

  it('shows a rejected export inline and stays open for another try', async () => {
    const spy = vi
      .spyOn(api, 'exportFindings')
      .mockRejectedValue(
        new ApiError(501, '{"error":"this build was compiled without PDF support"}', '{"error":"this build was compiled without PDF support"}'),
      );
    const { onClose } = setup();
    fireEvent.click(screen.getByRole('radio', { name: 'PDF' }));
    fireEvent.click(exportButton());

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('this build was compiled without PDF support');
    expect(alert).not.toHaveTextContent('{');
    expect(onClose).not.toHaveBeenCalled();
    expect(exportButton()).toBeEnabled();
    expect(createObjectURL).not.toHaveBeenCalled();

    // A retry clears the stale error while it runs.
    spy.mockResolvedValue(new Blob(['x']));
    fireEvent.click(screen.getByRole('radio', { name: 'Markdown' }));
    fireEvent.click(exportButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('blocks a second submit while an export is in flight', async () => {
    let resolve!: (b: Blob) => void;
    const spy = vi.spyOn(api, 'exportFindings').mockReturnValue(new Promise<Blob>((r) => { resolve = r; }));
    setup();
    fireEvent.click(exportButton());
    expect(exportButton()).toBeDisabled();
    fireEvent.click(exportButton());
    expect(spy).toHaveBeenCalledTimes(1);
    resolve(new Blob(['x']));
    await waitFor(() => expect(clicked).toHaveLength(1));
  });

  it('never sends an empty id list (the server reads it as "no restriction")', () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup({ findings: [] });
    expect(exportButton()).toBeDisabled();
    expect(screen.getByText('No findings to export')).toBeInTheDocument();
    fireEvent.click(exportButton());
    expect(spy).not.toHaveBeenCalled();
  });

  it('explains when only summary rows remain and the toggle is off', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup({ findings: [{ id: 's1', profile: 'summary' }, { id: 's2' }] });

    expect(exportButton()).toBeDisabled();
    expect(screen.getByText(/only summary findings/i)).toBeInTheDocument();
    fireEvent.click(exportButton());
    expect(spy).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('checkbox', { name: /Include summary findings/ }));
    expect(exportButton()).toBeEnabled();
    fireEvent.click(exportButton());
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy.mock.calls[0][0].ids).toEqual(['s1', 's2']);
  });

  it('closes on Escape', () => {
    const { onClose } = setup();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes on an overlay click but not on a click inside the panel', () => {
    const { onClose } = setup();
    fireEvent.mouseDown(within(screen.getByRole('dialog')).getByLabelText('Title'));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.mouseDown(screen.getByTestId('export-overlay'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes from Cancel', () => {
    const { onClose } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('ignores Escape and the overlay while an export is running', () => {
    vi.spyOn(api, 'exportFindings').mockReturnValue(new Promise<Blob>(() => {}));
    const { onClose } = setup();
    fireEvent.click(exportButton());
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.mouseDown(screen.getByTestId('export-overlay'));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('starts from a clean form each time it is opened', () => {
    const onClose = vi.fn();
    const { rerender } = render(<ExportDialog open onClose={onClose} findings={ROWS} defaultTitle="First" />);
    fireEvent.click(screen.getByRole('radio', { name: 'PDF' }));
    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'edited' } });
    rerender(<ExportDialog open={false} onClose={onClose} findings={ROWS} defaultTitle="First" />);
    rerender(<ExportDialog open onClose={onClose} findings={ROWS} defaultTitle="Second" />);
    expect(screen.getByRole('radio', { name: 'Markdown' })).toBeChecked();
    expect(screen.getByLabelText('Title')).toHaveValue('Second');
  });
});
