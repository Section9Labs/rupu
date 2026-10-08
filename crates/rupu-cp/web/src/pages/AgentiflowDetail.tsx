// Agentiflow run detail — one scrollable page over `GET /api/agentiflows/:id`:
// header, goals, budget, the fleet of units the lead dispatched, the lead's
// transcript and the run's event log. Composed from the same chrome RunDetail
// uses (back link + header with name / CrewChip / StatusPill, `bg-panel
// border rounded-xl shadow-card` sections, TranscriptPanel for transcripts).
//
// v1 is LOCAL-only and graph-free: the data is surfaced as panels; the
// branching graph is a later pass. A `running` run re-fetches every 5 s (the
// lead's transcript tails live over SSE on its own); Refresh re-fetches on demand.

import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, CheckCircle2, Circle, RefreshCw } from 'lucide-react';
import {
  api,
  apiErrorMessage,
  ApiError,
  type AgentiflowDetail as AgentiflowDetailData,
  type AgentiflowUnit,
} from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { CrewChip } from '../components/codename/CrewChip';
import { AgentName } from '../components/codename/AgentName';
import AgentiflowGraph from '../components/agentiflow/AgentiflowGraph';
import AssetInventory from '../components/agentiflow/AssetInventory';
import MessageFeed from '../components/agentiflow/MessageFeed';
import EngagementFindings from '../components/agentiflow/EngagementFindings';
import TranscriptBrowser from '../components/agentiflow/TranscriptBrowser';
import AgentiflowEventFeed from '../components/agentiflow/AgentiflowEventFeed';
import { Segmented } from '../components/ui/Segmented';
import { Badge } from '../components/ui/Badge';
import { Button } from '../components/ui/Button';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { EmptyState } from '../components/ui/EmptyState';
import { Spinner } from '../components/ui/Spinner';
import { parseCodename } from '../lib/codename';
import { cn } from '../lib/cn';
import { absoluteTime, durationBetween, relativeTime } from '../lib/time';
import { formatDuration } from '../lib/duration';
import { formatCost, formatTokens } from '../lib/usage';
import { shortId } from '../lib/shortId';
import {
  STOP_TONE_BADGE,
  STOP_TONE_TEXT,
  agentiflowPillStatus,
  budgetStateTone,
  budgetUses,
  formatSpendTokens,
  formatSpendUsd,
  goalProgress,
  goalViews,
  stopReasonTone,
  unitPillStatus,
  type BudgetUse,
} from '../lib/agentiflow';

const POLL_MS = 5000;

type AfTab = 'flow' | 'assets' | 'findings' | 'messages' | 'transcript' | 'events';
const TABS: { value: AfTab; label: string }[] = [
  { value: 'flow', label: 'Flow' },
  { value: 'assets', label: 'Assets' },
  { value: 'findings', label: 'Findings' },
  { value: 'messages', label: 'Messages' },
  { value: 'transcript', label: 'Transcript' },
  { value: 'events', label: 'Events' },
];

