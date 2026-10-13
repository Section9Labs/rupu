// A message in an agent's transcript (spec F1c §3.4): what it received sits
// left as the sender's bubble, what it sent sits right as an outline bubble
// with its delivery. Each links to the Messages tab at that message; the
// messaging tool call a sent bubble came from is behind "raw".

import { useEffect, useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, ArrowUpRight, CheckCheck, Code2, Lock, Megaphone, Pin, Sparkles } from 'lucide-react';
import { hexA, identityOf, undeliveredText } from '../../lib/messages';
import { cn } from '../../lib/cn';
import { useThemeMode } from '../codename/useThemeMode';
import { AddressPill, Avatar } from '../messages/Avatar';
import Markdown from './Markdown';
import { useMessageLinks } from './messageLinks';
import type { MessageView } from './transcriptView';

export interface MessageBubbleProps {
  message: MessageView;
  /** The transcript's own address in the space, when known. */
  self?: string | null;
  /** The `#msg-` anchor points here. */
  highlight?: boolean;
  /** The tool card a sent message folded, shown by "raw". */
  raw?: ReactNode;
}

function KindBadge({ kind, urgent }: { kind: string; urgent?: boolean }) {
  const Icon = kind === 'broadcast' ? Megaphone : kind === 'direct' ? Lock : kind === 'directive' ? Pin : kind === 'operator' ? Sparkles : null;
  return (
    <>
      <span className="inline-flex items-center gap-0.5 rounded bg-surface px-1 py-px text-[10px] font-medium leading-none text-ink-mute">
        {Icon && <Icon size={9} />}
        {kind}
      </span>
      {urgent && <span className="rounded bg-err px-1 py-px text-[10px] font-bold uppercase leading-none tracking-wide text-white">urgent</span>}
    </>
  );
}

function Links({ onFeed, onOther, otherLabel }: { onFeed?: () => void; onOther?: () => void; otherLabel: string }) {
  if (!onFeed && !onOther) return null;
  return (
    <span className="inline-flex items-center gap-2 text-meta text-ink-mute">
      {onFeed && (
        <button type="button" onClick={onFeed} className="inline-flex items-center gap-0.5 hover:text-brand-600">
          <ArrowUpRight size={10} />
          in Messages
        </button>
      )}
      {onOther && (
        <button type="button" onClick={onOther} className="inline-flex items-center gap-0.5 hover:text-brand-600">
          <ArrowUpRight size={10} />
          {otherLabel}
        </button>
      )}
    </span>
  );
}

/** Who a received message was for, from the reader's side. */
function receivedTo(kind: string, to: string | null, self?: string | null): string {
  if (to === null) return 'the channel';
  if (to === 'broadcast') return 'everyone';
  if (to.includes(':')) return to;
  if (self) return to === self ? 'you' : to;
  // A post can name anyone; a direct or operator message reached its `to`.
  return kind === 'post' ? to : 'you';
}

function toLabel(to: string | null): string {
  if (!to || to === 'broadcast') return 'everyone';
  return to;
}

