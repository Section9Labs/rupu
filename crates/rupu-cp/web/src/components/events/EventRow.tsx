// One row of the event rail (styles/events.css): a clock, a marker on the
// rail, and either a quiet one-line row or a raised card (a finding, a gate
// that waits on you, a failure). A row that carries a transcript opens to it.
// Cues (a bloom when something lands, a flash on a failure) are set on the
// row's element by the rail, never rendered from state, so a cue replays
// without re-rendering the list.

import { lazy, memo, Suspense, type CSSProperties, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { Check, ExternalLink } from 'lucide-react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import { ctxOf, isRaised, markOf, rowText, type CtxPart, type GateOutcome, type Mark, type RailScope, type Resolved } from '../../lib/events/rail';
import { parseErrorDetail } from '../../lib/situationRoom/errorDetail';
import { languageForPath } from '../../lib/situationRoom/lang';
import CodeHighlight from '../CodeHighlight';
import ErrorDetail from '../situationRoom/ErrorDetail';
import { EvCap, EvGlyph } from './parts';
import GateButtons, { type EventGate } from './GateButtons';

const TranscriptPanel = lazy(() => import('../TranscriptPanel'));

/** HH:MM:SS — the rail's left column. */
export function clock(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}

export function Ctx({ parts }: { parts: CtxPart[] }) {
  return (
    <>
      {parts.map((p, i) => {
        let body: ReactNode = p.text;
        if (p.codename) {
          body = (
            <span
              className="evx-cn"
              data-derived={p.codename.derived ? '' : undefined}
              title={p.codename.derived ? 'Name derived for a run recorded before codenames' : undefined}
            >
              {p.codename.crew && <>{p.codename.crew}/</>}
              <b>{p.codename.leaf}</b>
            </span>
          );
        } else if (p.mono) {
          body = <span className="evx-cn">{p.text}</span>;
        }
        return (
          <span key={i}>
            {i > 0 && <span className="evx-dot">·</span>}
            {p.href ? (
              <Link to={p.href} onClick={(e) => e.stopPropagation()} title="Open the run">
                {body}
              </Link>
            ) : (
              body
            )}
          </span>
        );
      })}
    </>
  );
}

/** `path:16-18` → [16, 18]. */
function rangeOf(fileRef: string | undefined): [number, number] | null {
  const m = fileRef && /:(\d+)(?:-(\d+))?$/.exec(fileRef);
  if (!m) return null;
  const a = Number(m[1]);
  return [a, m[2] ? Number(m[2]) : a];
}

/** A finding's excerpt with a line gutter. The finding's own lines get a wash
 *  when the excerpt shows more than them. */
export function EvCode({ card, open }: { card: StreamCard; open: boolean }) {
  const lines = (card.code ?? '').replace(/\n$/, '').split('\n');
  const start = card.fileLine;
  const range = rangeOf(card.fileRef);
  const hitOf = (n: number) => !!range && n >= range[0] && n <= range[1];
  const all = start != null && lines.every((_, i) => hitOf(start + i));
  const max = open ? lines.length : 4;
  const language = languageForPath(card.filePath);
  return (
    <div className="evx-code" role="group" aria-label="code excerpt">
      {lines.slice(0, max).map((line, i) => {
        const n = start != null ? start + i : undefined;
        return (
          <div key={i} className="evx-code-l" data-hit={n != null && !all && hitOf(n) ? '' : undefined}>
            {n != null && <span className="evx-ln">{n}</span>}
            <span className="evx-s">
              <CodeHighlight code={line.length ? line : ' '} language={language} inline />
            </span>
          </div>
        );
      })}
      {lines.length > max && <div className="evx-code-more">{lines.length - max} more lines</div>}
    </div>
  );
}

export interface EventRowProps {
  card: StreamCard;
  scope: RailScope;
  resolved: Resolved;
  /** The marker's face when it is not the card's own: `running` on a live run's newest row. */
  mark?: Mark;
  gateOutcome?: GateOutcome;
  gate?: EventGate;
  seen: boolean;
  crew?: string;
  threadUp: boolean;
  threadDn: boolean;
  open: boolean;
  arrived: boolean;
  delay?: number;
  timed: boolean;
  onToggle: (key: string) => void;
}

function EventRowInner(p: EventRowProps) {
  const { card } = p;
  const raised = isRaised(card);
  const mark = p.mark ?? gateMark(card, p.gateOutcome, p.gate?.state(card).decision ?? null) ?? markOf(card);
  const openable = raised || !!card.transcriptPath;
  const style = {
    ...(p.crew ? { '--crew': p.crew } : {}),
    ...(p.delay != null ? { '--delay': `${p.delay}ms` } : {}),
  } as CSSProperties;
  const waiting = card.form === 'await' && p.gateOutcome === 'open' && p.gate?.state(card).decision == null;
  return (
    <div
      className="evx-ev"
      data-evkey={card.key}
      data-thread={card.runId ?? card.crew ?? undefined}
      data-testid="sr-ev"
      data-accent={card.accent}
      data-kind={card.form}
      data-mark={mark}
      data-raised={raised ? '' : undefined}
      data-waiting={waiting ? '' : undefined}
      data-open={p.open ? '' : undefined}
      data-selected={p.open ? '' : undefined}
      data-arrived={p.arrived ? '' : undefined}
      data-th-up={p.threadUp ? '' : undefined}
      data-th-dn={p.threadDn ? '' : undefined}
      style={style}
    >
      <i className="evx-rl evx-rl-up evx-railx" aria-hidden />
      <i className="evx-rl evx-rl-dn evx-railx" aria-hidden />
      <i className="evx-th evx-th-up evx-railx" aria-hidden />
      <i className="evx-th evx-th-dn evx-railx" aria-hidden />
      <time className="evx-t" dateTime={card.ts ? new Date(card.ts).toISOString() : undefined} title={card.ts ? new Date(card.ts).toLocaleString() : undefined}>
        {p.timed && card.ts ? clock(card.ts) : ''}
      </time>
      <div className="evx-mk">
        <EvGlyph mark={mark} />
      </div>
      <div className="evx-c">
        {raised ? (
          <RaisedCard {...p} mark={mark} onOpen={openable ? () => p.onToggle(card.key) : undefined} />
        ) : (
          <QuietLine {...p} onOpen={openable ? () => p.onToggle(card.key) : undefined} />
        )}
      </div>
      {p.open && card.transcriptPath && (
        <div className="evx-xcerpt">
          <div className="evx-xcerpt-body">
            <Suspense fallback={<div className="evx-note">Loading the transcript…</div>}>
              <TranscriptPanel path={card.transcriptPath} runId={card.runId} live={false} embedded />
            </Suspense>
          </div>
        </div>
      )}
      <div className="evx-sel" aria-hidden />
    </div>
  );
}

/** A gate's marker: waiting, answered here (resuming / rejected), or closed by the run. */
function gateMark(card: StreamCard, outcome: GateOutcome | undefined, decided: 'approved' | 'rejected' | null): Mark | undefined {
  if (card.form !== 'await' || !outcome) return undefined;
  if (outcome === 'open') return decided === 'approved' ? 'running' : decided === 'rejected' ? 'failed' : undefined;
  return outcome === 'approved' ? 'done' : 'failed';
}

function QuietLine({ card, scope, resolved, onOpen }: EventRowProps & { onOpen?: () => void }) {
  const t = rowText(card);
  const parts = ctxOf(card, scope, resolved);
  const body = (
    <>
      <span className="evx-kw">{t.kw}</span>
      <span className="evx-hl" title={t.headline}>
        {t.headline}
      </span>
      <span className="evx-ctx">
        {t.note && (
          <>
            <span className="evx-note">{t.note}</span>
            {parts.length > 0 && <span className="evx-dot">·</span>}
          </>
        )}
        <Ctx parts={parts} />
      </span>
      {t.meta && <span className="evx-meta">{t.meta}</span>}
    </>
  );
  return onOpen ? (
    <button type="button" className="evx-line" onClick={onOpen} aria-label={`${t.headline}: show what happened`}>
      {body}
    </button>
  ) : (
    <div className="evx-line">{body}</div>
  );
}

/** "Approval needed · deploy" → "gate deploy". */
function gateTitle(card: StreamCard): string {
  return card.stepId ? `gate ${card.stepId}` : card.title;
}

function RaisedCard(p: EventRowProps & { mark: Mark; onOpen?: () => void }) {
  const { card, scope, resolved } = p;
  const parts = ctxOf(card, scope, resolved);
  const onCard = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('a, button')) return;
    p.onOpen?.();
  };
  return (
    <div className="evx-cardwrap" onClick={onCard}>
      <div className="evx-glow" aria-hidden />
      <div className="evx-bloom" aria-hidden />
      <div className="evx-lift" aria-hidden />
      <div className="evx-card evx-m" data-mark={card.form === 'finding' ? p.mark : card.form === 'await' ? 'awaiting' : 'failed'}>
        <div className="evx-flash" aria-hidden />
        {card.form === 'finding' && <FindingBody {...p} parts={parts} />}
        {card.form === 'await' && <GateBody {...p} parts={parts} />}
        {card.form === 'error' && <ErrorBody {...p} parts={parts} />}
      </div>
    </div>
  );
}

