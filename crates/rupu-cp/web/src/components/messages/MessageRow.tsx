// One log line in the feed, treated by its kind (spec F1c §3.1): a channel
// post, an @mention, a DM, a broadcast, an operator message, or a compact
// directive / retraction marker. Every row carries its delivery problems,
// its receipts and a link to the sender's transcript.

import { useState, type ReactNode } from 'react';
import { AlertTriangle, ArrowUpRight, Eye, Lock, Megaphone, Pin, PinOff } from 'lucide-react';
import type { SpaceMessage } from '../../lib/api';
import { absoluteTime, relativeTime } from '../../lib/time';
import { hexA, identityOf, receiptsText, rowKind, undeliveredText, type Mode } from '../../lib/messages';
import { cn } from '../../lib/cn';
import { useThemeMode } from '../codename/useThemeMode';
import { AddressPill, Avatar } from './Avatar';

const POST_KIND_HEX: Record<string, string> = {
  question: '#0ea5e9',
  answer: '#16a34a',
  observation: '#8b5cf6',
  vote: '#f59e0b',
};

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** `body` with every participant address it names (bare or `@`) tinted. */
export function renderBody(body: string, addresses: string[], mode: Mode): ReactNode[] {
  const keys = addresses.filter((a) => a.length >= 2).sort((a, b) => b.length - a.length).map(escapeRe);
  if (keys.length === 0) return [body];
  const re = new RegExp(`(^|[^\\w#@>])(@?)(${keys.join('|')})(?![\\w#-])`, 'g');
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(body))) {
    const start = m.index + m[1].length;
    if (start > last) out.push(body.slice(last, start));
    const { color } = identityOf(m[3], mode);
    out.push(
      <span key={i++} className="rounded px-0.5 font-medium" style={{ background: hexA(color, 0.14), color }}>
        {m[2]}
        {m[3]}
      </span>,
    );
    last = m.index + m[0].length;
    if (re.lastIndex === m.index) re.lastIndex++;
  }
  if (last < body.length) out.push(body.slice(last));
  return out;
}

/** The row's time, with "in transcript" revealed beside it on hover. */
function Time({ ts, onOpen }: { ts: string; onOpen?: () => void }) {
  return (
    <span className="ml-auto inline-flex shrink-0 items-center gap-2 text-meta">
      {onOpen && (
        <button
          type="button"
          onClick={onOpen}
          className="inline-flex items-center gap-0.5 text-ink-mute opacity-0 transition-opacity hover:text-brand-600 focus-visible:opacity-100 group-hover/msg:opacity-100"
        >
          <ArrowUpRight size={11} />
          in transcript
        </button>
      )}
      <span className="tabular-nums text-ink-mute" title={absoluteTime(ts)}>
        {relativeTime(ts)}
      </span>
    </span>
  );
}

function PostKind({ kind }: { kind: string }) {
  const hex = POST_KIND_HEX[kind];
  if (!hex) return <span className="rounded bg-surface px-1.5 py-px text-[10px] font-medium leading-none text-ink-mute">{kind}</span>;
  return (
    <span className="rounded px-1.5 py-px text-[10px] font-medium leading-none" style={{ background: hexA(hex, 0.16), color: hex }}>
      {kind}
    </span>
  );
}

function Sender({ address }: { address: string }) {
  const mode = useThemeMode();
  return (
    <span className="text-note font-semibold" style={{ color: identityOf(address, mode).color }}>
      {address}
    </span>
  );
}

/** Undelivered chips, receipts and the transcript link under a row. */
function RowFooter({ m }: { m: SpaceMessage }) {
  const receipts = receiptsText(m.seen);
  const undelivered = m.delivery.undelivered ?? [];
  if (!receipts && undelivered.length === 0) return null;
  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-meta">
      {undelivered.map((u) => (
        <span key={`${u.to}-${u.reason}`} data-testid="undelivered" className="inline-flex items-center gap-1 rounded bg-warn-bg px-1.5 py-px font-medium text-warn">
          <AlertTriangle size={10} />
          undelivered: {undeliveredText(u)}
        </span>
      ))}
      {receipts && (
        <span data-testid="receipts" className="inline-flex items-center gap-1 text-ink-mute" title={receipts.title}>
          <Eye size={10} />
          {receipts.text}
        </span>
      )}
    </div>
  );
}

function Body({ body, addresses, className }: { body: string; addresses: string[]; className?: string }) {
  const mode = useThemeMode();
  const [expanded, setExpanded] = useState(false);
  const long = body.length > 360;
  return (
    <>
      <p className={cn('mt-0.5 whitespace-pre-wrap break-words text-sm leading-relaxed text-ink-dim', long && !expanded && 'line-clamp-4', className)}>{renderBody(body, addresses, mode)}</p>
      {long && (
        <button type="button" onClick={() => setExpanded((v) => !v)} className="mt-0.5 text-meta font-medium text-brand-600 hover:text-brand-700">
          {expanded ? 'show less' : 'show more'}
        </button>
      )}
    </>
  );
}

export interface MessageRowProps {
  m: SpaceMessage;
  /** Every participant address, for highlighting names in bodies. */
  addresses: string[];
  /** Open the sender's transcript at this message. */
  onOpenInTranscript?: () => void;
  /** The `#msg=` anchor points here. */
  highlight?: boolean;
  /** The directive a retraction lifted, for its marker. */
  retracted?: SpaceMessage;
}

