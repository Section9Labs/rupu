// SessionControls — Compact and Stop in the session header (mock §3).
//
// Compact opens a popover with the context window to size the summary
// against (empty = the session's own) and starts `POST …/compact`; Stop asks
// first, then `POST …/stop`. Neither is optimistic: after the server answers,
// `onChanged` re-reads the session, and the page draws `compacting` / the
// stopped state from what the record says. Both are local-only (a remote
// session's host can't run them) and disabled, with the reason, while a
// compaction runs, as the server answers 409 then.

import { useId, useState } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { runsApi } from '../../lib/api3b/runs';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { DialogFrame } from '../customers/DialogFrame';
import { ActionPopover } from './ActionPopover';

const REMOTE_REASON = 'Compacting and stopping a session are local-only.';
const COMPACTING_REASON = 'The session is compacting.';

export function SessionControls({
  sessionId,
  compacting,
  stopped,
  turnRunning,
  remote,
  onChanged,
}: {
  sessionId: string;
  /** `GET /api/sessions/:id` says a compaction is running. */
  compacting: boolean;
  stopped: boolean;
  /** A turn is in flight (the server refuses a compaction then). */
  turnRunning: boolean;
  remote: boolean;
  /** Re-read the session after a write the server accepted. */
  onChanged: () => void;
}) {
  const [pending, setPending] = useState<'compact' | 'stop' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmStop, setConfirmStop] = useState(false);

  async function run(verb: 'compact' | 'stop', call: () => Promise<unknown>) {
    setPending(verb);
    setError(null);
    try {
      await call();
      onChanged();
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setPending(null);
    }
  }

  const compactReason = remote
    ? REMOTE_REASON
    : compacting
      ? COMPACTING_REASON
      : turnRunning
        ? 'A turn is running: compact once it ends.'
        : undefined;
  const stopReason = remote ? REMOTE_REASON : compacting ? COMPACTING_REASON : stopped ? 'The session is stopped.' : undefined;

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-2">
        <ActionPopover
          title="Compact this session"
          label={pending === 'compact' ? 'Compacting…' : 'Compact'}
          ariaLabel="Compact"
          variant="secondary"
          disabled={compactReason !== undefined || pending !== null}
          disabledReason={compactReason}
        >
          {(close) => (
            <CompactForm
              onCancel={close}
              onSubmit={(window) => {
                close();
                void run('compact', () => runsApi.compactSession(sessionId, window));
              }}
            />
          )}
        </ActionPopover>
        <Button
          variant="danger-outline"
          disabled={stopReason !== undefined || pending !== null}
          title={stopReason}
          aria-label="Stop"
          onClick={() => setConfirmStop(true)}
        >
          {pending === 'stop' ? 'Stopping…' : 'Stop'}
        </Button>
      </div>
      {error && (
        <p role="alert" className="max-w-md text-right text-ui font-medium text-err">
          {error}
        </p>
      )}
      {confirmStop && (
        <DialogFrame title="Stop this session?" onRequestClose={() => setConfirmStop(false)}>
          <p className="mt-1.5 text-ui text-ink-dim">
            Ends the session’s worker and any turn it is running, and marks the session stopped. Sending a message
            later starts it again.
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setConfirmStop(false)}>
              Keep it running
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConfirmStop(false);
                void run('stop', () => runsApi.stopSession(sessionId));
              }}
            >
              Stop session
            </Button>
          </div>
        </DialogFrame>
      )}
    </div>
  );
}

function CompactForm({ onCancel, onSubmit }: { onCancel: () => void; onSubmit: (window?: number) => void }) {
  const [value, setValue] = useState('');
  const id = useId();
  const helpId = useId();
  const trimmed = value.trim();
  const parsed = trimmed === '' ? undefined : Number(trimmed);
  const invalid = parsed !== undefined && (!Number.isInteger(parsed) || parsed <= 0);

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (!invalid) onSubmit(parsed);
      }}
    >
      <p className="mb-2.5 text-note text-ink-dim">
        Summarizes the stored conversation so it fits again, then re-attaches the loaded skills. Uses one model
        call.
      </p>
      <label htmlFor={id} className="block text-note font-semibold text-ink">
        Context window
      </label>
      <Input
        id={id}
        inputMode="numeric"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="the session's own"
        aria-describedby={helpId}
        aria-invalid={invalid || undefined}
        className="mt-1 max-w-none text-ui tabular-nums"
      />
      <p id={helpId} className={invalid ? 'mt-1 text-note text-err' : 'mt-1 text-note text-ink-mute'}>
        {invalid ? 'A whole number of tokens.' : 'Tokens to size the summary against.'}
      </p>
      <div className="mt-3 flex justify-end gap-2">
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="solid" type="submit" disabled={invalid}>
          Compact
        </Button>
      </div>
    </form>
  );
}

export default SessionControls;