function SeenTag() {
  return (
    <span className="evx-seentag">
      <Check size={11} strokeWidth={2.25} aria-hidden />
      seen
    </span>
  );
}

function FindingBody({ card, open, seen, mark, parts }: EventRowProps & { mark: Mark; parts: CtxPart[] }) {
  const codeHref =
    card.wsId && card.filePath
      ? `/projects/${card.wsId}/code?path=${encodeURIComponent(card.filePath)}${card.fileLine ? `&line=${card.fileLine}` : ''}`
      : undefined;
  return (
    <>
      <div className="evx-head">
        <EvCap mark={mark} />
        <span className="evx-title" title={card.title}>
          {card.title}
        </span>
        {card.fileRef &&
          (codeHref ? (
            <Link className="evx-loc" to={codeHref} title={`Open ${card.fileRef} in the code viewer`}>
              {card.fileRef}
            </Link>
          ) : (
            <span className="evx-loc">{card.fileRef}</span>
          ))}
        {seen && <SeenTag />}
      </div>
      {card.detail && <div className="evx-why">{card.detail}</div>}
      {card.code && <EvCode card={card} open={open} />}
      <div className="evx-foot">
        <span className="evx-ctx">
          <Ctx parts={parts} />
        </span>
        {card.permalink && (
          <a className="evx-link" href={card.permalink} target="_blank" rel="noreferrer">
            <ExternalLink size={12} aria-hidden />
            View on repository
          </a>
        )}
      </div>
    </>
  );
}

