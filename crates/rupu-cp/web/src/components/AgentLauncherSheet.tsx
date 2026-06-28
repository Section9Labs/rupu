// AgentLauncherSheet — modal sheet to dispatch an agent run from the browser.
//
// Prompt textarea + Mode picker (Ask / Bypass / Read-only) + TargetPicker.
// On Launch it POSTs to /api/agents/:name/run (single run) or
// /api/agents/:name/session (session) and navigates to the result.

import { useEffect, useId, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type LaunchMode } from '../lib/api';
import TargetPicker from './TargetPicker';
import HostSelect from './HostSelect';
import { Button } from './ui/Button';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';

export interface AgentLaunch {
  prompt?: string;
  mode: LaunchMode;
  target?: string;
  working_dir?: string;
}

export function buildAgentLaunch(prompt: string, mode: LaunchMode, target: TargetItem): AgentLaunch {
  const out: AgentLaunch = { mode };
  const p = prompt.trim();
  if (p) out.prompt = p;
  if (target.resolved.target) out.target = target.resolved.target;
  if (target.resolved.working_dir) out.working_dir = target.resolved.working_dir;
  return out;
}

type LaunchKind = 'run' | 'session';

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

  const [launchKind, setLaunchKind] = useState<LaunchKind>('run');
  const [prompt, setPrompt] = useState('');
  const [mode, setMode] = useState<LaunchMode>('ask');
  const [target, setTarget] = useState<TargetItem>(WORKSPACE_ITEM);
  const [host, setHost] = useState('local');
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

  async function onLaunch() {
    if (launching) return;
    setLaunching(true);
    setError(null);
    try {
      const opts = {
        ...buildAgentLaunch(prompt, mode, target),
        host: host !== 'local' ? host : undefined,
      };
      if (launchKind === 'session') {
        const res = await api.startSession(agent, opts);
        navigate(`/sessions/${res.session_id}`);
      } else {
        const res = await api.launchAgent(agent, opts);
        navigate(`/runs/${res.run_id}`);
      }
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : (launchKind === 'session' ? 'Failed to start session' : 'Failed to launch run'));
      setLaunching(false);
    }
  }

  const fieldCls =
    'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60';

  const submitLabel = launchKind === 'session'
    ? (launching ? 'Starting…' : 'Start session')
    : (launching ? 'Launching…' : 'Run');

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
          {/* ── Launch kind toggle ────────────────────────────────── */}
          <div>
            <div className="inline-flex rounded-md border border-border overflow-hidden">
              <button
                type="button"
                onClick={() => setLaunchKind('run')}
                disabled={launching}
                aria-pressed={launchKind === 'run'}
                className={
                  'rounded-none px-2 py-1 text-ui font-medium disabled:cursor-not-allowed disabled:opacity-60 ' +
                  (launchKind === 'run'
                    ? 'bg-brand-600 text-white'
                    : 'bg-panel text-ink-dim hover:bg-surface-hover')
                }
              >
                Single run
              </button>
              <button
                type="button"
                onClick={() => {
                  if (mode === 'ask') setMode('bypass');
                  setLaunchKind('session');
                }}
                disabled={launching}
                aria-pressed={launchKind === 'session'}
                className={
                  'rounded-none border-l border-border px-2 py-1 text-ui font-medium disabled:cursor-not-allowed disabled:opacity-60 ' +
                  (launchKind === 'session'
                    ? 'bg-brand-600 text-white'
                    : 'bg-panel text-ink-dim hover:bg-surface-hover')
                }
              >
                Session
              </button>
            </div>
            {launchKind === 'session' && (
              <p className="mt-1 text-ui text-ink-mute">
                Opens a multi-turn chat you can keep messaging.
              </p>
            )}
          </div>

          {/* ── Prompt ─────────────────────────────────────────────── */}
          <label className="block">
            <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">
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
            <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">
              Mode
            </span>
            <select
              value={mode}
              onChange={(e) => setMode(e.target.value as LaunchMode)}
              disabled={launching}
              aria-label="Permission mode"
              className={fieldCls}
            >
              {launchKind !== 'session' && <option value="ask">Ask</option>}
              <option value="bypass">Bypass</option>
              <option value="readonly">Read-only</option>
            </select>
          </label>

          {/* ── Target ─────────────────────────────────────────────── */}
          <div>
            <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">Target</span>
            <TargetPicker value={target} onChange={setTarget} disabled={launching} />
          </div>

          {/* ── Host ───────────────────────────────────────────────── */}
          <label className="block">
            <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">
              Host
            </span>
            <HostSelect
              value={host}
              onChange={setHost}
              disabled={launching}
              className="w-full"
            />
          </label>

          {error && (
            <p role="alert" className="text-ui font-medium text-err">
              {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
          <Button variant="secondary" onClick={onClose} disabled={launching}>
            Cancel
          </Button>
          <Button onClick={onLaunch} disabled={launching}>
            {submitLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
