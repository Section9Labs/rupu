// The operator's composer (spec F1c §3.2): a recipient, a body, an urgent
// toggle. A send is never shown as a message here or in the feed until its
// line reaches the log: until then it is a "pending" entry the feed clears
// when the stream delivers that id.

import { useState } from 'react';
import { Loader2, Send, Siren } from 'lucide-react';
import type { PendingSend, RecipientGroup } from '../../lib/messages';
import { cn } from '../../lib/cn';
import { Button } from '../ui/Button';
import { AddressPill } from './Avatar';
import { RecipientPicker } from './RecipientPicker';

export interface ComposerProps {
  groups: RecipientGroup[];
  defaultTo: string;
  /** Why sending is off (the run finished, the host can't take messages). */
  disabledReason?: string | null;
  pending: PendingSend[];
  /** The send failed: shown under the box. */
  error?: string | null;
  onSend: (msg: { to: string; body: string; urgent: boolean }) => Promise<boolean>;
  pickerOpen?: boolean;
}

export const URGENT_TIP = "delivered first, and ends an agentiflow lead's round early";

export function Composer({ groups, defaultTo, disabledReason, pending, error, onSend, pickerOpen }: ComposerProps) {
  const [to, setTo] = useState(defaultTo);
  const [draft, setDraft] = useState('');
  const [urgent, setUrgent] = useState(false);
  const [busy, setBusy] = useState(false);
  const disabled = !!disabledReason;

  async function send() {
    const body = draft.trim();
    if (!body || disabled || busy) return;
    setBusy(true);
    try {
      if (await onSend({ to, body, urgent })) {
        setDraft('');
        setUrgent(false);
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div data-testid="composer" className="rounded-xl border border-border bg-panel p-2 shadow-card">
      {pending.length > 0 && (
        <ul className="mb-2 flex flex-col gap-1">
          {pending.map((p) => (
            <li key={p.key} data-testid="pending" className="flex items-center gap-1.5 rounded-md bg-surface/70 px-2 py-1 text-meta text-ink-mute">
              <Loader2 size={11} className="animate-spin" />
              <span className="font-medium">pending</span>
              <span>→</span>
              <AddressPill address={p.to} />
              {p.urgent && <span className="font-semibold uppercase text-err">urgent</span>}
              <span className="min-w-0 flex-1 truncate italic">“{p.body}”</span>
              <span className="shrink-0">{p.id ? 'waiting for the log' : 'sending'}</span>
            </li>
          ))}
        </ul>
      )}
      <div className="flex items-end gap-2">
        <RecipientPicker groups={groups} value={to} onChange={setTo} disabled={disabled} defaultOpen={pickerOpen} />
        <textarea
          rows={1}
          aria-label="Message"
          className="min-h-8 min-w-0 flex-1 resize-y rounded-lg border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute disabled:opacity-50"
          placeholder={disabled ? '' : 'Message as the operator… (Enter to send, Shift+Enter for a new line)'}
          value={draft}
          disabled={disabled}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button
          type="button"
          aria-pressed={urgent}
          disabled={disabled}
          onClick={() => setUrgent((v) => !v)}
          title={`Urgent: ${URGENT_TIP}`}
          className={cn(
            'inline-flex h-8 items-center gap-1 rounded-lg border px-2 text-note font-medium disabled:opacity-50',
            urgent ? 'border-err/50 bg-err-bg text-err' : 'border-border text-ink-mute hover:bg-surface-hover',
          )}
        >
          <Siren size={12} />
          Urgent
        </button>
        <Button onClick={() => void send()} disabled={disabled || busy || !draft.trim()} className="h-8 gap-1.5">
          <Send size={12} />
          Send
        </Button>
      </div>
      {disabledReason && <div data-testid="composer-disabled" className="mt-1.5 text-meta text-ink-mute">{disabledReason}</div>}
      {error && <div className="mt-1.5 text-meta text-err">{error}</div>}
    </div>
  );
}
