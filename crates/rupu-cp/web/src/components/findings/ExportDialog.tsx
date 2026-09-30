// ExportDialog — the "Export report" modal for a set of findings. The caller
// passes the rows it is currently showing (after its filters), so the report
// covers exactly what is on screen. Choosing a format, an optional zip (one
// file per finding) and whether to keep summary-profile findings, then
// Export, POSTs the selection to `/api/findings/export` and hands the bytes
// to the browser as a download.
//
// Summary findings: the server treats a NAMED summary finding as asked-for
// whatever `include_summaries` says, so with the toggle off the summary rows'
// ids are left out of the request — otherwise the toggle would do nothing.
// An empty `ids` list means "no restriction" server-side (every finding), so
// it is never sent: with nothing to export the button is disabled instead.

import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import {
  api,
  apiErrorMessage,
  type FindingExportFormat,
  type FindingOut,
  type FindingsExportBody,
} from '../../lib/api';
import { Button } from '../ui/Button';
import { ErrorBanner } from '../ui/ErrorBanner';

/** What the dialog needs of a row: its id and whether it is a full report.
 *  A row with no `profile` is a summary, as the list filters treat it. */
export type ExportableFinding = Pick<FindingOut, 'id' | 'profile'>;

export interface ExportDialogProps {
  open: boolean;
  onClose: () => void;
  /** The currently filtered rows. */
  findings: readonly ExportableFinding[];
  /** Pre-fills the Title field. */
  defaultTitle: string;
  /** Scopes the request to one project (the project Findings tab). */
  wsId?: string;
}

const FORMATS: { value: FindingExportFormat; label: string }[] = [
  { value: 'md', label: 'Markdown' },
  { value: 'html', label: 'HTML' },
  { value: 'pdf', label: 'PDF' },
];

export const NO_FINDINGS_TEXT = 'No findings to export';

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';
const checkLabelCls = 'flex items-center gap-2 text-lead text-ink';

const FOCUSABLE = 'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])';

/** Offer `blob` to the browser as a download named `name`. */
function saveBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.hidden = true;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // After the click has been handed to the browser; revoking inside the same
  // task can cancel the download in some engines.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function ExportDialog({ open, onClose, findings, defaultTitle, wsId }: ExportDialogProps) {
  // The form lives in a child that only exists while open, so every opening
  // starts from a clean state (default format, fresh title, no stale error).
  if (!open) return null;
  return <ExportForm onClose={onClose} findings={findings} defaultTitle={defaultTitle} wsId={wsId} />;
}

function ExportForm({
  onClose,
  findings,
  defaultTitle,
  wsId,
}: Omit<ExportDialogProps, 'open'>) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const firstRef = useRef<HTMLInputElement>(null);

  const [format, setFormat] = useState<FindingExportFormat>('md');
  const [split, setSplit] = useState(false);
  const [includeSummaries, setIncludeSummaries] = useState(false);
  const [title, setTitle] = useState(defaultTitle);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const summaryCount = findings.filter((f) => f.profile !== 'full').length;
  const ids = (includeSummaries ? findings : findings.filter((f) => f.profile === 'full')).map((f) => f.id);
  const canExport = ids.length > 0;

  // Focus the first control on open and give focus back to the opener on close.
  useEffect(() => {
    const opener = document.activeElement;
    firstRef.current?.focus();
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  // Escape closes — but not mid-export, which would orphan the download.
  useEffect(() => {
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === 'Escape' && !busy) onClose();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [busy, onClose]);

  // Keep Tab inside the dialog (it is modal).
  function trapTab(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== 'Tab' || !panelRef.current) return;
    const items = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => !el.hasAttribute('disabled'),
    );
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (busy || !canExport) return;
    const body: FindingsExportBody = { format, ids, include_summaries: includeSummaries, split };
    const t = title.trim();
    if (t) body.title = t;
    if (wsId) body.ws_id = wsId;
    setBusy(true);
    setError(null);
    try {
      const blob = await api.exportFindings(body);
      // The server names the download (carried on a File); else a sensible default.
      const name = blob instanceof File && blob.name ? blob.name : `findings-report.${split ? 'zip' : format}`;
      saveBlob(blob, name);
      setBusy(false);
      onClose();
    } catch (err: unknown) {
      setError(apiErrorMessage(err));
      setBusy(false);
    }
  }

  return (
    <div
      data-testid="export-overlay"
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/30 p-4 pt-[10vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={trapTab}
        className="w-full max-w-md rounded-xl border border-border bg-panel p-5 shadow-card"
      >
        <h2 id={titleId} className="text-base font-semibold text-ink">
          Export report
        </h2>
        <p className="mt-1 text-ui text-ink-mute">
          {findings.length} {findings.length === 1 ? 'finding' : 'findings'} in the current view.
        </p>

        <form onSubmit={onSubmit} className="mt-4 space-y-4">
          <fieldset disabled={busy}>
            <legend className={labelCls}>Format</legend>
            <div className="flex flex-wrap gap-x-5 gap-y-1">
              {FORMATS.map((f, i) => (
                <label key={f.value} className={checkLabelCls}>
                  <input
                    ref={i === 0 ? firstRef : undefined}
                    type="radio"
                    name="export-format"
                    value={f.value}
                    checked={format === f.value}
                    onChange={() => setFormat(f.value)}
                    className="accent-brand-600"
                  />
                  {f.label}
                </label>
              ))}
            </div>
          </fieldset>

          <div className="space-y-1.5">
            <label className={checkLabelCls}>
              <input
                type="checkbox"
                checked={split}
                onChange={(e) => setSplit(e.target.checked)}
                disabled={busy}
                className="accent-brand-600"
              />
              One file per finding (zip)
            </label>
            <label className={checkLabelCls}>
              <input
                type="checkbox"
                checked={includeSummaries}
                onChange={(e) => setIncludeSummaries(e.target.checked)}
                disabled={busy || summaryCount === 0}
                className="accent-brand-600"
              />
              {`Include summary findings (${summaryCount})`}
            </label>
          </div>

          <label className="block">
            <span className={labelCls}>Title</span>
            <input
              type="text"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              disabled={busy}
              className={fieldCls}
            />
          </label>

          {!canExport && (
            <p className="text-ui text-ink-mute">
              {findings.length === 0
                ? NO_FINDINGS_TEXT
                : 'This view has only summary findings. Turn on "Include summary findings" to export them.'}
            </p>
          )}

          {error && <ErrorBanner>{error}</ErrorBanner>}

          <div className="flex items-center justify-end gap-2">
            {busy && (
              <span role="status" className="mr-auto text-ui text-ink-mute">
                Exporting…
              </span>
            )}
            <Button variant="secondary" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" disabled={busy || !canExport}>
              Export
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}

export default ExportDialog;
