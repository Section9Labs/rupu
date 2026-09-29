// One event in the Situation Room live stream (also reused by the per-run
// feed). A context-rich, openable timeline row built from a StreamCard
// (lib/situationRoom/cards.ts): it surfaces WHICH agent, in WHICH workflow, for
// WHICH project, and WHAT it did — plus step / unit / duration / tokens when the
// event carries them — and expands to show the ids + transcript path. `await`
// cards keep inline Approve / Reject; findings keep the rich severity + code
// treatment; errors render via ErrorDetail. Nothing is fabricated: a field the
// event doesn't carry simply isn't shown.
//
// Visual language mirrors Ghost's activity view: bordered `bg-panel` cards,
// notable accents tint the border + a faint wash, a tone Badge, a strict
// text-sm/note/meta hierarchy, mono ids and tabular-nums.

import { useState } from 'react';
import {
  AlertTriangle, CheckCircle2, ChevronDown, ChevronRight, Clock, Coins, Cog,
  ExternalLink, Layers, Pause, PlayCircle, Search, ShieldAlert, Users,
} from 'lucide-react';
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

/** Compact token count: 1234 → 1.2k. */
function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 100) / 10}k`;
  return `${Math.round(n / 100_000) / 10}M`;
}

function ActorIcon({ form, className }: { form: CardForm; className?: string }) {
  const cls = className ?? 'h-[13px] w-[13px]';
  switch (form) {
    case 'await': return <Pause className={cls} />;
    case 'error': return <AlertTriangle className={cls} />;
    case 'complete': return <CheckCircle2 className={cls} />;
    case 'panel': return <Users className={cls} />;
    case 'lifecycle': return <PlayCircle className={cls} />;
    default: return <Cog className={cls} />;
  }
}

/** Accent → Badge tone + optional border/wash tint. Routine `brand`/`info`
 *  stay untinted; notable accents tint the card the way Ghost does. */
function accentVisual(accent: CardAccent): { tone: BadgeTone; tint?: React.CSSProperties } {
  const sev = (name: string, b: number, g: number): React.CSSProperties => ({
    borderColor: `rgb(var(--c-sev-${name})/${b})`,
    background: `rgb(var(--c-sev-${name})/${g})`,
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

/** A labelled meta chip in the context row (step / unit / duration / tokens). */
function Meta({ icon, children }: { icon?: React.ReactNode; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono tabular-nums text-meta text-ink-mute">
      {icon}
      {children}
    </span>
  );
}

export default function EventCard({
  card,
  projectLabel,
  branch,
  workflow,
  fresh,
  hideRunLink,
  onApprove,
  onReject,
}: {
  card: StreamCard;
  projectLabel?: string;
  branch?: string;
  workflow?: string;
  fresh?: boolean;
  /** Per-run feeds already scope every card to one run — hide the redundant link. */
  hideRunLink?: boolean;
  onApprove?: (runId: string) => Promise<void>;
  onReject?: (runId: string) => Promise<void>;
}) {
  const [state, setState] = useState<ApproveState>({ busy: false });
  const [open, setOpen] = useState(false);

  const label = projectLabel ?? card.projectName;
  const wf = workflow ?? card.workflow;
  const canApprove = card.form === 'await' && !!onApprove && !!onReject;
  const { tone, tint } = accentVisual(card.accent);

  // The headline "what": prefer the agent, then the action title.
  const durationS = card.durationMs != null ? `${Math.round(card.durationMs / 100) / 10}s` : undefined;
  const hasMeta = !!(card.stepId || card.unitKey || durationS || card.tokensIn != null || card.round || card.stepKind);
  // Extra detail worth an expand: the transcript path + the full ids.
  const hasExpand = !!(card.transcriptPath || card.runId || card.stepId || card.unitKey);

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

  // Whether to show `detail` as a note line. For `complete` cards the detail is
  // the redundant "ok · 5.2s / a→b tok" summary — already in the meta chips.
  const showNote = !!card.detail && card.form !== 'complete' && card.form !== 'finding' && card.form !== 'error';

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
      {/* Header: badge · project · workflow · run · ago */}
      <div className="flex items-center gap-2">
        <Badge tone={tone}>{card.badge}</Badge>
        {label && (
          <span className="inline-flex min-w-0 items-center gap-1.5 truncate font-mono text-note font-medium text-ink">
            {label}
            {branch && <span className="font-normal text-ink-mute">{branch}</span>}
          </span>
        )}
        {wf && (
          <span className="inline-flex min-w-0 shrink items-center gap-1 truncate font-mono text-meta text-ink-mute" title={`workflow ${wf}`}>
            <span className="text-ink-mute/60">/</span>
            {wf}
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
          <span className="ml-auto shrink-0 font-mono text-meta tabular-nums text-ink-mute" title={new Date(card.ts).toLocaleString()}>
            {rel(card.ts)}
          </span>
        )}
      </div>

      {/* Headline: icon · agent · what */}
      <div className="mt-2 flex items-start gap-2 text-sm font-medium text-ink">
        <span className="mt-px shrink-0" style={card.form === 'finding' ? { color: `rgb(var(--c-sev-${card.severity ?? 'info'}))` } : card.form === 'error' ? { color: 'rgb(var(--c-status-failed))' } : card.form === 'await' ? { color: 'rgb(var(--c-status-awaiting))' } : undefined}>
          {card.form === 'finding' ? <ShieldAlert className="h-4 w-4" /> : card.badge === 'Scanning' ? <Search className="h-[14px] w-[14px] text-ink-mute" /> : <ActorIcon form={card.form} className="h-[14px] w-[14px] text-ink-mute" />}
        </span>
        {card.agent && <span className="shrink-0 font-mono text-brand-700">{card.agent}</span>}
        <span className="min-w-0">{card.title}</span>
      </div>

      {/* Meta chips: step · unit · duration · tokens · round */}
      {hasMeta && (
        <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1">
          {card.stepId && <Meta icon={<Layers className="h-3 w-3" />}>{card.stepId}</Meta>}
          {card.unitKey && <Meta>· {card.unitKey}</Meta>}
          {card.round && <Meta>round {card.round.n}/{card.round.max}</Meta>}
          {durationS && <Meta icon={<Clock className="h-3 w-3" />}>{durationS}</Meta>}
          {card.tokensIn != null && card.tokensOut != null && (
            <Meta icon={<Coins className="h-3 w-3" />}>{fmtTokens(card.tokensIn)}→{fmtTokens(card.tokensOut)}</Meta>
          )}
        </div>
      )}

      {/* Secondary note (real notes / reasons; not the redundant complete summary) */}
      {showNote && <p className="mt-1.5 text-note leading-relaxed text-ink-dim">{card.detail}</p>}

      {/* Finding extras */}
      {card.form === 'finding' && (
        <>
          {card.fileRef &&
            (codeHref ? (
              <Link to={codeHref} className="mt-1 inline-block font-mono text-meta text-ink-mute transition-colors hover:text-ink">{card.fileRef}</Link>
            ) : (
              <div className="mt-1 font-mono text-meta text-ink-mute">{card.fileRef}</div>
            ))}
          {card.detail && <p className="mt-1.5 text-note leading-relaxed text-ink-dim">{card.detail}</p>}
          {card.code && <div className="mt-2"><CodeExcerpt code={card.code} startLine={card.fileLine} filePath={card.filePath} /></div>}
          {card.permalink && (
            <a href={card.permalink} target="_blank" rel="noreferrer" className="mt-2 inline-flex items-center gap-1 text-meta text-ink-dim transition-colors hover:text-ink">
              <ExternalLink className="h-3 w-3" /> View on repository
            </a>
          )}
        </>
      )}

      {/* Error body */}
      {card.form === 'error' && card.detail && <div className="mt-2"><ErrorDetail text={card.detail} /></div>}

      {/* Await approve / reject */}
      {card.form === 'await' && (
        state.resolved ? (
          <span className="mt-2 inline-block text-note font-medium" style={{ color: `rgb(var(--c-status-${state.resolved === 'approved' ? 'done' : 'failed'}))` }}>
            ✓ {state.resolved} · you
          </span>
        ) : canApprove ? (
          <div className="mt-2.5 flex max-w-[320px] gap-2">
            <button type="button" disabled={state.busy} onClick={() => act('approve')}
              className="flex-1 rounded-md border px-3 py-1.5 text-note font-semibold transition-colors disabled:opacity-50"
              style={{ background: 'rgb(var(--c-status-done)/.12)', color: 'rgb(var(--c-status-done))', borderColor: 'rgb(var(--c-status-done)/.35)' }}>
              {state.busy ? '…' : 'Approve'}
            </button>
            <button type="button" disabled={state.busy} onClick={() => act('reject')}
              className="flex-1 rounded-md border px-3 py-1.5 text-note font-semibold transition-colors disabled:opacity-50"
              style={{ background: 'rgb(var(--c-status-failed)/.1)', color: 'rgb(var(--c-status-failed))', borderColor: 'rgb(var(--c-status-failed)/.35)' }}>
              Reject
            </button>
          </div>
        ) : (
          <span className="mt-2 inline-block text-note font-medium" style={{ color: 'rgb(var(--c-status-awaiting))' }}>Awaiting approval</span>
        )
      )}
      {card.form === 'await' && state.error && <div className="mt-1.5 text-note" style={{ color: 'rgb(var(--c-err))' }}>Could not submit: {state.error}</div>}

      {/* Expand: ids + transcript path */}
      {hasExpand && (
        <div className="mt-2">
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
            className="inline-flex items-center gap-1 text-meta text-ink-mute transition-colors hover:text-ink"
          >
            {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
            {open ? 'Hide detail' : 'Detail'}
          </button>
          {open && (
            <dl className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 rounded-md border border-border bg-surface/60 px-3 py-2 text-meta">
              {card.runId && (<><dt className="text-ink-mute">run</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{card.runId}</dd></>)}
              {wf && (<><dt className="text-ink-mute">workflow</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{wf}</dd></>)}
              {card.stepId && (<><dt className="text-ink-mute">step</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{card.stepId}{card.stepKind ? ` (${card.stepKind})` : ''}</dd></>)}
              {card.unitKey && (<><dt className="text-ink-mute">unit</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{card.unitKey}</dd></>)}
              {card.agent && (<><dt className="text-ink-mute">agent</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{card.agent}</dd></>)}
              {card.transcriptPath && (<><dt className="text-ink-mute">transcript</dt><dd className="min-w-0 break-all font-mono text-ink-dim">{card.transcriptPath}</dd></>)}
            </dl>
          )}
        </div>
      )}
    </article>
  );
}
