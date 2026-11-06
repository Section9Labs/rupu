// RetryControl — the run page's Retry ▾ (mock §1). A retry resumes the run, so
// its mode defaults to the RECORDED one ("(as launched)"): a read-only run
// retries read-only unless the operator picks otherwise, and only a changed
// mode is sent. "Restart interrupted steps" starts steps and `for_each` units
// that were cut off mid-turn over instead of continuing them. The popover
// closes as the request goes out; its outcome (the pending chip, or the
// server's message) is the page's to show.

import { useId, useState } from 'react';
import { ChevronDown, RotateCcw } from 'lucide-react';
import { Button } from '../ui/Button';
import { Select } from '../ui/Select';
import { ActionPopover } from './ActionPopover';
import { PERMISSION_MODES, modeLabel, type PermissionMode } from '../../lib/api3b/runs';

export interface RetryChoice {
  /** Absent keeps the run's recorded mode. */
  mode?: PermissionMode;
  restartInterrupted: boolean;
}

export function RetryControl({
  workflowName,
  recordedMode,
  disabled,
  disabledReason,
  onRetry,
}: {
  workflowName: string;
  /** The run's `permission_mode`, or null when it predates the field. */
  recordedMode: PermissionMode | null;
  disabled?: boolean;
  disabledReason?: string;
  onRetry: (choice: RetryChoice) => void;
}) {
  return (
    <ActionPopover
      title={`Retry ${workflowName}`}
      variant="solid"
      disabled={disabled}
      disabledReason={disabledReason}
      label={
        <>
          <RotateCcw size={14} aria-hidden /> Retry <ChevronDown size={13} aria-hidden className="-mr-0.5 opacity-70" />
        </>
      }
    >
      {(close) => (
        <RetryForm
          recordedMode={recordedMode}
          onCancel={close}
          onSubmit={(choice) => {
            close();
            onRetry(choice);
          }}
        />
      )}
    </ActionPopover>
  );
}

function RetryForm({
  recordedMode,
  onCancel,
  onSubmit,
}: {
  recordedMode: PermissionMode | null;
  onCancel: () => void;
  onSubmit: (choice: RetryChoice) => void;
}) {
  // '' = as launched, for a run whose record names no mode.
  const [mode, setMode] = useState<PermissionMode | ''>(recordedMode ?? '');
  const [restart, setRestart] = useState(false);
  const modeId = useId();
  const restartId = useId();
  const helpId = useId();

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit({
          mode: mode === '' || mode === recordedMode ? undefined : mode,
          restartInterrupted: restart,
        });
      }}
    >
      <label htmlFor={modeId} className="block text-note font-semibold text-ink">
        Mode
      </label>
      <Select
        id={modeId}
        value={mode}
        onChange={(e) => setMode(e.target.value as PermissionMode | '')}
        className="mt-1 w-full text-ui"
      >
        {recordedMode === null && <option value="">As launched</option>}
        {PERMISSION_MODES.map((m) => (
          <option key={m} value={m}>
            {modeLabel(m)}
            {m === recordedMode ? ' (as launched)' : ''}
          </option>
        ))}
      </Select>

      <div className="mt-3">
        <label htmlFor={restartId} className="inline-flex items-center gap-2 text-ui text-ink">
          <input
            id={restartId}
            type="checkbox"
            checked={restart}
            onChange={(e) => setRestart(e.target.checked)}
            aria-describedby={helpId}
            className="h-3.5 w-3.5 rounded border-border-strong accent-ink"
          />
          Restart interrupted steps from scratch
        </label>
        <p id={helpId} className="mt-0.5 pl-[22px] text-note text-ink-mute">
          Off: steps cut off mid-turn continue where they stopped.
        </p>
      </div>

      <div className="mt-3 flex justify-end gap-2">
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="solid" type="submit">
          Retry
        </Button>
      </div>
    </form>
  );
}

export default RetryControl;