export function MessageRow({ m, addresses, onOpenInTranscript, highlight, retracted }: MessageRowProps) {
  const mode = useThemeMode();
  const kind = rowKind(m);
  const anchor = { id: `msg-${m.id}`, 'data-msg-id': m.id, 'data-kind': kind };
  const ring = highlight ? 'ring-2 ring-brand-500/60' : '';

  if (kind === 'directive' || kind === 'retract') {
    const lifted = kind === 'retract';
    return (
      <div {...anchor} className={cn('group/msg flex items-center gap-1.5 rounded-md px-2 py-1 text-meta text-ink-mute', ring)}>
        {lifted ? <PinOff size={11} /> : <Pin size={11} className="text-amber-500" />}
        <Sender address={m.from} />
        <span>{lifted ? 'retracted a directive' : 'posted a directive'}</span>
        {m.to && <AddressPill address={m.to} />}
        <span className="min-w-0 truncate italic">“{lifted ? (retracted?.body ?? m.refs?.retracts ?? '') : m.body}”</span>
        <Time ts={m.ts} onOpen={onOpenInTranscript} />
      </div>
    );
  }

  if (kind === 'broadcast') {
    const tint = identityOf(m.from, mode).color;
    return (
      <div {...anchor} className={cn('group/msg my-1 rounded-lg border px-3 py-2', ring)} style={{ borderColor: hexA(tint, 0.35), background: hexA(tint, 0.07) }}>
        <div className="flex items-center gap-2">
          <Avatar address={m.from} />
          <Sender address={m.from} />
          <span className="inline-flex items-center gap-1 rounded px-1.5 py-px text-[10px] font-semibold uppercase leading-none tracking-wide" style={{ background: hexA(tint, 0.18), color: tint }}>
            <Megaphone size={10} />
            broadcast
          </span>
          <span className="text-meta text-ink-mute">to everyone</span>
          <Time ts={m.ts} onOpen={onOpenInTranscript} />
        </div>
        <Body body={m.body} addresses={addresses} className="pl-8 text-ink" />
        <div className="pl-8">
          <RowFooter m={m} />
        </div>
      </div>
    );
  }

  if (kind === 'operator') {
    return (
      <div {...anchor} className={cn('group/msg relative my-1 flex gap-2.5 rounded-md px-2 py-1.5', m.urgent ? 'bg-err-bg/50' : 'bg-surface/60', ring)} style={{ borderLeft: `3px solid ${m.urgent ? 'rgb(var(--c-err))' : identityOf('operator', mode).color}` }}>
        <div className="pt-0.5">
          <Avatar address="operator" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
            <Sender address="operator" />
            {m.viewer && <span className="text-meta text-ink-mute">{m.viewer}</span>}
            {m.urgent && <span className="rounded bg-err px-1.5 py-px text-[10px] font-bold uppercase leading-none tracking-wide text-white">urgent</span>}
            <span className="inline-flex items-center gap-0.5 text-meta text-ink-mute">
              → <AddressPill address={m.to ?? 'broadcast'} />
            </span>
            {m.via && <span className="text-meta text-ink-mute">via {m.via === 'cp' ? 'control plane' : 'CLI'}</span>}
            <Time ts={m.ts} onOpen={onOpenInTranscript} />
          </div>
          <Body body={m.body} addresses={addresses} className="text-ink" />
          <RowFooter m={m} />
        </div>
      </div>
    );
  }

  if (kind === 'direct') {
    return (
      <div {...anchor} className={cn('group/msg my-0.5 flex gap-2.5 rounded-md border border-dashed border-border bg-sky-500/[0.04] px-2 py-1.5', ring)}>
        <div className="pt-0.5">
          <Avatar address={m.from} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
            <Sender address={m.from} />
            <span className="text-meta text-ink-mute">→</span>
            <Sender address={m.to ?? ''} />
            <span className="inline-flex items-center gap-1 rounded bg-sky-500/10 px-1.5 py-px text-[10px] font-medium leading-none text-sky-600 dark:text-sky-300">
              <Lock size={9} />
              direct
            </span>
            <Time ts={m.ts} onOpen={onOpenInTranscript} />
          </div>
          <Body body={m.body} addresses={addresses} />
          <RowFooter m={m} />
        </div>
      </div>
    );
  }

  const to = kind === 'mention' ? m.to : null;
  const rTint = to ? identityOf(to, mode).color : null;
  return (
    <div
      {...anchor}
      className={cn('group/msg relative flex gap-2.5 rounded-md px-2 py-1.5 transition-colors', !to && 'hover:bg-surface/40', ring)}
      style={rTint ? { borderLeft: `3px solid ${rTint}`, background: hexA(rTint, 0.06) } : undefined}
    >
      <div className="pt-0.5">
        <Avatar address={m.from} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
          <Sender address={m.from} />
          {to && (
            <span className="inline-flex items-center gap-0.5 text-meta text-ink-mute">
              → <AddressPill address={to} />
            </span>
          )}
          {m.post_kind && <PostKind kind={m.post_kind} />}
          <Time ts={m.ts} onOpen={onOpenInTranscript} />
        </div>
        <Body body={m.body} addresses={addresses} />
        <RowFooter m={m} />
      </div>
    </div>
  );
}
