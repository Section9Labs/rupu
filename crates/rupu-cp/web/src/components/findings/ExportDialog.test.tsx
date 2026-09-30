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
    expect(spy).toHaveBeenCalledWith(
      {
        format: 'pdf',
        title: 'Notebin findings',
        ids: ['f1', 'f2'],
        include_summaries: false,
        split: true,
      },
      { signal: expect.any(AbortSignal) },
    );
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('sends every row, summaries included, when "Include summary findings" is on', async () => {
    const spy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    setup();

    fireEvent.click(screen.getByRole('checkbox', { name: /Include summary findings/ }));
    fireEvent.click(exportButton());

    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy.mock.calls[0][0]).toEqual(
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
    // Revoked on a long delay (see lib/download.test.ts), not while the browser
    // may still be starting the download.
    expect(revokeObjectURL).not.toHaveBeenCalled();
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

  /** An `exportFindings` that stays pending until its signal aborts. */
  function stalledExport() {
    let signal: AbortSignal | undefined;
    const spy = vi.spyOn(api, 'exportFindings').mockImplementation((_body, opts) => {
      signal = opts?.signal;
      return new Promise<Blob>((_resolve, reject) => {
        opts?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
      });
    });
    return { spy, aborted: () => signal?.aborted ?? false };
  }

  it('Cancel during a pending export aborts the request and closes', async () => {
    const { aborted } = stalledExport();
    const { onClose } = setup();
    fireEvent.click(exportButton());
    expect(exportButton()).toBeDisabled();

    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toBeEnabled();
    fireEvent.click(cancel);

    expect(aborted()).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
    // The abort is not surfaced as an error, and nothing is saved.
    await Promise.resolve();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(clicked).toHaveLength(0);
  });

  it('Escape during a pending export aborts the request and closes', () => {
    const { aborted } = stalledExport();
    const { onClose } = setup();
    fireEvent.click(exportButton());
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(aborted()).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('an overlay click during a pending export aborts the request and closes', () => {
    const { aborted } = stalledExport();
    const { onClose } = setup();
    fireEvent.click(exportButton());
    fireEvent.mouseDown(screen.getByTestId('export-overlay'));
    expect(aborted()).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('aborts an in-flight export when the dialog is unmounted by its owner', () => {
    const { aborted } = stalledExport();
    const { unmount } = setup();
    fireEvent.click(exportButton());
    unmount();
    expect(aborted()).toBe(true);
  });

  it('does not save a response that arrives after the dialog was cancelled', async () => {
    let resolve!: (b: Blob) => void;
    vi.spyOn(api, 'exportFindings').mockReturnValue(new Promise<Blob>((r) => { resolve = r; }));
    setup();
    fireEvent.click(exportButton());
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    resolve(new Blob(['late']));
    await Promise.resolve();
    await Promise.resolve();
    expect(clicked).toHaveLength(0);
    expect(createObjectURL).not.toHaveBeenCalled();
  });

  describe('keyboard focus', () => {
    const tab = (el: Element, shiftKey = false) => fireEvent.keyDown(el, { key: 'Tab', shiftKey });

    it('keeps Shift+Tab inside the dialog after a non-first radio is chosen', () => {
      setup();
      const pdf = screen.getByRole('radio', { name: 'PDF' });
      fireEvent.click(pdf);
      pdf.focus();
      // A radio group is one tab stop, on the checked radio: PDF is now the
      // dialog's first stop, so Shift+Tab from it must wrap to the last one.
      const notPrevented = tab(pdf, true);
      expect(notPrevented).toBe(false);
      expect(exportButton()).toHaveFocus();
    });

    it('wraps Tab from the last control to the CHECKED radio, not the first one', () => {
      setup();
      const html = screen.getByRole('radio', { name: 'HTML' });
      fireEvent.click(html);
      exportButton().focus();
      expect(tab(exportButton())).toBe(false);
      expect(html).toHaveFocus();
    });

    it('lets Tab move normally between controls in the middle of the dialog', () => {
      setup();
      const title = screen.getByLabelText('Title');
      title.focus();
      expect(tab(title)).toBe(true);
      expect(tab(title, true)).toBe(true);
    });

    it('pulls focus back when it escapes to something outside the dialog', () => {
      render(
        <>
          <button>outside</button>
          <ExportDialog open onClose={() => {}} findings={ROWS} defaultTitle="T" />
        </>,
      );
      const radio = screen.getByRole('radio', { name: 'Markdown' });
      radio.focus();
      screen.getByRole('button', { name: 'outside' }).focus();
      expect(screen.getByRole('dialog').contains(document.activeElement)).toBe(true);
    });

    it('puts focus back on Export after a failed export', async () => {
      vi.spyOn(api, 'exportFindings').mockRejectedValue(new Error('boom'));
      setup();
      // Focus is on the first radio, which (with the rest of the form) is
      // disabled while the export runs; the Export button is disabled too.
      fireEvent.click(exportButton());
      expect(exportButton()).not.toHaveFocus();
      await screen.findByRole('alert');
      expect(exportButton()).toBeEnabled();
      expect(exportButton()).toHaveFocus();
    });
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