export default function MessageBubble({ message: m, self, highlight, raw: rawCard }: MessageBubbleProps) {
  const mode = useThemeMode();
  const links = useMessageLinks();
  const [raw, setRaw] = useState(false);
  const ring = highlight ? 'ring-2 ring-brand-500/60' : '';
  const anchor = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (highlight) anchor.current?.scrollIntoView?.({ block: 'center' });
  }, [highlight]);
  const operator = m.kind === 'operator';
  const onFeed = links.openInFeed && m.id ? () => links.openInFeed!(m.id) : undefined;

  if (m.direction === 'received') {
    const tint = identityOf(m.from, mode).color;
    const accent = m.urgent ? 'rgb(var(--c-err))' : tint;
    const onSender = links.openParticipant && !operator && m.id ? () => links.openParticipant!(m.from, m.id) : undefined;
    return (
      <div ref={anchor} id={`msg-${m.id}`} data-testid="message-bubble" data-direction="received" className="flex max-w-[85%] items-start gap-2">
        <div className="pt-1">
          <Avatar address={m.from} size={22} />
        </div>
        <div className={cn('min-w-0 rounded-2xl rounded-tl-sm px-3 py-1.5', ring)} style={{ background: hexA(tint, operator ? 0.1 : 0.08), boxShadow: `inset 3px 0 0 ${accent}` }}>
          <div className="flex flex-wrap items-center gap-1.5 text-meta">
            <span className="font-semibold" style={{ color: tint }}>
              {m.from}
            </span>
            <span className="text-ink-mute">→ {receivedTo(m.kind, m.to, self)}</span>
            <KindBadge kind={m.kind} urgent={m.urgent} />
            <span className="text-[10px] uppercase tracking-wide text-ink-mute">received</span>
          </div>
          <div className="text-sm text-ink">
            <Markdown text={m.body} />
          </div>
          <Links onFeed={onFeed} onOther={onSender} otherLabel="sender's transcript" />
        </div>
      </div>
    );
  }

  const undelivered = m.delivery?.undelivered ?? [];
  const resolved = m.delivery?.resolved ?? [];
  const recipient = resolved.length === 1 ? resolved[0] : null;
  const onRecipient = links.openParticipant && recipient && m.id ? () => links.openParticipant!(recipient, m.id) : undefined;
  return (
    <div ref={anchor} id={`msg-${m.id}`} data-testid="message-bubble" data-direction="sent" className="flex flex-col items-end gap-1">
      <div className={cn('max-w-[85%] rounded-2xl rounded-tr-sm border border-brand-500/40 bg-panel px-3 py-1.5', ring)}>
        <div className="flex flex-wrap items-center justify-end gap-1.5 text-meta">
          <span className="text-[10px] uppercase tracking-wide text-ink-mute">sent</span>
          <KindBadge kind={m.kind} urgent={m.urgent} />
          <span className="text-ink-mute">you →</span>
          <AddressPill address={m.to ?? 'broadcast'} label={toLabel(m.to)} />
        </div>
        <div className="text-sm text-ink">
          <Markdown text={m.body} />
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2 text-meta">
          {resolved.length > 0 && (
            <span className="inline-flex items-center gap-0.5 text-ok" title={resolved.join(', ')}>
              <CheckCheck size={11} />
              delivered to {resolved.length > 2 ? `${resolved.length}` : resolved.join(', ')}
            </span>
          )}
          {undelivered.map((u) => (
            <span key={`${u.to}-${u.reason}`} data-testid="undelivered" className="inline-flex items-center gap-1 rounded bg-warn-bg px-1.5 py-px font-medium text-warn">
              <AlertTriangle size={10} />
              undelivered: {undeliveredText(u)}
            </span>
          ))}
          <Links onFeed={onFeed} onOther={onRecipient} otherLabel="recipient's transcript" />
          {rawCard && (
            <button type="button" aria-pressed={raw} onClick={() => setRaw((v) => !v)} className="inline-flex items-center gap-0.5 text-ink-mute hover:text-ink">
              <Code2 size={10} />
              raw
            </button>
          )}
        </div>
      </div>
      {raw && <div className="w-full">{rawCard}</div>}
    </div>
  );
}

/** Context a collector put in front of the model that isn't a message
 *  (roster, children, directives): one collapsed line, the exact text on
 *  expand. */
export function InjectedChip({ source, kind, cadence, content }: { source: string; kind: string; cadence: string; content: string }) {
  const [open, setOpen] = useState(false);
  const lines = content.split('\n').length;
  return (
    <div data-testid="injected-chip">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1.5 rounded-full border border-dashed border-border px-2 py-0.5 text-meta text-ink-mute hover:bg-surface-hover hover:text-ink"
      >
        <Sparkles size={10} />
        context injected · {source}
        <span className="opacity-70">
          ({lines} {lines === 1 ? 'line' : 'lines'}
          {cadence === 'every_turn' ? ' · every turn' : ''}
          {kind !== source ? ` · ${kind}` : ''})
        </span>
      </button>
      {open && <pre className="mt-1 max-h-72 overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-surface/50 p-2 font-mono text-[11px] leading-relaxed text-ink-dim">{content}</pre>}
    </div>
  );
}
