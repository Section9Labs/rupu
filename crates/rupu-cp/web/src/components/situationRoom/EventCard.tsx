// One row in the Situation Room live timeline (also reused by the per-run
// feed). A StreamCard (lib/situationRoom/cards.ts) rendered as a timeline
// entry: an absolute timestamp + a status-coloured marker on a connecting rail,
// then a status pill, a plain-language "what happened" headline (agent · step ·
// unit + outcome), workflow/run context, and — for findings — the full rich
// body (severity, file:line, evidence, code, SCM link). `await` rows keep
// inline Approve / Reject; errors render via ErrorDetail. Nothing fabricated: a
// field the event doesn't carry simply isn't shown.

import { lazy, Suspense, useState } from 'react';
import {
  AlertTriangle, CheckCircle2, ChevronDown, ChevronRight, Clock, Coins, Cog,
  ExternalLink, Layers, Pause, PlayCircle, Search, ShieldAlert, SkipForward, Users,
} from 'lucide-react';
import { Link } from 'react-router-dom';
import { cn } from '../../lib/cn';
import { Badge, type BadgeTone } from '../ui/Badge';
import type { DotColor } from '../ui/Dot';
import type { CardForm, StreamCard } from '../../lib/situationRoom/cards';
import { crewTint, memberLabel } from '../../lib/codename';
import { AgentName } from '../codename/AgentName';
import { useThemeMode } from '../codename/useThemeMode';
import CodeExcerpt from './CodeExcerpt';
import ErrorDetail from './ErrorDetail';

// The real "what happened" for a step lives in its transcript (tool calls,
// files read/written, output) — not on the event. Fetch + render it on demand
// when a row is opened, reusing the run transcript renderer. Lazy so the live
// stream's bundle stays lean until someone actually opens a row.
const TranscriptDetail = lazy(() => import('../TranscriptPanel'));

/** Absolute wall-clock, HH:MM:SS — the timeline's left column. */
function clock(ts: number): string {
  if (!ts) return '—';
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}

