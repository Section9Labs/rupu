import { useState } from 'react';
import { copyText } from '../../../lib/findingReport';

export default function CommandBlock({ label, command }: { label: string; command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="overflow-hidden rounded-md border border-border bg-panel">
      <div className="flex items-center justify-between border-b border-border px-3 py-1">
        <span className="text-note text-ink-mute">{label}</span>
        <button
          type="button"
          onClick={() => void copyText(command).then((ok) => { setCopied(ok); if (ok) setTimeout(() => setCopied(false), 1500); })}
          className="rounded border border-border px-2 py-0.5 text-note text-ink-dim hover:bg-surface"
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre className="overflow-x-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">{command}</pre>
    </div>
  );
}
