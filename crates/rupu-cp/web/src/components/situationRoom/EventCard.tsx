// One editorial card in the Situation Room live stream (also reused by the
// per-run event feed). Pure render of a StreamCard (built by
// lib/situationRoom/cards.ts) plus, for `await` cards, inline Approve / Reject
// wired to the real run-control API by the caller.
//
// Findings get the rich treatment — severity accent, a file:line that deep-
// links to the Code viewer, the evidence rationale, the real code excerpt with
// line numbers + syntax highlighting (CodeExcerpt), and an SCM permalink when
// present. Errors render via ErrorDetail (Parsed/Raw for JSON). Agent activity
// renders honestly (avatar + agent + step + note); no fabricated code.

import { useState } from 'react';
import { AlertTriangle, CheckCircle2, Cog, ExternalLink, Pause, PlayCircle, Search, ShieldAlert, Users } from 'lucide-react';
import { Link } from 'react-router-dom';
import { cn } from '../../lib/cn';
import type { CardForm, StreamCard } from '../../lib/situationRoom/cards';
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

function ActorIcon({ form }: { form: CardForm }) {
  const cls = 'w-[13px] h-[13px]';
  switch (form) {
    case 'await': return <Pause className={cls} />;
    case 'error': return <AlertTriangle className={cls} />;
    case 'complete': return <CheckCircle2 className={cls} />;
    case 'panel': return <Users className={cls} />;
    case 'lifecycle': return <PlayCircle className={cls} />;
    default: return <Cog className={cls} />;
  }
}

/** Avatar tint follows the card accent so an error/awaiting card doesn't read
 *  as routine brand-purple activity. */
function avatarStyle(accent: StreamCard['accent']): React.CSSProperties | undefined {
  if (accent === 'error') return { background: 'rgb(var(--c-status-failed)/.12)', color: 'rgb(var(--c-status-failed))', borderColor: 'rgb(var(--c-status-failed)/.35)' };
  if (accent === 'await') return { background: 'rgb(var(--c-status-awaiting)/.14)', color: 'rgb(var(--c-status-awaiting))', borderColor: 'rgb(var(--c-status-awaiting)/.35)' };
  return undefined;
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
    <article className={cn('sr-ev', `sr-a-${card.accent}`, fresh && 'is-fresh', state.resolved && 'resolved')}>
      <div className="sr-ev-head">
        <span className={cn('sr-badge', `sr-a-${card.accent}`)}>{card.badge}</span>
        {label && (
          <span className="sr-ev-repo">
            {label}
            {branch && <span className="br">{branch}</span>}
          </span>
        )}
        {card.runId && !hideRunLink && (
          <Link to={`/runs/${card.runId}`} className="sr-ev-run hover:text-ink transition-colors" title={`run ${card.runId}`}>
            {card.runId.slice(0, 8)}
          </Link>
        )}
        {card.ts > 0 && (
          <span className="sr-ev-ago" title={new Date(card.ts).toLocaleString()}>
            {rel(card.ts)}
          </span>
        )}
      </div>

      {card.form === 'finding' ? (
        <>
          <div className="sr-ev-title flex items-start gap-2">
            <ShieldAlert className="w-4 h-4 mt-0.5 shrink-0" style={{ color: `rgb(var(--c-sev-${card.severity ?? 'info'}))` }} />
            <span>{card.title}</span>
          </div>
          {card.fileRef && (
            codeHref ? (
              <Link to={codeHref} className="sr-fileref hover:text-ink transition-colors inline-block">{card.fileRef}</Link>
            ) : (
              <div className="sr-fileref">{card.fileRef}</div>
            )
          )}
          {card.detail && <div className="sr-note">{card.detail}</div>}
          {card.code && <CodeExcerpt code={card.code} startLine={card.fileLine} filePath={card.filePath} />}
          {card.permalink && (
            <a
              href={card.permalink}
              target="_blank"
              rel="noreferrer"
              className="mt-2 inline-flex items-center gap-1 text-note text-ink-dim hover:text-ink transition-colors"
            >
              <ExternalLink className="w-3 h-3" /> View on repository
            </a>
          )}
        </>
      ) : card.form === 'error' ? (
        <>
          <div className="sr-actor">
            <span className="sr-avatar" style={avatarStyle('error')}><AlertTriangle className="w-[13px] h-[13px]" /></span>
            <span className="sr-ev-title">{card.title}</span>
          </div>
          {card.detail && <ErrorDetail text={card.detail} />}
        </>
      ) : card.form === 'await' ? (
        <>
          <div className="sr-ev-title">{card.title}</div>
          {card.detail && <div className="sr-note">{card.detail}</div>}
          {state.resolved ? (
            <span className="sr-resolved-tag" style={{ color: `rgb(var(--c-status-${state.resolved === 'approved' ? 'done' : 'failed'}))` }}>
              ✓ {state.resolved} · you
            </span>
          ) : canApprove ? (
            <div className="sr-approve">
              <button className="sr-btn ok" disabled={state.busy} onClick={() => act('approve')}>
                {state.busy ? '…' : 'Approve'}
              </button>
              <button className="sr-btn no" disabled={state.busy} onClick={() => act('reject')}>Reject</button>
            </div>
          ) : (
            <span className="sr-resolved-tag" style={{ color: 'rgb(var(--c-status-awaiting))' }}>Awaiting approval</span>
          )}
          {state.error && <div className="sr-note" style={{ color: 'rgb(var(--c-err))' }}>Could not submit: {state.error}</div>}
        </>
      ) : (
        <>
          <div className="sr-actor">
            <span className="sr-avatar" style={avatarStyle(card.accent)}>
              {card.badge === 'Scanning' ? <Search className="w-[13px] h-[13px]" /> : <ActorIcon form={card.form} />}
            </span>
            <span className="sr-ev-title">{card.title}</span>
          </div>
          {card.detail && <div className="sr-note">{card.detail}</div>}
        </>
      )}
    </article>
  );
}