export default function AgentiflowDetail() {
  const [tab, setTab] = useState<AfTab>('flow');
  const { id = '' } = useParams<{ id: string }>();
  const [detail, setDetail] = useState<AgentiflowDetailData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [loading, setLoading] = useState(true);
  // The newest load wins (a poll and a Refresh can overlap).
  const seq = useRef(0);

  const load = useCallback(
    (silent: boolean) => {
      const mine = ++seq.current;
      if (!silent) setLoading(true);
      return api
        .getAgentiflow(id)
        .then((d) => {
          if (seq.current !== mine) return;
          setDetail(d);
          setError(null);
          setNotFound(false);
        })
        .catch((e: unknown) => {
          if (seq.current !== mine) return;
          if (e instanceof ApiError && e.status === 404) setNotFound(true);
          else setError(apiErrorMessage(e));
        })
        .finally(() => {
          if (seq.current === mine) setLoading(false);
        });
    },
    [id],
  );

  useEffect(() => {
    setDetail(null);
    setNotFound(false);
    void load(false);
    return () => {
      seq.current++; // drop any in-flight result on unmount / id change
    };
  }, [load]);

  const running = detail?.record.status === 'running';
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => void load(true), POLL_MS);
    return () => clearInterval(t);
  }, [running, load]);

  if (notFound) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4">
          <EmptyState title="Agentiflow not found" hint={<span className="font-mono">{id}</span>} />
        </div>
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="p-8">
        <BackLink />
        {error ? (
          <ErrorBanner className="mt-4">{error}</ErrorBanner>
        ) : (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading agentiflow…" />
          </div>
        )}
      </div>
    );
  }

  const { record, def } = detail;
  const goals = goalViews(record, def);

  return (
    <div className="p-8 space-y-4">
      <div>
        <BackLink />
        <header className="mt-3 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-3">
              <h1 className="truncate text-2xl font-semibold text-ink">{record.name}</h1>
              <CrewChip crew={parseCodename(record.codename).crew} derived={record.codename_derived} />
              <StatusPill status={agentiflowPillStatus(record.status)} />
              {record.engagement_profiles.map((p) => (
                <Badge key={p} tone="violet">
                  {p}
                </Badge>
              ))}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-0.5 text-note text-ink-dim">
              <span className="font-mono">{record.id}</span>
              <span title={absoluteTime(record.started_at)}>started {relativeTime(record.started_at)}</span>
              {record.ended_at && (
                <span title={absoluteTime(record.ended_at)}>
                  ended {relativeTime(record.ended_at)} · ran {durationBetween(record.started_at, record.ended_at)}
                </span>
              )}
              <span>
                {record.rounds} {record.rounds === 1 ? 'round' : 'rounds'}
              </span>
              <span className="inline-flex items-center gap-1.5 font-medium text-ink tabular-nums">
                {formatSpendUsd(record.spent_usd)}
                <span className="font-normal text-ink-mute">· {formatSpendTokens(record.spent_tokens)}</span>
              </span>
            </div>
            {record.stop_reason && (
              <div className="mt-1.5 text-note text-ink-dim">
                <span className="text-ink-mute">stopped: </span>
                <span className={cn('break-words font-mono', STOP_TONE_TEXT[stopReasonTone(record.stop_reason)])}>
                  {record.stop_reason}
                </span>
              </div>
            )}
            {record.status === 'running' && record.runner_alive === false && (
              <div
                role="status"
                className="mt-2 rounded-md border border-warn/30 bg-warn-bg px-3 py-1.5 text-note text-warn"
              >
                The coordinator process (pid {record.runner_pid ?? '?'}) is no longer running — this run died without
                finalizing and will be closed out by the orphan reaper.
              </div>
            )}
            {def?.description && <p className="mt-2 max-w-3xl text-sm text-ink-dim">{def.description}</p>}
          </div>
          <Button variant="secondary" onClick={() => void load(false)} className="shrink-0 gap-1.5">
            <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
            Refresh
          </Button>
        </header>
      </div>

      {error && <ErrorBanner>{error}</ErrorBanner>}

      {/* Always-visible status: goals + budget. */}
      <div className="grid gap-4 lg:grid-cols-2">
        <Panel
          title="Goals"
          meta={
            <span className="text-note tabular-nums text-ink-dim">
              {record.goals.filter((g) => g.met).length}/{record.goals.length} met
            </span>
          }
        >
          {goals.length === 0 ? (
            <Muted>No goals recorded.</Muted>
          ) : (
            <ul className="divide-y divide-border">
              {goals.map((g) => (
                <li key={g.id} className="flex items-start gap-3 py-2.5 first:pt-0 last:pb-0">
                  {g.met ? (
                    <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-ok" aria-label="met" />
                  ) : (
                    <Circle size={16} className="mt-0.5 shrink-0 text-ink-mute" aria-label="not met" />
                  )}
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium text-ink">{g.objective}</span>
                      {g.objective !== g.id && <span className="font-mono text-note text-ink-mute">{g.id}</span>}
                      {!g.required && <Badge tone="neutral">optional</Badge>}
                    </div>
                    {g.predicate && (
                      <div className="mt-0.5 break-words font-mono text-note text-ink-mute">{g.predicate}</div>
                    )}
                    <div className="mt-1.5 flex items-center gap-3">
                      <ProgressBar value={goalProgress(g)} tone={g.met ? 'ok' : 'brand'} label={`${g.id} progress`} />
                      <span className="shrink-0 text-note tabular-nums text-ink-dim">
                        {g.current}/{g.target}
                      </span>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          title="Budget"
          meta={
            detail.budget_state ? (
              <Badge tone={STOP_TONE_BADGE[budgetStateTone(detail.budget_state)]} ring>
                {detail.budget_state}
              </Badge>
            ) : (
              <span className="text-note text-ink-mute">not evaluated yet</span>
            )
          }
        >
          <BudgetBody detail={detail} />
        </Panel>
      </div>

      {/* Profile-aware tabs — the same shape workflow runs + projects use. */}
      <div className="flex flex-wrap items-center gap-3">
        <Segmented options={TABS} value={tab} onChange={(v) => setTab(v as AfTab)} ariaLabel="Agentiflow view" />
        <span className="text-meta text-ink-mute">
          profile{record.engagement_profiles.length === 1 ? '' : 's'}: {record.engagement_profiles.join(', ') || 'none'}
        </span>
      </div>

      {tab === 'flow' && (
        <>
          <Panel
            title="Flow"
            meta={
              <span className="text-note tabular-nums text-ink-dim">
                {record.rounds} {record.rounds === 1 ? 'round' : 'rounds'} · {detail.units.length} units
              </span>
            }
          >
            {detail.events.length === 0 && detail.units.length === 0 ? (
              <Muted>Nothing has happened yet.</Muted>
            ) : (
              <AgentiflowGraph detail={detail} />
            )}
          </Panel>

          <Panel
        title="Fleet"
        meta={<span className="text-note tabular-nums text-ink-dim">{detail.units.length} units</span>}
        flush
      >
        {detail.units.length === 0 ? (
          <div className="px-4 pb-3">
            <Muted>No units dispatched yet.</Muted>
          </div>
        ) : (
          <ul className="divide-y divide-border border-t border-border">
            {detail.units.map((u) => (
              <UnitRow key={u.unit_id} unit={u} />
            ))}
          </ul>
        )}
      </Panel>
        </>
      )}

      {tab === 'assets' && (
        <Panel
          title="Assets"
          meta={<span className="text-note text-ink-mute">{record.engagement_profiles.join(', ') || 'engagement'}</span>}
        >
          <AssetInventory
            emptyHint={
              <>
                This engagement hasn’t recorded assets yet — a fleet records them with the{' '}
                <span className="font-mono">assets.mark</span> tool as it discovers hosts, services, sites or routes. They
                also appear on the <Link to="/assets" className="text-brand-600 hover:underline">Assets</Link> page.
              </>
            }
          />
        </Panel>
      )}

      {tab === 'findings' && (
        <Panel title="Findings" meta={<span className="text-meta text-ink-mute">verified by this engagement</span>}>
          <EngagementFindings runId={record.id} live={running} />
        </Panel>
      )}

      {tab === 'messages' && (
        <Panel title="Board" meta={<span className="text-meta text-ink-mute">how the fleet talks to itself</span>}>
          <MessageFeed id={id} live={running} />
        </Panel>
      )}

      {tab === 'transcript' && (
        <section>
          <TranscriptBrowser detail={detail} running={running} />
        </section>
      )}

      {tab === 'events' && (
        <section className="flex flex-col" style={{ height: '65vh' }}>
          <AgentiflowEventFeed id={id} codename={record.codename} running={running} />
        </section>
      )}
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/runs/agentiflows"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Agentiflows
    </Link>
  );
}

// ---------------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------------

/** The standard CP panel: `bg-panel border rounded-xl shadow-card`, a small
 *  uppercase title (the RunDetail chrome-section heading) and an optional
 *  right-hand meta slot. `flush` drops the body padding for divided lists. */
function Panel({
  title,
  meta,
  flush = false,
  children,
}: {
  title: string;
  meta?: ReactNode;
  flush?: boolean;
  children: ReactNode;
}) {
  return (
    <section className="rounded-xl border border-border bg-panel shadow-card">
      <div className="flex items-center justify-between gap-3 px-4 pb-2 pt-3">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-ink-dim">{title}</h2>
        {meta}
      </div>
      <div className={cn(!flush && 'px-4 pb-3')}>{children}</div>
    </section>
  );
}

function Muted({ children }: { children: ReactNode }) {
  return <p className="text-sm text-ink-mute">{children}</p>;
}

type BarTone = 'brand' | 'ok' | 'warn' | 'err';
const BAR_FILL: Record<BarTone, string> = {
  brand: 'bg-brand-500',
  ok: 'bg-ok',
  warn: 'bg-warn',
  err: 'bg-err',
};

/** A thin progress bar on the panel's `surface` track. `value` is 0..1. */
function ProgressBar({ value, tone, label }: { value: number; tone: BarTone; label: string }) {
  const pct = Math.round(Math.min(1, Math.max(0, value)) * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct}
      className="h-1.5 flex-1 overflow-hidden rounded-full bg-surface"
    >
      <div className={cn('h-full rounded-full', BAR_FILL[tone])} style={{ width: `${pct}%` }} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

function formatUse(dim: BudgetUse['dim'], n: number): string {
  switch (dim) {
    case 'usd':
      return formatCost(n);
    case 'tokens':
      return formatTokens(n);
    case 'wall_clock':
      return formatDuration(n);
    case 'rounds':
      return String(n);
  }
}

function BudgetBody({ detail }: { detail: AgentiflowDetailData }) {
  const { record, def } = detail;
  // Recomputed each render, so a running run's wall-clock bar advances on every poll.
  const uses = budgetUses(def?.budget, record);
  return (
    <div className="space-y-3">
      {uses.map((u) => (
        <div key={u.dim}>
          <div className="mb-1 flex items-baseline justify-between gap-3 text-note">
            <span className="text-ink-dim">{u.label}</span>
            <span className={cn('tabular-nums', u.over ? 'font-medium text-err' : u.soft ? 'text-warn' : 'text-ink-dim')}>
              {formatUse(u.dim, u.used)} <span className="text-ink-mute">/ {formatUse(u.dim, u.cap)}</span>
            </span>
          </div>
          <div className="flex">
            <ProgressBar value={u.ratio} tone={u.over ? 'err' : u.soft ? 'warn' : 'brand'} label={`${u.label} used`} />
          </div>
        </div>
      ))}
      {uses.length === 0 && (
        <Muted>{def ? 'The definition sets no budget caps.' : 'Definition snapshot unavailable — no caps to compare against.'}</Muted>
      )}
      <div className="flex items-baseline justify-between gap-3 border-t border-border pt-2 text-note text-ink-dim">
        <span>spent</span>
        <span className="tabular-nums">
          <span className="font-medium text-ink">{formatSpendUsd(record.spent_usd)}</span>
          <span className="text-ink-mute"> · {formatSpendTokens(record.spent_tokens)}</span>
        </span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Fleet
// ---------------------------------------------------------------------------

function UnitRow({ unit }: { unit: AgentiflowUnit }) {
  const { status } = unit;
  // A unit's final answer (`done`) or failure (`failed`) — can be several KB,
  // so collapsed until asked for.
  const body = status.state === 'failed' ? status.error : status.output;
  const bodyLabel = status.state === 'failed' ? 'error' : 'output';
  return (
    <li className="px-4 py-2.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <AgentName
          codename={unit.codename}
          agent={unit.agent ?? undefined}
          derived={unit.codename_derived}
          showCrew
        />
        <Badge tone="neutral" ring>
          {unit.kind}
        </Badge>
        <StatusPill status={unitPillStatus(unit)} />
        {unit.participant && <span className="font-mono text-note text-ink-mute">{unit.participant}</span>}
        <span className="ml-auto inline-flex items-center gap-3 text-note text-ink-mute">
          <span className="font-mono" title={unit.unit_id}>
            {shortId(unit.unit_id)}
          </span>
          {unit.started_at && <span title={absoluteTime(unit.started_at)}>{relativeTime(unit.started_at)}</span>}
          {unit.transcript_path && (
            <Link
              to={`/transcript?path=${encodeURIComponent(unit.transcript_path)}&live=0`}
              className="font-medium text-brand-600 hover:text-brand-700 hover:underline"
            >
              transcript
            </Link>
          )}
        </span>
      </div>
      {body && (
        <details className="mt-1.5 group">
          <summary className="cursor-pointer select-none text-note text-ink-dim hover:text-ink">{bodyLabel}</summary>
          <pre
            className={cn(
              'mt-1.5 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border bg-bg p-2 font-mono text-note',
              status.state === 'failed' ? 'text-err' : 'text-ink',
            )}
          >
            {body}
          </pre>
        </details>
      )}
    </li>
  );
}

