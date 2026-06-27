// LauncherSheet — a modal sheet to dispatch a workflow run from the browser.
//
// One text field per declared input (when known), else a small add-row
// key/value editor; a Mode picker (Ask / Bypass / Read-only); an optional
// Target field. On Launch it POSTs to the launcher endpoint and navigates to
// the new run's detail page.

import { useEffect, useId, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type LaunchMode, type RepoEntry } from '../lib/api';
import Combobox, { type ComboboxOption } from './Combobox';

export function repoToOption(r: RepoEntry): ComboboxOption {
  return { value: `${r.platform}:${r.repo}`, label: r.repo };
}

interface KvRow {
  key: string;
  value: string;
}

/** Collect declared-input fields into a `Record<string,string>`, dropping
 *  empty values so the backend sees only what the operator actually set. */
function collectDeclared(values: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(values)) {
    if (v.trim() !== '') out[k] = v;
  }
  return out;
}

/** Collect free-form rows, dropping rows with an empty key or value. */
function collectRows(rows: KvRow[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const r of rows) {
    const k = r.key.trim();
    if (k !== '' && r.value.trim() !== '') out[k] = r.value;
  }
  return out;
}

export default function LauncherSheet({
  workflow,
  declaredInputs,
  onClose,
}: {
  workflow: string;
  declaredInputs?: string[];
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  const hasDeclared = (declaredInputs?.length ?? 0) > 0;

  // Declared-input mode: one controlled value per declared name.
  const [declaredValues, setDeclaredValues] = useState<Record<string, string>>(() =>
    Object.fromEntries((declaredInputs ?? []).map((n) => [n, ''])),
  );
  // Free-form mode: a few editable key/value rows.
  const [rows, setRows] = useState<KvRow[]>([{ key: '', value: '' }]);

  const [mode, setMode] = useState<LaunchMode>('ask');
  const [target, setTarget] = useState('');
  const [repoOptions, setRepoOptions] = useState<ComboboxOption[]>([]);
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Esc-to-close; focus the dialog on open for keyboard users.
  useEffect(() => {
    dialogRef.current?.focus();
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Fetch available repos once on open so Target gets typeahead suggestions.
  useEffect(() => {
    let cancelled = false;
    api.getRepos().then((repos: RepoEntry[]) => {
      if (cancelled) return;
      setRepoOptions(repos.map(repoToOption));
    }).catch(() => {
      // Non-critical — leave repoOptions empty and fall back to free text.
    });
    return () => { cancelled = true; };
  }, []);

  async function onLaunch() {
    if (launching) return;
    const inputs = hasDeclared ? collectDeclared(declaredValues) : collectRows(rows);
    setLaunching(true);
    setError(null);
    try {
      const res = await api.launchRun(workflow, {
        inputs: Object.keys(inputs).length > 0 ? inputs : undefined,
        mode,
        target: target.trim() || undefined,
      });
      navigate(`/runs/${res.run_id}`);
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to launch run');
      setLaunching(false);
    }
  }

  function updateRow(i: number, patch: Partial<KvRow>) {
    setRows((prev) => prev.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  }

  const fieldCls =
    'w-full rounded-md border border-border bg-white px-2.5 py-1.5 text-[13px] text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60';

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/40 p-4 pt-[10vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="w-full max-w-md rounded-xl border border-border bg-panel shadow-card focus:outline-none"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="border-b border-border px-5 py-4">
          <h2 id={titleId} className="text-base font-semibold text-ink">
            Run <span className="font-mono break-all">{workflow}</span>
          </h2>
        </div>

        <div className="space-y-4 px-5 py-4">
          {/* ── Inputs ─────────────────────────────────────────────── */}
          <fieldset>
            <legend className="mb-1.5 text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Inputs
            </legend>
            {hasDeclared ? (
              <div className="space-y-2.5">
                {(declaredInputs ?? []).map((name) => (
                  <label key={name} className="block">
                    <span className="mb-1 block text-[12px] font-medium text-ink font-mono">{name}</span>
                    <input
                      type="text"
                      value={declaredValues[name] ?? ''}
                      onChange={(e) =>
                        setDeclaredValues((prev) => ({ ...prev, [name]: e.target.value }))
                      }
                      disabled={launching}
                      aria-label={`Input ${name}`}
                      className={fieldCls}
                    />
                  </label>
                ))}
              </div>
            ) : (
              <div className="space-y-2">
                {rows.map((r, i) => (
                  <div key={i} className="flex items-center gap-2">
                    <input
                      type="text"
                      value={r.key}
                      onChange={(e) => updateRow(i, { key: e.target.value })}
                      disabled={launching}
                      placeholder="key"
                      aria-label={`Input name ${i + 1}`}
                      className={fieldCls}
                    />
                    <input
                      type="text"
                      value={r.value}
                      onChange={(e) => updateRow(i, { value: e.target.value })}
                      disabled={launching}
                      placeholder="value"
                      aria-label={`Input value ${i + 1}`}
                      className={fieldCls}
                    />
                  </div>
                ))}
                <button
                  type="button"
                  onClick={() => setRows((prev) => [...prev, { key: '', value: '' }])}
                  disabled={launching}
                  className="text-[12px] font-medium text-brand-600 hover:text-brand-700 disabled:opacity-60"
                >
                  + Add input
                </button>
              </div>
            )}
          </fieldset>

          {/* ── Mode ───────────────────────────────────────────────── */}
          <label className="block">
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Mode
            </span>
            <select
              value={mode}
              onChange={(e) => setMode(e.target.value as LaunchMode)}
              disabled={launching}
              aria-label="Permission mode"
              className={fieldCls}
            >
              <option value="ask">Ask</option>
              <option value="bypass">Bypass</option>
              <option value="readonly">Read-only</option>
            </select>
          </label>

          {/* ── Target ─────────────────────────────────────────────── */}
          <div className="block">
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Target <span className="font-normal normal-case text-ink-mute">(optional)</span>
            </span>
            <Combobox
              value={target}
              onChange={setTarget}
              options={repoOptions}
              disabled={launching}
              placeholder="e.g. github:owner/repo"
              aria-label="Target"
              className={fieldCls}
            />
            <span className="mt-1 block text-[11px] text-ink-mute">
              leave blank to run in this workspace
            </span>
          </div>

          {error && (
            <p role="alert" className="text-[12px] font-medium text-red-700">
              {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
          <button
            type="button"
            onClick={onClose}
            disabled={launching}
            className="inline-flex items-center rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onLaunch}
            disabled={launching}
            className="inline-flex items-center rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700 disabled:cursor-not-allowed disabled:opacity-60"
          >
            {launching ? 'Launching…' : 'Launch'}
          </button>
        </div>
      </div>
    </div>
  );
}
