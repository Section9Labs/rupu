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
//
// Lifecycle: Cancel, Escape and an overlay click all close the dialog, and
// while an export is running they also abort its request (a stalled server
// must not lock the dialog). Focus: the first control is focused on open, Tab
// is trapped inside, and returning focus to whatever opened the dialog is the
// OWNER's job (see ExportReportButton) — it knows its trigger, and a mouse
// click does not focus a button in every browser.

import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import {
  api,
  apiErrorMessage,
  type FindingExportFormat,
  type FindingOut,
  type FindingsExportBody,
} from '../../lib/api';
import { saveBlob } from '../../lib/download';
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

/** The controls Tab visits, in order. A radio group is ONE tab stop — its
 *  checked member (else its first) — so only that radio is listed; treating
 *  every radio as a stop would leave Shift+Tab from a non-first checked radio
 *  free to walk out of the dialog. */
function tabStops(root: HTMLElement): HTMLElement[] {
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => !(el as HTMLInputElement).disabled && !el.closest('fieldset[disabled]'),
  );
  const isRadio = (el: HTMLElement): el is HTMLInputElement =>
    el instanceof HTMLInputElement && el.type === 'radio' && el.name !== '';
  return all.filter((el) => {
    if (!isRadio(el)) return true;
    const group = all.filter((o): o is HTMLInputElement => isRadio(o) && o.name === el.name);
    return el === (group.find((r) => r.checked) ?? group[0]);
  });
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

  const abortRef = useRef<AbortController | null>(null);
  const wasBusy = useRef(false);
  const lastFocused = useRef<HTMLElement | null>(null);

  // Focus the first control on open.
  useEffect(() => {
    firstRef.current?.focus();
  }, []);

  // Closing for any reason (including the owner unmounting us) abandons a
  // request still in flight.
  useEffect(() => () => abortRef.current?.abort(), []);

  // Cancel / Escape / overlay click: abort a running export, then close.
  function close() {
    abortRef.current?.abort();
    setBusy(false);
    onClose();
  }
  const closeRef = useRef(close);
  closeRef.current = close;

  useEffect(() => {
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === 'Escape') closeRef.current();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  // The Export button is disabled while running, so a keyboard user loses
  // focus to <body>: put it back when the export ends and the dialog stays.
  useEffect(() => {
    if (wasBusy.current && !busy) {
      panelRef.current?.querySelector<HTMLButtonElement>('button[type="submit"]')?.focus();
    }
    wasBusy.current = busy;
  }, [busy]);

  // Keep Tab inside the dialog (it is modal).
  function trapTab(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== 'Tab' || !panelRef.current) return;
    const stops = tabStops(panelRef.current);
    if (stops.length === 0) return;
    const first = stops[0];
    const last = stops[stops.length - 1];
    const at = stops.indexOf(document.activeElement as HTMLElement);
    if (e.shiftKey && at <= 0) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (at === -1 || at === stops.length - 1)) {
      e.preventDefault();
      first.focus();
    }
  }

  // Belt and braces for focus that gets out by other means (a script, browser
  // chrome): when focus lands outside the dialog, send it back to where it was
  // inside. Listens for `focusin` (after the move) rather than blur, since
  // refocusing from inside a blur handler loses to the move in progress.
  useEffect(() => {
    function onFocusIn(e: globalThis.FocusEvent) {
      const panel = panelRef.current;
      if (!panel || !(e.target instanceof Node) || panel.contains(e.target)) return;
      const back = lastFocused.current;
      (back && panel.contains(back) && !(back as HTMLInputElement).disabled ? back : tabStops(panel)[0])?.focus();
    }
    document.addEventListener('focusin', onFocusIn);
    return () => document.removeEventListener('focusin', onFocusIn);
  }, []);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (busy || !canExport) return;
    const body: FindingsExportBody = { format, ids, include_summaries: includeSummaries, split };
    const t = title.trim();
    if (t) body.title = t;
    if (wsId) body.ws_id = wsId;
    const controller = new AbortController();
    abortRef.current = controller;
    setBusy(true);
    setError(null);
    try {
      const blob = await api.exportFindings(body, { signal: controller.signal });
      if (controller.signal.aborted) return; // cancelled while the bytes arrived
      // The server names the download (carried on a File); else a sensible default.
      saveBlob(blob, `findings-report.${split ? 'zip' : format}`);
      setBusy(false);
      onClose();
    } catch (err: unknown) {
      if (controller.signal.aborted) return; // cancelled: nothing to report
      setError(apiErrorMessage(err));
      setBusy(false);
    }
  }

  return (
    <div
      data-testid="export-overlay"
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/30 p-4 pt-[10vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={trapTab}
        onFocus={(e) => { lastFocused.current = e.target as HTMLElement; }}
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
            <Button variant="secondary" onClick={close}>
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
