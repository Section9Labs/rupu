// ImportFindingsDialog — "Import" next to the findings Export (Plan 3b mock
// §7): attach Markdown reports written before the full profile to their
// findings. It always runs a dry run first (`POST /api/findings/import
// {paths, dry_run: true}`) and shows each file's outcome; only then does the
// import button appear, labelled with how many reports it attaches. After
// the import the owner reloads its list, so the attached reports show when
// the server serves them. "Report schema ↗" swaps in a read-only view of
// the report format (`GET /api/findings/schema`). Rendered only while open.

import { useEffect, useState } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { getFindingSchema, importFindings, type FindingsImportReport, type ImportedFile } from '../../lib/api3b/library';
import { DialogFrame } from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { inputCls } from '../ui/Input';
import { Spinner } from '../ui/Spinner';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { cn } from '../../lib/cn';

const OUTCOME: Record<ImportedFile['status'], { label: string; tone: ChipTone }> = {
  attached: { label: 'attach', tone: 'completed' },
  skipped: { label: 'skip', tone: 'neutral' },
  failed: { label: 'fail', tone: 'failed' },
};

const reports = (n: number) => `${n} report${n === 1 ? '' : 's'}`;
const baseName = (p: string) => p.split('/').filter(Boolean).pop() ?? p;
const summary = (r: FindingsImportReport, attach: string) => `${r.attached} ${attach} · ${r.skipped} skipped · ${r.failed} failed`;

export interface ImportFindingsDialogProps {
  onClose: () => void;
  /** After a real import attached at least one report. */
  onImported: () => void;
}

export function ImportFindingsDialog({ onClose, onImported }: ImportFindingsDialogProps) {
  const [text, setText] = useState('');
  const [dry, setDry] = useState<FindingsImportReport | null>(null);
  const [done, setDone] = useState<FindingsImportReport | null>(null);
  const [busy, setBusy] = useState<'dry' | 'import' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<'import' | 'schema'>('import');

  const paths = text
    .split('\n')
    .map((p) => p.trim())
    .filter(Boolean);

  async function run(dryRun: boolean) {
    if (paths.length === 0 || busy) return;
    setBusy(dryRun ? 'dry' : 'import');
    setError(null);
    try {
      const r = await importFindings(paths, dryRun);
      if (dryRun) setDry(r);
      else {
        setDone(r);
        if (r.attached > 0) onImported();
      }
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  if (view === 'schema') {
    return (
      <DialogFrame
        title="Finding report schema"
        onRequestClose={onClose}
        onEscape={() => setView('import')}
        className="max-w-3xl"
      >
        <SchemaView onBack={() => setView('import')} />
      </DialogFrame>
    );
  }

  const shown = done ?? dry;

  return (
    <DialogFrame title="Import finding reports" onRequestClose={onClose} className="relative max-w-2xl">
      <label className="mt-4 block">
        <span className="text-ui font-medium text-ink">Files or directories on this machine</span>
        {/* One path per line, each on one line: it reads as an input for one path. */}
        <textarea
          rows={Math.min(Math.max(text.split('\n').length, 1), 6)}
          wrap="off"
          value={text}
          disabled={busy !== null || done !== null}
          onChange={(e) => {
            setText(e.target.value);
            setDry(null);
            setError(null);
          }}
          spellCheck={false}
          placeholder="/path/to/reports"
          className={cn(inputCls, 'mt-1 block max-w-none resize-none overflow-x-auto whitespace-pre font-mono')}
        />
      </label>
      <p className="mt-1 text-note text-ink-mute">
        Absolute paths, one per line. A directory is searched for Markdown reports. The ledger is backed up before
        anything is rewritten.
      </p>
      {/* In the title row; after the paths in the DOM, so they get the first focus. */}
      <div className="absolute right-5 top-4">
        <Button variant="ghost" size="sm" onClick={() => setView('schema')}>
          Report schema ↗
        </Button>
      </div>

      {shown && (
        <>
          <div
            role="status"
            className={cn(
              'mt-4 rounded-lg px-3 py-2 text-note ring-1',
              done ? 'bg-ok-bg text-ok ring-ok/30' : 'bg-info-bg text-info ring-info/30',
            )}
          >
            {done ? (
              <>
                <p>Imported: {summary(done, 'attached')}.</p>
                {done.backups.map((b) => (
                  <p key={b} className="break-all font-mono text-ink-dim">
                    backup {b}
                  </p>
                ))}
              </>
            ) : (
              <p>
                Dry run: <span className="font-semibold">{summary(shown, 'attach')}</span>. Nothing has been written yet.
              </p>
            )}
            {shown.warnings?.map((w) => (
              <p key={w} className="text-warn">
                {w}
              </p>
            ))}
          </div>
          <ul aria-label="Files" className="mt-3 max-h-72 divide-y divide-border overflow-y-auto rounded-lg border border-border">
            {shown.files.map((f) => {
              const o = OUTCOME[f.status];
              return (
                <li key={f.path} className="flex items-start gap-3 px-3 py-2" title={f.path}>
                  <StatusChip tone={o.tone} className="mt-0.5 w-12 justify-center">
                    {o.label}
                  </StatusChip>
                  <span className="min-w-0 flex-1 truncate font-mono text-ui text-ink">{baseName(f.path)}</span>
                  <span className="min-w-0 max-w-[55%] text-right text-note text-ink-dim">
                    {f.status === 'attached' ? (
                      <span className="font-mono">→ {f.finding_id}</span>
                    ) : (
                      <>
                        {f.reason}
                        {f.status === 'failed' &&
                          f.details?.map((d) => (
                            <span key={d} className="block text-ink-mute">
                              {d}
                            </span>
                          ))}
                      </>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </>
      )}

      {error && (
        <p role="alert" className="mt-3 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note text-err">
          {error}
        </p>
      )}

      <div className="mt-5 flex justify-end gap-2">
        <Button variant="secondary" onClick={onClose}>
          {done ? 'Close' : 'Cancel'}
        </Button>
        {!done &&
          (dry ? (
            <Button variant="solid" disabled={dry.attached === 0 || busy !== null} onClick={() => run(false)}>
              {busy === 'import' ? 'Importing…' : `Import ${reports(dry.attached)}`}
            </Button>
          ) : (
            <Button variant="solid" disabled={paths.length === 0 || busy !== null} onClick={() => run(true)}>
              {busy === 'dry' ? 'Checking…' : 'Dry run'}
            </Button>
          ))}
      </div>
    </DialogFrame>
  );
}

/** The report JSON Schema, pretty-printed, read-only. */
function SchemaView({ onBack }: { onBack: () => void }) {
  const [schema, setSchema] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getFindingSchema().then(
      (s) => !cancelled && setSchema(JSON.stringify(s, null, 2)),
      (e: unknown) => !cancelled && setError(apiErrorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <>
      <p className="mt-1 text-note text-ink-mute">
        The draft-07 JSON Schema of a full-profile finding report, as this build embeds it.
      </p>
      <div className="mt-3">
        {error ? (
          <p role="alert" className="text-note text-err">
            {error}
          </p>
        ) : schema === null ? (
          <Spinner size="sm" label="Loading the schema…" />
        ) : (
          <pre
            aria-label="Finding report schema"
            tabIndex={0}
            className="max-h-[60vh] overflow-auto rounded-lg border border-border bg-surface p-3 font-mono text-note leading-relaxed text-ink"
          >
            {schema}
          </pre>
        )}
      </div>
      <div className="mt-4 flex justify-end">
        <Button variant="secondary" onClick={onBack}>
          Back to import
        </Button>
      </div>
    </>
  );
}

export default ImportFindingsDialog;
