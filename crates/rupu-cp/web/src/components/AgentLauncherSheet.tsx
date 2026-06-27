// AgentLauncherSheet — modal sheet to dispatch an agent run from the browser.
//
// Prompt textarea + Mode picker (Ask / Bypass / Read-only) + target-mode
// selector (Workspace / Directory / Repository). On Launch it POSTs to
// /api/agents/:name/run and navigates to the new run's detail page.

import { useEffect, useId, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type LaunchMode, type RepoEntry } from '../lib/api';
import Combobox, { type ComboboxOption } from './Combobox';
import DirectoryPicker from './DirectoryPicker';

export type AgentTargetMode = 'workspace' | 'directory' | 'repo';

export interface AgentLaunch {
  prompt?: string;
  mode: LaunchMode;
  target?: string;
  working_dir?: string;
}

export function buildAgentLaunch(
  prompt: string,
  mode: LaunchMode,
  targetMode: AgentTargetMode,
  target: string,
  workingDir: string,
): AgentLaunch {
  const out: AgentLaunch = { mode };
  const p = prompt.trim();
  if (p) out.prompt = p;
  if (targetMode === 'repo') { const t = target.trim(); if (t) out.target = t; }
  if (targetMode === 'directory') { const d = workingDir.trim(); if (d) out.working_dir = d; }
  return out;
}

function repoToOption(r: RepoEntry): ComboboxOption {
  return { value: `${r.platform}:${r.repo}`, label: r.repo };
}

export default function AgentLauncherSheet({
  agent,
  onClose,
}: {
  agent: string;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  const [prompt, setPrompt] = useState('');
  const [mode, setMode] = useState<LaunchMode>('ask');
  const [targetMode, setTargetMode] = useState<AgentTargetMode>('workspace');
  const [target, setTarget] = useState('');
  const [workingDir, setWorkingDir] = useState('');
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
    setLaunching(true);
    setError(null);
    try {
      const opts = buildAgentLaunch(prompt, mode, targetMode, target, workingDir);
      const res = await api.launchAgent(agent, opts);
      navigate(`/runs/${res.run_id}`);
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to launch run');
      setLaunching(false);
    }
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
            Run <span className="font-mono break-all">{agent}</span>
          </h2>
        </div>

        <div className="space-y-4 px-5 py-4">
          {/* ── Prompt ─────────────────────────────────────────────── */}
          <label className="block">
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Prompt
            </span>
            <textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              disabled={launching}
              rows={4}
              placeholder="Describe what the agent should do…"
              aria-label="Prompt"
              className={fieldCls + ' resize-y'}
            />
          </label>

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
          <div>
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Target
            </span>
            <div className="mb-2 flex gap-1">
              {(['workspace', 'directory', 'repo'] as AgentTargetMode[]).map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => setTargetMode(m)}
                  disabled={launching}
                  className={
                    'rounded-md px-2 py-1 text-[12px] font-medium ' +
                    (targetMode === m
                      ? 'bg-brand-600 text-white'
                      : 'border border-border bg-white text-ink-dim hover:bg-slate-50')
                  }
                >
                  {m === 'workspace' ? 'This workspace' : m === 'directory' ? 'Directory' : 'Repository'}
                </button>
              ))}
            </div>
            {targetMode === 'workspace' && (
              <p className="text-[11px] text-ink-mute">Runs in the control-plane working directory.</p>
            )}
            {targetMode === 'directory' && (
              <DirectoryPicker value={workingDir} onChange={setWorkingDir} />
            )}
            {targetMode === 'repo' && (
              <Combobox
                value={target}
                onChange={setTarget}
                options={repoOptions}
                disabled={launching}
                aria-label="Target"
                placeholder="e.g. github:owner/repo"
                className={fieldCls}
              />
            )}
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