/** Compact token count: 1234 → 1.2k. */
function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 100) / 10}k`;
  return `${Math.round(n / 100_000) / 10}M`;
}

function KindIcon({ form, badge, className }: { form: CardForm; badge: string; className?: string }) {
  const cls = className ?? 'h-[13px] w-[13px]';
  if (form === 'finding') return <ShieldAlert className={cls} />;
  if (badge === 'Scanning') return <Search className={cls} />;
  if (badge === 'Skipped') return <SkipForward className={cls} />;
  switch (form) {
    case 'await': return <Pause className={cls} />;
    case 'error': return <AlertTriangle className={cls} />;
    case 'complete': return <CheckCircle2 className={cls} />;
    case 'panel': return <Users className={cls} />;
    case 'lifecycle': return <PlayCircle className={cls} />;
    default: return <Cog className={cls} />;
  }
}

/** Status pill tone + rail-marker colour, from the card's outcome. Green =
 *  succeeded, red = failed, amber = awaiting, blue = in-progress, violet =
 *  routine; findings tint by severity. */
function statusVisual(card: StreamCard): { tone: BadgeTone; dot: DotColor } {
  if (card.accent === 'error') return { tone: 'red', dot: 'failed' };
  if (card.accent === 'await') return { tone: 'amber', dot: 'awaiting' };
  if (card.form === 'finding') {
    const s = card.severity;
    if (s === 'critical' || s === 'high') return { tone: 'red', dot: 'failed' };
    if (s === 'medium') return { tone: 'amber', dot: 'awaiting' };
    return { tone: 'sky', dot: 'mute' };
  }
  if (card.form === 'complete') return { tone: 'green', dot: 'done' };
  if (card.badge === 'Skipped') return { tone: 'neutral', dot: 'mute' };
  if (card.badge === 'Paused') return { tone: 'amber', dot: 'awaiting' };
  return { tone: 'sky', dot: 'running' }; // scanning / working / fan-out / started
}

export interface ApproveState {
  busy: boolean;
  resolved?: 'approved' | 'rejected';
  error?: string;
}

/** A labelled meta chip (unit / duration / tokens / round). */
function Meta({ icon, children }: { icon?: React.ReactNode; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono tabular-nums text-meta text-ink-mute">
      {icon}
      {children}
    </span>
  );
}

const DOT_RGB: Record<DotColor, string> = {
  brand: 'var(--c-brand-500)',
  awaiting: 'var(--c-status-awaiting)',
  failed: 'var(--c-status-failed)',
  done: 'var(--c-status-done)',
  running: 'var(--c-status-running)',
  mute: 'var(--c-ink-mute)',
};

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
  hideRunLink?: boolean;
  onApprove?: (runId: string) => Promise<void>;
  onReject?: (runId: string) => Promise<void>;
}) {
  const [state, setState] = useState<ApproveState>({ busy: false });
  const [open, setOpen] = useState(false);

  const label = projectLabel ?? card.projectName;
  const wf = workflow ?? card.workflow;
  const canApprove = card.form === 'await' && !!onApprove && !!onReject;
  const { tone, dot } = statusVisual(card);
  const mode = useThemeMode();
  const tint = card.crew ? crewTint(card.crew, mode) : undefined;
  // agent_started's headline already IS the member label — don't repeat it
  // after the name.
  const titleIsName = card.title === memberLabel(card.codename, card.agent, card.provider, card.model);

  const durationS = card.durationMs != null ? `${Math.round(card.durationMs / 100) / 10}s` : undefined;
  const hasMeta = !!(card.unitKey || durationS || card.tokensIn != null || card.round);
  const showNote = !!card.detail && card.form !== 'complete' && card.form !== 'finding' && card.form !== 'error';

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
    <article data-testid="sr-ev" data-accent={card.accent} className={cn('relative flex gap-2.5', fresh && 'sr-fresh')}>
      {/* Timeline column: absolute time + status marker on a connecting rail */}
      <time
        className="w-[52px] shrink-0 pt-0.5 text-right font-mono text-meta tabular-nums text-ink-mute"
        title={card.ts ? new Date(card.ts).toLocaleString() : undefined}
      >
        {clock(card.ts)}
      </time>
      <div className="relative flex w-3 shrink-0 justify-center" aria-hidden>
        <span className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border" />
        <span
          className="relative z-10 mt-[5px] block h-2.5 w-2.5 rounded-full ring-4 ring-panel"
          style={{ background: `rgb(${DOT_RGB[dot]})` }}
        />
      </div>

      {/* Content — a crew tint stripe runs down its left edge when the run
          has a codename (palette colour, never hardcoded). */}
      <div className={cn('relative min-w-0 flex-1 pb-4', tint && 'pl-2.5', state.resolved && 'opacity-70')}>
        {tint && (
          <span
            data-testid="sr-crew-stripe"
            aria-hidden
            className="absolute bottom-4 left-0 top-0.5 w-0.5 rounded-full"
            style={{ backgroundColor: tint }}
          />
        )}
        {/* Line 1: status pill · workflow · run */}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Badge tone={tone}>{card.badge}</Badge>
          {label && (
            <span className="inline-flex min-w-0 items-center gap-1.5 truncate font-mono text-meta text-ink-dim">
              {label}
              {branch && <span className="text-ink-mute">{branch}</span>}
            </span>
          )}
          {wf && (
            <span className="inline-flex min-w-0 items-center gap-1 truncate font-mono text-meta text-ink-mute" title={`workflow ${wf}`}>
              <span className="text-ink-mute/50">/</span>{wf}
            </span>
          )}
          {card.runId && !hideRunLink && (
            <Link to={`/runs/${card.runId}`} className="shrink-0 font-mono text-meta text-ink-mute transition-colors hover:text-ink" title={`run ${card.runId}`}>
              {card.crew ?? card.runId.slice(0, 8)}
            </Link>
          )}
        </div>

        {/* Line 2: what happened — icon · agent · step/what */}
        <div className="mt-1 flex items-start gap-1.5 text-sm font-medium text-ink">
          <span className="mt-px shrink-0 text-ink-mute"><KindIcon form={card.form} badge={card.badge} className="h-[14px] w-[14px]" /></span>
          {card.codename ? (
            <>
              <span className="shrink-0 text-brand-700">
                <AgentName codename={card.codename} agent={card.agent} provider={card.provider} model={card.model} />
              </span>
              {!titleIsName && <span className="shrink-0 text-ink-mute">·</span>}
              {!titleIsName && <span className="min-w-0">{card.title}</span>}
            </>
          ) : titleIsName ? (
            <span className="min-w-0 font-mono text-brand-700">{card.title}</span>
          ) : (
            <>
              {card.agent && <span className="shrink-0 font-mono text-brand-700">{card.agent}</span>}
              {card.agent && <span className="shrink-0 text-ink-mute">·</span>}
              <span className="min-w-0">{card.title}</span>
            </>
          )}
        </div>

        {/* Meta: unit target · round · duration · tokens */}
        {hasMeta && (
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
            {card.unitKey && <Meta icon={<Layers className="h-3 w-3" />}>{card.unitKey}</Meta>}
            {card.round && <Meta>round {card.round.n}/{card.round.max}</Meta>}
            {durationS && <Meta icon={<Clock className="h-3 w-3" />}>{durationS}</Meta>}
            {card.tokensIn != null && card.tokensOut != null && (
              <Meta icon={<Coins className="h-3 w-3" />}>{fmtTokens(card.tokensIn)}→{fmtTokens(card.tokensOut)} tok</Meta>
            )}
          </div>
        )}

        {/* Secondary note (real notes / reasons) */}
        {showNote && <p className="mt-1 text-note leading-relaxed text-ink-dim">{card.detail}</p>}

        {/* Finding: the rich, detailed body */}
        {card.form === 'finding' && (
          <>
            {card.fileRef &&
              (codeHref ? (
                <Link to={codeHref} className="mt-1 inline-block font-mono text-meta text-ink-mute transition-colors hover:text-ink">{card.fileRef}</Link>
              ) : (
                <div className="mt-1 font-mono text-meta text-ink-mute">{card.fileRef}</div>
              ))}
            {card.detail && <p className="mt-1 text-note leading-relaxed text-ink-dim">{card.detail}</p>}
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
            <span className="mt-1.5 inline-block text-note font-medium" style={{ color: `rgb(var(--c-status-${state.resolved === 'approved' ? 'done' : 'failed'}))` }}>
              ✓ {state.resolved} · you
            </span>
          ) : canApprove ? (
            <div className="mt-2 flex max-w-[320px] gap-2">
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
            <span className="mt-1.5 inline-block text-note font-medium" style={{ color: 'rgb(var(--c-status-awaiting))' }}>Awaiting approval</span>
          )
        )}
        {card.form === 'await' && state.error && <div className="mt-1.5 text-note" style={{ color: 'rgb(var(--c-err))' }}>Could not submit: {state.error}</div>}

        {/* Expand → the step's actual transcript: which files, tool calls,
            output. Only rows that carry a transcript path can open. */}
        {card.transcriptPath && (
          <div className="mt-2">
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              aria-expanded={open}
              className="inline-flex items-center gap-1 text-meta font-medium text-ink-mute transition-colors hover:text-ink"
            >
              {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
              {open ? 'Hide what happened' : 'Show what happened'}
            </button>
            {open && (
              <div className="mt-2 max-h-[440px] overflow-auto rounded-md border border-border bg-surface/40 p-2">
                <Suspense fallback={<div className="p-3 text-meta text-ink-mute">Loading transcript…</div>}>
                  <TranscriptDetail path={card.transcriptPath} runId={card.runId} live={false} embedded />
                </Suspense>
              </div>
            )}
          </div>
        )}
      </div>
    </article>
  );
}