function GateBody({ card, gate, gateOutcome, parts }: EventRowProps & { parts: CtxPart[] }) {
  const decided = gate?.state(card).decision ?? null;
  const outcome = gateOutcome ?? 'open';
  const capMark: Mark =
    outcome === 'open'
      ? decided === 'rejected'
        ? 'failed'
        : decided === 'approved'
          ? 'running'
          : 'awaiting'
      : outcome === 'approved'
        ? 'done'
        : 'failed';
  const capWord = outcome === 'open' ? (decided ?? 'needs you') : outcome;
  return (
    <>
      <div className="evx-head">
        <EvCap mark={capMark} word={capWord} />
        <span className="evx-title">{gateTitle(card)}</span>
        <span className="evx-loc">{outcome === 'open' ? (decided ? 'answered' : 'waiting') : outcome}</span>
      </div>
      {card.detail && <div className="evx-msg">{card.detail}</div>}
      {outcome === 'open' &&
        (gate ? (
          <GateButtons card={card} gate={gate} />
        ) : (
          <div className="evx-gate">
            <span className="evx-resolved">Awaiting approval</span>
          </div>
        ))}
      <div className="evx-foot">
        <span className="evx-ctx">
          <Ctx parts={parts} />
        </span>
      </div>
    </>
  );
}

function ErrorBody({ card, seen, parts }: EventRowProps & { parts: CtxPart[] }) {
  const json = card.detail ? parseErrorDetail(card.detail).json !== undefined : false;
  return (
    <>
      <div className="evx-head">
        <EvCap mark="failed" />
        <span className="evx-title" title={card.title}>
          {card.title}
        </span>
        {card.durationMs != null && <span className="evx-loc">{Math.round(card.durationMs / 1000)}s</span>}
        {seen && <SeenTag />}
      </div>
      {card.detail && (json ? <ErrorDetail text={card.detail} /> : <div className="evx-detail">{card.detail}</div>)}
      <div className="evx-foot">
        <span className="evx-ctx">
          <Ctx parts={parts} />
        </span>
      </div>
    </>
  );
}

const EventRow = memo(EventRowInner);
export default EventRow;
