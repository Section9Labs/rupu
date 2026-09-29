// One editorial card in the Situation Room live stream (also reused by the
// per-run event feed). Pure render of a StreamCard (built by
// lib/situationRoom/cards.ts) plus, for `await` cards, inline Approve / Reject
// wired to the real run-control API by the caller.
//
// Visual language mirrors Ghost's ActivityView: a bordered `bg-panel` card,
// notable accents (await / error / finding severity) tint the border + a faint
// wash, a tone Badge pill, a strict text-sm/note/meta hierarchy, mono run ids
// and tabular-nums timestamps. Findings keep the rich treatment (severity
// icon, file:line deep-link, evidence, real code excerpt, SCM permalink);
// errors render via ErrorDetail. Nothing fabricated.

import { useState } from 'react';
import { AlertTriangle, CheckCircle2, Cog, ExternalLink, Pause, PlayCircle, Search, ShieldAlert, Users } from 'lucide-react';
import { Link } from 'react-router-dom';
import { cn } from '../../lib/cn';
import { Badge, type BadgeTone } from '../ui/Badge';
import type { CardForm, CardAccent, StreamCard } from '../../lib/situationRoom/cards';
import CodeExcerpt from './CodeExcerpt';
import ErrorDetail from './ErrorDetail';

/** Relative "time ago" from a ms timestamp. */
function rel(ts: number): string {
  const sec = Math.round((Date.now() - ts) / 1000);
  if (sec < 5) return 'now';
  if (sec < 60) return `${sec}s`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h`;
  return `${Math.round(hr / 24)}d`;
}

function ActorIcon({ form, className }: { form: CardForm; className?: string }) {
  const cls = className ?? 'w-[13px] h-[13px]';
  switch (form) {
    case 'await': return <Pause className={cls} />;
    case 'error': return <AlertTriangle className={cls} />;
    case 'complete': return <CheckCircle2 className={cls} />;
    case 'panel': return <Users className={cls} />;
    case 'lifecycle': return <PlayCircle className={cls} />;
    default: return <Cog className={cls} />;
  }
}

/** Map a card accent to a Badge tone + optional border/wash tint. Routine
 *  `brand` activity and `info` stay untinted (plain bordered card that lifts on
 *  hover); notable accents tint the whole card the way Ghost tints a card for a
 *  finding or a failure. */
function accentVisual(accent: CardAccent): { tone: BadgeTone; tint?: React.CSSProperties } {
  const sev = (name: string, borderA: number, bgA: number): React.CSSProperties => ({
    borderColor: `rgb(var(--c-sev-${name})/${borderA})`,
    background: `rgb(var(--c-sev-${name})/${bgA})`,
  });
  switch (accent) {
    case 'await':
      return { tone: 'amber', tint: { borderColor: 'rgb(var(--c-status-awaiting)/.4)', background: 'rgb(var(--c-status-awaiting)/.06)' } };
    case 'error':
      return { tone: 'red', tint: { borderColor: 'rgb(var(--c-status-failed)/.4)', background: 'rgb(var(--c-status-failed)/.05)' } };
    case 'critical': return { tone: 'red', tint: sev('critical', 0.4, 0.06) };
    case 'high': return { tone: 'red', tint: sev('high', 0.4, 0.05) };
    case 'medium': return { tone: 'amber', tint: sev('medium', 0.4, 0.05) };
    case 'low': return { tone: 'sky', tint: sev('low', 0.35, 0.04) };
    case 'info': return { tone: 'neutral' };
    default: return { tone: 'brand' };
  }
}

export interface ApproveState {
  busy: boolean;
  resolved?: 'approved' | 'rejected';
  error?: string;
}

export default function EventCard({
  card,
  projectLabel,
  branch,
  fresh,
  hideRunLink,
  onApprove,
  onReject,
}: {
  card: StreamCard;
  projectLabel?: string;
  branch?: string;
  fresh?: boolean;
  /** Per-run feeds already scope every card to one run — hide the redundant link. */
  hideRunLink?: boolean;
  onApprove?: (runId: string) => Promise<void>;
  onReject?: (runId: string) => Promise<void>;
}) {
  const [state, setState] = useState<ApproveState>({ busy: false });

  const label = projectLabel ?? card.projectName;
  const canApprove = card.form === 'await' && !!onApprove && !!onReject;
  const { tone, tint } = accentVisual(card.accent);

  async function act(kind: 'approve' | 'reject') {
    const runId = card.approvable?.runId;
    if (!runId) return;
    setState({ busy: true });
    try {
      if (kind === 'approve') await onApprove?.(runId);
      else await onReject?.(runId);
      setState({ busy: false, resolved: kind === 'approve' ? 'approved' : 'rejected' });
    } catch (e) {
      setState({ busy: false, error: e instanceof Error ? e.message : String(e) });
    }
  }

  const codeHref =
    card.wsId && card.filePath
      ? `/projects/${card.wsId}/code?path=${encodeURIComponent(card.filePath)}${card.fileLine ? `&line=${card.fileLine}` : ''}`
      : undefined;

  return (
    <article
      data-testid="sr-ev"
      data-accent={card.accent}
      className={cn(
        'rounded-lg border bg-panel px-3.5 py-3 transition-colors',
        !tint && 'border-border hover:border-ink-mute',
        fresh && 'ring-1 ring-brand-500/30',
        state.resolved && 'opacity-70',
      )}
      style={tint}
    >
      {/* Header: badge · project/branch · run · ago */}
      <div className="flex items-center gap-2">
        <Badge tone={tone}>{card.badge}</Badge>
        {label && (
          <span className="inline-flex min-w-0 items-center gap-1.5 truncate font-mono text-note font-medium text-ink">
            {label}
            {branch && <span className="font-normal text-ink-mute">{branch}</span>}
          </span>
        )}
        {card.runId && !hideRunLink && (
          <Link
            to={`/runs/${card.runId}`}
            className="shrink-0 font-mono text-meta text-ink-mute transition-colors hover:text-ink"
            title={`run ${card.runId}`}
          >
            {card.runId.slice(0, 8)}
          </Link>
        )}
        {card.ts > 0 && (
          <span
            className="ml-auto shrink-0 font-mono text-meta tabular-nums text-ink-mute"
            title={new Date(card.ts).toLocaleString()}
          >
            {rel(card.ts)}
          </span>
        )}
      </div>

      {/* Body per form */}
      <div className="mt-2">
        {card.form === 'finding' ? (
          <>
            <div className="flex items-start gap-2 text-sm font-medium text-ink">
              <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" style={{ color: `rgb(var(--c-sev-${card.severity ?? 'info'}))` }} />
              <span className="min-w-0">{card.title}</span>
            </div>
            {card.fileRef &&
              (codeHref ? (
                <Link to={codeHref} className="mt-1 inline-block font-mono text-meta text-ink-mute transition-colors hover:text-ink">
                  {card.fileRef}
                </Link>
              ) : (
                <div className="mt-1 font-mono text-meta text-ink-mute">{card.fileRef}</div>
              ))}
            {card.detail && <p className="mt-1.5 text-note leading-relaxed text-ink-dim">{card.detail}</p>}
            {card.code && <div className="mt-2"><CodeExcerpt code={card.code} startLine={card.fileLine} filePath={card.filePath} /></div>}
            {card.permalink && (
              <a
                href={card.permalink}
                target="_blank"
                rel="noreferrer"
                className="mt-2 inline-flex items-center gap-1 text-meta text-ink-dim transition-colors hover:text-ink"
              >
                <ExternalLink className="h-3 w-3" /> View on repository
              </a>
            )}
          </>
        ) : card.form === 'error' ? (
          <>
            <div className="flex items-center gap-2 text-sm font-medium text-ink">
              <AlertTriangle className="h-[15px] w-[15px] shrink-0" style={{ color: 'rgb(var(--c-status-failed))' }} />
              <span className="min-w-0">{card.title}</span>
            </div>
            {card.detail && <div className="mt-2"><ErrorDetail text={card.detail} /></div>}
          </>
        ) : card.form === 'await' ? (
          <>
            <div className="flex items-center gap-2 text-sm font-medium text-ink">
              <Pause className="h-[15px] w-[15px] shrink-0" style={{ color: 'rgb(var(--c-status-awaiting))' }} />
              <span className="min-w-0">{card.title}</span>
            </div>
            {card.detail && <p className="mt-1.5 text-note leading-relaxed text-ink-dim">{card.detail}</p>}
            {state.resolved ? (
              <span
                className="mt-2 inline-block text-note font-medium"
                style={{ color: `rgb(var(--c-status-${state.resolved === 'approved' ? 'done' : 'failed'}))` }}
              >
                ✓ {state.resolved} · you
              </span>
            ) : canApprove ? (
              <div className="mt-2.5 flex max-w-[320px] gap-2">
                <button
                  type="button"
                  disabled={state.busy}
                  onClick={() => act('approve')}
                  className="flex-1 rounded-md border px-3 py-1.5 text-note font-semibold transition-colors disabled:opacity-50"
                  style={{ background: 'rgb(var(--c-status-done)/.12)', color: 'rgb(var(--c-status-done))', borderColor: 'rgb(var(--c-status-done)/.35)' }}
                >
                  {state.busy ? '…' : 'Approve'}
                </button>
                <button
                  type="button"
                  disabled={state.busy}
                  onClick={() => act('reject')}
                  className="flex-1 rounded-md border px-3 py-1.5 text-note font-semibold transition-colors disabled:opacity-50"
                  style={{ background: 'rgb(var(--c-status-failed)/.1)', color: 'rgb(var(--c-status-failed))', borderColor: 'rgb(var(--c-status-failed)/.35)' }}
                >
                  Reject
                </button>
              </div>
            ) : (
              <span className="mt-2 inline-block text-note font-medium" style={{ color: 'rgb(var(--c-status-awaiting))' }}>
                Awaiting approval
              </span>
            )}
            {state.error && <div className="mt-1.5 text-note" style={{ color: 'rgb(var(--c-err))' }}>Could not submit: {state.error}</div>}
          </>
        ) : (
          <>
            <div className="flex items-center gap-2 text-sm font-medium text-ink">
              <span className="text-ink-mute">
                {card.badge === 'Scanning' ? <Search className="h-[13px] w-[13px]" /> : <ActorIcon form={card.form} />}
              </span>
              <span className="min-w-0 truncate">{card.title}</span>
            </div>
            {card.detail && <p className="mt-1.5 text-note leading-relaxed text-ink-dim">{card.detail}</p>}
          </>
        )}
      </div>
    </article>
  );
}
