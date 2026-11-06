// AutoflowRunNow — the inline issue-ref field under Library → Autoflows that
// a row's "Run now" opens (Plan 3b mock §9). It sends what was typed to
// `POST /api/autoflows/:name/run`; the server is the one judge of the ref, so
// a bare `42` comes back as its 400 and shows inline. A 202 means the cycle
// was started in the background, not that it did anything: the state says
// "started", and Explain is one click away to follow it.

import { useId, useState, type FormEvent } from 'react';
import { X } from 'lucide-react';
import { apiErrorMessage } from '../../lib/api';
import { admin, type AutoflowRunReply } from '../../lib/api3b/admin';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { StatusChip } from '../ui/StatusChip';

export function AutoflowRunNow({
  name,
  onExplain,
  onClose,
}: {
  name: string;
  /** Open the Explain drawer for the issue just started. */
  onExplain: (issueRef: string) => void;
  onClose: () => void;
}) {
  const inputId = useId();
  const [issue, setIssue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState<AutoflowRunReply | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy || !issue.trim()) return;
    setBusy(true);
    setError(null);
    setStarted(null);
    try {
      setStarted(await admin.runAutoflow(name, { issue: issue.trim() }));
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-3 rounded-xl border border-border bg-panel px-4 py-3 shadow-card">
      <form onSubmit={(e) => void submit(e)}>
        <div className="mb-1.5 flex items-center gap-2">
          <label htmlFor={inputId} className="text-ui font-semibold text-ink">
            Run {name} for
          </label>
          <span className="flex-1" />
          <button
            type="button"
            onClick={onClose}
            aria-label="Close run now"
            className="rounded p-1 text-ink-mute hover:bg-surface-hover hover:text-ink"
          >
            <X size={12} />
          </button>
        </div>
        <div className="flex gap-2">
          <Input
            id={inputId}
            value={issue}
            onChange={(e) => setIssue(e.target.value)}
            placeholder="github:owner/repo/issues/42"
            className="max-w-none flex-1 font-mono"
            autoComplete="off"
            spellCheck={false}
          />
          <Button type="submit" variant="solid" disabled={busy || !issue.trim()}>
            {busy ? 'Starting…' : 'Run'}
          </Button>
        </div>
        <p className="mt-1 text-note text-ink-mute">
          A full issue ref: <span className="font-mono">platform:owner/repo/issues/N</span>.
        </p>
      </form>
      {error && (
        <p role="alert" className="mt-2 text-note text-err">
          {error}
        </p>
      )}
      {started && (
        <div className="mt-2 flex flex-wrap items-center gap-2 text-note text-ink-dim">
          <StatusChip tone="running" dot>
            started · pid {started.pid}
          </StatusChip>
          <span>The cycle runs in the background; its run appears under Activity → Autoflows once it is recorded.</span>
          <Button variant="ring" onClick={() => onExplain(started.issue)}>
            Explain
          </Button>
        </div>
      )}
    </div>
  );
}

export default AutoflowRunNow;
