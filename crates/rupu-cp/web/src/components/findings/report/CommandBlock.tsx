import { useEffect, useRef, useState } from 'react';
import { copyText } from '../../../lib/findingReport';

type CopyState = 'idle' | 'copied' | 'failed';

const LABEL: Record<CopyState, string> = { idle: 'Copy', copied: 'Copied', failed: 'Copy failed' };

export default function CommandBlock({ label, command }: { label: string; command: string }) {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  function clearTimer() {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }

  // Drop a pending reset if the block unmounts mid-flash.
  useEffect(() => clearTimer, []);

  async function copy() {
    const ok = await copyText(command);
    clearTimer();
    setState(ok ? 'copied' : 'failed');
    timer.current = setTimeout(() => {
      timer.current = null;
      setState('idle');
    }, 1500);
  }

  return (
    <div className="overflow-hidden rounded-md border border-border bg-panel">
      <div className="flex items-center justify-between border-b border-border px-3 py-1">
        <span className="text-note text-ink-mute">{label}</span>
        <button
          type="button"
          onClick={() => void copy()}
          className="rounded border border-border px-2 py-0.5 text-note text-ink-dim hover:bg-surface"
        >
          <span aria-live="polite">{LABEL[state]}</span>
        </button>
      </div>
      <pre className="overflow-x-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">{command}</pre>
    </div>
  );
}
