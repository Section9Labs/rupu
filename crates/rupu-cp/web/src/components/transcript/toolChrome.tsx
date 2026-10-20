/**
 * toolChrome — the shared visual vocabulary every bespoke tool body draws from,
 * so the whole transcript reader reads as ONE system rather than a pile of
 * one-off cards.
 *
 * The contract (CP visual language):
 *   • one chip shape    — `Chip`: a pale tint + colored text, optional mono,
 *     tones keyed to the semantic tokens (neutral / ok / warn / err / info /
 *     brand). Use it for every status / count / id / host / kind label.
 *   • one verb pill     — `VerbPill`: the uppercase-mono action label that
 *     leads a bespoke row's summary (claim / release / read / mark / skill …).
 *   • one key/value grid — `KeyVals`: a compact label→value list for the small
 *     structured bodies (connector repo/issue cards) that don't warrant a
 *     full StructuredView.
 *
 * No `any`. STATIC Tailwind class strings only (no `bg-${x}` templates) so the
 * JIT sees every class.
 */

import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';

export type ChipTone = 'neutral' | 'ok' | 'warn' | 'err' | 'info' | 'brand';

const CHIP_TONE: Record<ChipTone, string> = {
  neutral: 'bg-surface text-ink-mute',
  ok: 'bg-ok-bg text-ok',
  warn: 'bg-warn-bg text-warn',
  err: 'bg-err-bg text-err',
  info: 'bg-info-bg text-info',
  brand: 'bg-brand-500/10 text-brand-700',
};

/** The one metadata chip: a pale-tint pill with colored text. `mono` for
 *  code/paths/ids/hosts; plain sans otherwise. */
export function Chip({
  tone = 'neutral',
  mono = false,
  title,
  className,
  children,
}: {
  tone?: ChipTone;
  mono?: boolean;
  title?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-meta leading-tight whitespace-nowrap',
        CHIP_TONE[tone],
        mono && 'font-mono',
        className,
      )}
    >
      {children}
    </span>
  );
}

/** The leading action label of a bespoke tool row's summary: uppercase mono,
 *  tinted by tone. One per family so claim/release/read/mark/skill/connector
 *  read as siblings. */
export function VerbPill({
  tone = 'neutral',
  title,
  children,
}: {
  tone?: ChipTone;
  title?: string;
  children: ReactNode;
}) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-mono font-bold uppercase tracking-wider leading-tight',
        CHIP_TONE[tone],
      )}
    >
      {children}
    </span>
  );
}

/** A compact label→value grid for small structured bodies (connector repo /
 *  issue cards). Values render as supplied (a string, a chip, a link). Rows
 *  whose value is null/undefined are skipped. */
export function KeyVals({ rows }: { rows: [string, ReactNode][] }) {
  const present = rows.filter(([, v]) => v !== null && v !== undefined && v !== '');
  if (present.length === 0) return null;
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
      {present.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="font-mono text-meta text-ink-mute pt-px">{k}</dt>
          <dd className="min-w-0 text-note text-ink-dim break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
