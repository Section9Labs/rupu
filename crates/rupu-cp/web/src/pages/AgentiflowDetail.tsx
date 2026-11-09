// Agentiflow run detail — one scrollable page over `GET /api/agentiflows/:id`:
// header, goals, budget, the fleet of units the lead dispatched, the lead's
// transcript and the run's event log. Composed from the same chrome RunDetail
// uses (back link + header with name / CrewChip / StatusPill, `bg-panel
// border rounded-xl shadow-card` sections, TranscriptPanel for transcripts).
//
// LOCAL-only. The Flow tab is the unfolding map (components/flowmap/FlowMap).
// A `running` run re-fetches every 5 s (the lead's transcript tails live over
// SSE on its own); Refresh re-fetches on demand.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import {
  Boxes,
  CheckCircle2,
  Circle,
  FileText,
  GitFork,
  ListOrdered,
  MessagesSquare,
  Network as NetworkIcon,
  RefreshCw,
  Server,
  ShieldAlert,
  Target,
  X,
} from 'lucide-react';
import TokenFlow from '../components/charts/TokenFlow';
import { spanLabel } from '../components/run/TurnWindowBar';
import type { UsageTimelinePoint } from '../lib/api';
import WindowTranscripts from '../components/run/WindowTranscripts';
import { turnWindow, type TurnWindow } from '../lib/run/turnWindow';
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
import FlowMap from '../components/flowmap/FlowMap';
import AssetInventory from '../components/agentiflow/AssetInventory';
import ChildrenPanel from '../components/run/ChildrenPanel';
import MessageFeed from '../components/messages/MessageFeed';
import { MessageLinksContext, type MessageLinks } from '../components/transcript/messageLinks';
import { msgHash, parseMsgHash } from '../lib/messages';
import { useTabParam, withTab } from '../lib/tabParam';
import { useMessageSpace } from '../lib/useMessageSpace';
import EngagementFindings from '../components/agentiflow/EngagementFindings';
import { useFlowStop } from '../components/agentiflow/useFlowStop';
import TranscriptBrowser, { type Selection as TranscriptSelection } from '../components/agentiflow/TranscriptBrowser';
import AgentiflowEventFeed from '../components/agentiflow/AgentiflowEventFeed';
import NetflowExplorer from '../components/netflow/explorer/NetflowExplorer';
import { TabBar, TabButton } from '../components/TabBar';
import { Badge } from '../components/ui/Badge';
import { Button } from '../components/ui/Button';
import { PageHeader } from '../components/ui/PageHeader';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { EmptyState } from '../components/ui/EmptyState';
import { Spinner } from '../components/ui/Spinner';

const BACK = { label: 'Agentiflows', to: '/runs/agentiflows' };
import { parseCodename } from '../lib/codename';
import { cn } from '../lib/cn';
import { absoluteTime, durationBetween, relativeTime } from '../lib/time';
import { formatDuration } from '../lib/duration';
import { formatCost, formatTokens } from '../lib/usage';
import { shortId } from '../lib/shortId';
import { spanWindow } from '../lib/netflow';
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

// The tabs: the engagement's goals, budget and definition first, then the
// workflow run's order (Transcript, Events, Findings, Children, Messages,
// Network), plus the flow's own Fleet and Assets.
type AfTab = 'engagement' | 'transcript' | 'events' | 'findings' | 'children' | 'fleet' | 'messages' | 'network' | 'assets';
const AF_TAB_NAMES: AfTab[] = ['engagement', 'transcript', 'events', 'findings', 'children', 'fleet', 'messages', 'network', 'assets'];

export default function AgentiflowDetail() {
  const { hash, search } = useLocation();
  const navigate = useNavigate();
  const [tab, setTab] = useTabParam(AF_TAB_NAMES, parseMsgHash(hash) ? 'messages' : 'engagement');
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
  const stopControls = useFlowStop({
    id,
    running,
    coordinatorGone: detail?.record.runner_alive === false,
    liveUnits: detail?.units.filter((u) => u.status.state === 'running').length ?? 0,
    reload: () => void load(true),
  });
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => void load(true), POLL_MS);
    return () => clearInterval(t);
  }, [running, load]);

  const [childrenCount, setChildrenCount] = useState<number | null>(null);
  // The Token flow's points, and a selected range of them: a window the
  // tabs below narrow to (as on a run page).
  const usage = useFlowUsage(id, running);
  const [zoom, setZoom] = useState<[number, number] | null>(null);
  useEffect(() => setZoom(null), [id]);
  const turnWin = useMemo(() => (zoom ? turnWindow(usage ?? [], zoom[0], zoom[1]) : null), [usage, zoom]);
  const windowSpan = turnWin?.span ?? null;
  const messages = useMessageSpace({ route: 'agentiflows', id }, { live: running, load: true, reading: tab === 'messages' });
  const [transcriptFocus, setTranscriptFocus] = useState<TranscriptSelection | null>(null);
  // The transcript a participant's messages are in: a unit's own, or the
  // lead's latest round.
  const transcriptOf = useCallback(
    (address: string): TranscriptSelection | null => {
      if (!detail) return null;
      if (address === 'lead') {
        const last = detail.lead_transcripts[detail.lead_transcripts.length - 1];
        return last ? { path: last.path, live: running, title: `Lead · round ${last.round}`, sub: `lead · round ${last.round}` } : null;
      }
      const unit = detail.units.find((u) => u.participant === address);
      return unit?.transcript_path ? { path: unit.transcript_path, live: false, title: unit.codename, sub: unit.agent ?? unit.kind } : null;
    },
    [detail, running],
  );
  const messageLinks = useMemo<MessageLinks>(
    () => ({
      openInFeed: (msgId) => {
        navigate({ search: withTab(search, 'messages'), hash: msgHash(msgId) });
      },
      openParticipant: (address, msgId) => {
        const sel = transcriptOf(address);
        if (!sel) return;
        setTranscriptFocus(sel);
        navigate({ search: withTab(search, 'transcript'), hash: msgHash(msgId) });
      },
    }),
    [navigate, search, transcriptOf],
  );

  if (notFound) {
    return (
      <div className="p-8">
        <PageHeader back={BACK} title="Agentiflow" />
        <div className="mt-4">
          <EmptyState title="Agentiflow not found" hint={<span className="font-mono">{id}</span>} />
        </div>
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="p-8">
        <PageHeader back={BACK} title="Agentiflow" />
        {error ? (
          <ErrorBanner className="mt-4">{error}</ErrorBanner>
        ) : (
          <div className="mt-6">
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
        <PageHeader
          back={BACK}
          title={record.name}
          actions={
            <>
            {stopControls.buttons}
            <Button variant="secondary" onClick={() => void load(false)} className="gap-1.5">
              <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
              Refresh
            </Button>
            </>
          }
        />
        <div className="mt-2 flex flex-wrap items-center gap-3">
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
        </div>
        <EngagementSummary detail={detail} goals={goals} onOpen={() => setTab('engagement')} />
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
        {def?.description && <Objective text={def.description} />}
        {stopControls.panel && <div className="mt-3 max-w-3xl">{stopControls.panel}</div>}
      </div>

      {error && <ErrorBanner>{error}</ErrorBanner>}

      {/* The flow itself, always shown, as a workflow run shows its graph. */}
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
          <FlowMap detail={detail} running={running} flowSpace={messages} />
        )}
      </Panel>

      {/* Every model call of the lead and its units, by lead round. */}
      {usage && usage.length > 0 && (
        <section className="rounded-xl border border-border bg-panel p-4 shadow-card" data-testid="af-token-flow">
          <TokenFlow points={usage} stages live={running} zoom={zoom} onZoomChange={setZoom} />
        </section>
      )}

      {/* Tabs in the workflow run's order, on the same tab bar. */}
      <div>
        <TabBar>
          <TabButton active={tab === 'engagement'} onClick={() => setTab('engagement')} icon={Target} label="Engagement" />
          <TabButton active={tab === 'transcript'} onClick={() => setTab('transcript')} icon={FileText} label="Transcript" />
          <TabButton active={tab === 'events'} onClick={() => setTab('events')} icon={ListOrdered} label="Events" />
          <TabButton active={tab === 'findings'} onClick={() => setTab('findings')} icon={ShieldAlert} label="Findings" />
          <TabButton
            active={tab === 'children'}
            onClick={() => setTab('children')}
            icon={GitFork}
            label={childrenCount === null ? 'Children' : `Children (${childrenCount})`}
          />
          <TabButton
            active={tab === 'fleet'}
            onClick={() => setTab('fleet')}
            icon={Boxes}
            label={`Fleet (${detail.units.length})`}
          />
          <TabButton active={tab === 'messages'} onClick={() => setTab('messages')} icon={MessagesSquare} label="Messages" badge={messages.unread} />
          <TabButton active={tab === 'network'} onClick={() => setTab('network')} icon={NetworkIcon} label="Network" />
          <TabButton active={tab === 'assets'} onClick={() => setTab('assets')} icon={Server} label="Assets" />
        </TabBar>
      </div>
      {turnWin && <FlowWindowBar window={turnWin} onClear={() => setZoom(null)} />}

      {tab === 'engagement' && <EngagementTab detail={detail} goals={goals} />}

      {tab === 'fleet' && (
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
                <UnitRow key={u.unit_id} unit={u} afId={id} />
              ))}
            </ul>
          )}
        </Panel>
      )}

      {tab === 'assets' && (
        <Panel
          title="Assets"
          meta={<span className="text-note text-ink-mute">{record.engagement_profiles.join(', ') || 'engagement'}</span>}
        >
          {detail.asset_scope ? (
            <AssetInventory
              wsId={detail.asset_scope.ws_id}
              target={detail.asset_scope.target_id}
              emptyHint={
                <>
                  This engagement hasn’t recorded assets yet — a fleet records them with the{' '}
                  <span className="font-mono">assets.mark</span> tool as it discovers hosts, services, sites or routes.
                  They also appear on the{' '}
                  <Link to="/assets" className="text-brand-600 hover:underline">Assets</Link> page.
                </>
              }
            />
          ) : (
            <Muted>
              The engagement’s asset scope isn’t available yet — its lead hasn’t recorded a workspace. (Showing every
              workspace’s assets here would mix in other engagements, so this stays empty until the scope resolves.)
            </Muted>
          )}
        </Panel>
      )}

      {tab === 'children' && (
        <Panel
          title="Children"
          meta={
            <span className="text-meta text-ink-mute">
              every agent, sub-agent and workflow launched under this flow
            </span>
          }
        >
          <ChildrenPanel
            runId={id}
            fetchChildren={(sig) => api.getAgentiflowChildTree(id, sig)}
            onCount={setChildrenCount}
          />
        </Panel>
      )}

      {tab === 'findings' && (
        <Panel title="Findings" meta={<span className="text-meta text-ink-mute">verified by this engagement</span>}>
          <EngagementFindings runId={record.id} live={running} within={windowSpan} />
        </Panel>
      )}

      {tab === 'network' && (
        <Panel title="Network" meta={<span className="text-meta text-ink-mute">egress across the fleet</span>}>
          <NetflowExplorer
            key={windowSpan ? `${record.id}:${windowSpan.join('-')}` : record.id}
            scope="agentiflow"
            agentiflowId={record.id}
            initialWindow={
              windowSpan
                ? { from: new Date(windowSpan[0]).toISOString(), to: new Date(windowSpan[1]).toISOString() }
                : spanWindow(record.started_at, record.ended_at)
            }
          />
        </Panel>
      )}

      {tab === 'messages' && (
        <MessageFeed
          route="agentiflows"
          space={messages}
          notLiveReason={running ? null : `This agentiflow is ${record.status}: messages only reach a running flow.`}
          openInTranscript={(m) => (transcriptOf(m.from) ? () => messageLinks.openParticipant?.(m.from, m.id) : null)}
          highlightId={parseMsgHash(hash)}
          within={windowSpan}
        />
      )}

      {tab === 'transcript' && (
        <section>
          <MessageLinksContext.Provider value={messageLinks}>
            {turnWin ? (
              <WindowTranscripts
                window={turnWin}
                labelOf={(path) => flowTranscriptLabel(detail, path)}
                preferred={transcriptFocus?.path ?? null}
                live={running}
              />
            ) : (
              <TranscriptBrowser detail={detail} running={running} focus={transcriptFocus} />
            )}
          </MessageLinksContext.Provider>
        </section>
      )}

      {tab === 'events' && (
        <section className="flex flex-col" style={{ height: '65vh' }}>
          <AgentiflowEventFeed id={id} codename={record.codename} running={running} within={windowSpan} />
        </section>
      )}
    </div>
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

function UnitRow({ unit, afId }: { unit: AgentiflowUnit; afId: string }) {
  const { status } = unit;
  const isFailed = status.state === 'failed';
  const bodyLabel = isFailed ? 'error' : 'output';
  // Failure text is cheap and comes inline. A `done` unit's answer can be
  // several KB (a whole transcript read for a W7 unit), so the list omits it
  // and we fetch it the first time the row is expanded. A legacy unit still
  // carries its answer inline, so there is nothing to fetch.
  const inline = isFailed ? status.error : status.output;
  const lazy = !isFailed && status.state === 'done' && inline == null;
  const [fetched, setFetched] = useState<string | null | undefined>(undefined);
  const [fetching, setFetching] = useState(false);
  const [fetchErr, setFetchErr] = useState<string | null>(null);
  const loadOutput = useCallback(() => {
    if (!lazy || fetched !== undefined || fetching) return;
    setFetching(true);
    setFetchErr(null);
    api
      .getAgentiflowUnitOutput(afId, unit.unit_id)
      .then((o) => setFetched(o))
      .catch((e) => setFetchErr(apiErrorMessage(e)))
      .finally(() => setFetching(false));
  }, [lazy, fetched, fetching, afId, unit.unit_id]);
  const body = inline ?? fetched ?? null;
  // Show the expander when there is text inline, or a done unit whose answer
  // we can fetch.
  const hasBody = inline != null || lazy;
  return (
    <li className="px-4 py-2.5">
      {/* text-ui sizes the codename: AgentName sets none of its own. */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-ui">
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
      {hasBody && (
        <details
          className="mt-1.5 group"
          onToggle={(e) => {
            if (e.currentTarget.open) loadOutput();
          }}
        >
          <summary className="cursor-pointer select-none text-note text-ink-dim hover:text-ink">{bodyLabel}</summary>
          {fetching && fetched === undefined ? (
            <p className="mt-1.5 text-note text-ink-mute">Loading output…</p>
          ) : fetchErr ? (
            <p className="mt-1.5 text-note text-err">{fetchErr}</p>
          ) : body != null ? (
            <pre
              className={cn(
                'mt-1.5 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border bg-bg p-2 font-mono text-note',
                isFailed ? 'text-err' : 'text-ink',
              )}
            >
              {body}
            </pre>
          ) : (
            <p className="mt-1.5 text-note text-ink-mute">No output recorded.</p>
          )}
        </details>
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------
// The summary line, the objective, the Engagement tab, the Token flow window
// ---------------------------------------------------------------------------

type Tone = 'ok' | 'warn' | 'err' | 'plain';
const TONE_TEXT: Record<Tone, string> = { ok: 'text-ok', warn: 'text-warn', err: 'text-err', plain: 'text-ink' };

/** One line under the header: goals met, then each budget cap as used / cap,
 *  amber at the soft limit and red at or past the cap. Opens the Engagement tab. */
function EngagementSummary({ detail, goals, onOpen }: { detail: AgentiflowDetailData; goals: ReturnType<typeof goalViews>; onOpen: () => void }) {
  const { record, def } = detail;
  const uses = budgetUses(def?.budget, record);
  const met = goals.filter((g) => g.met).length;
  const items: { key: string; tone: Tone; value: string; of?: string; unit: string }[] = [];
  if (goals.length) items.push({ key: 'goals', tone: met === goals.length ? 'ok' : 'plain', value: `${met}/${goals.length}`, unit: 'goals met' });
  for (const u of uses) {
    const tone: Tone = u.over ? 'err' : u.soft ? 'warn' : 'plain';
    const unit = u.dim === 'usd' ? 'spent' : u.dim === 'wall_clock' ? 'running' : u.dim;
    items.push({ key: u.dim, tone, value: briefUse(u.dim, u.used), of: briefUse(u.dim, u.cap), unit });
  }
  if (!uses.some((u) => u.dim === 'usd')) items.push({ key: 'usd', tone: 'plain', value: formatSpendUsd(record.spent_usd), unit: 'spent' });
  if (!uses.some((u) => u.dim === 'tokens')) {
    items.push({ key: 'tok', tone: 'plain', value: formatTokens(record.spent_tokens), unit: 'tokens' });
  }
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group mt-2 inline-flex max-w-full flex-wrap items-baseline gap-x-1 gap-y-0.5 rounded-md px-1.5 py-1 -ml-1.5 text-note text-ink-dim transition-colors hover:bg-surface-hover"
      title="Goals and budget: open the Engagement tab"
      data-testid="af-summary"
    >
      {items.map((it, i) => (
        <span key={it.key} className="inline-flex items-baseline gap-1 whitespace-nowrap">
          {i > 0 && <span className="mx-1 text-ink-mute">·</span>}
          <span className={cn('font-medium tabular-nums', TONE_TEXT[it.tone])}>{it.value}</span>
          {it.of && <span className="tabular-nums text-ink-mute">of {it.of}</span>}
          <span>{it.unit}</span>
        </span>
      ))}
      <span className="ml-1 text-ink-mute opacity-0 transition-opacity group-hover:opacity-100">details →</span>
    </button>
  );
}

/** A budget figure as short as it reads: `$766.94`, `$100k`, `4h 22m`, `120h`. */
function briefUse(dim: BudgetUse['dim'], n: number): string {
  if (dim === 'usd') return n >= 1000 ? `$${Number((n / 1000).toFixed(n >= 10_000 ? 0 : 1))}k` : formatCost(n);
  const text = formatUse(dim, n);
  return dim === 'wall_clock' && text.endsWith(' 0m') ? text.slice(0, -3) : text;
}

/** The objective, two lines until asked for more. */
function Objective({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-2 max-w-3xl text-sm text-ink-dim">
      <p className={cn(!open && 'line-clamp-2')}>{text}</p>
      <button type="button" onClick={() => setOpen((v) => !v)} className="mt-0.5 text-note font-medium text-ink-mute hover:text-ink">
        {open ? 'less' : 'more'}
      </button>
    </div>
  );
}

/** Goals and budget, and what the engagement was given to work with. */
function EngagementTab({ detail, goals }: { detail: AgentiflowDetailData; goals: ReturnType<typeof goalViews> }) {
  const { record, def } = detail;
  return (
    <div className="grid gap-4 lg:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
      <Panel title="Goals" meta={<span className="text-note tabular-nums text-ink-dim">{goals.filter((g) => g.met).length}/{goals.length} met</span>}>
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
      <div className="space-y-4">
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
        <Panel title="Definition">
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-note">
            <dt className="text-ink-mute">lead</dt>
            <dd className="font-mono text-ink">{def?.lead ?? '—'}</dd>
            <dt className="text-ink-mute">profiles</dt>
            <dd className="text-ink">{record.engagement_profiles.join(', ') || 'none'}</dd>
            <dt className="text-ink-mute">pool</dt>
            <dd className="text-ink">
              {def ? `${def.pool.agents.length} agents · workflows ${def.pool.workflows === 'all' ? 'all' : def.pool.workflows.length}` : '—'}
            </dd>
            <dt className="text-ink-mute">lead turns</dt>
            <dd className="tabular-nums text-ink">{def?.round?.lead_max_turns ?? '—'} per round</dd>
          </dl>
        </Panel>
      </div>
    </div>
  );
}

/** The flow's per-call token series (lead + units), re-read while it runs.
 *  `null` until the first answer; a failed read keeps the last one. */
function useFlowUsage(id: string, running: boolean): UsageTimelinePoint[] | null {
  const [points, setPoints] = useState<UsageTimelinePoint[] | null>(null);
  useEffect(() => {
    setPoints(null);
  }, [id]);
  useEffect(() => {
    let alive = true;
    const ctl = new AbortController();
    const read = () =>
      api.getAgentiflowUsageTimeline(id, { signal: ctl.signal }).then(
        (p) => alive && setPoints(p),
        () => alive && setPoints((cur) => cur ?? []),
      );
    void read();
    const t = running ? setInterval(() => void read(), POLL_MS) : undefined;
    return () => {
      alive = false;
      ctl.abort();
      if (t) clearInterval(t);
    };
  }, [id, running]);
  return points;
}

/** What a transcript in the window is: a lead round's, or a unit's codename. */
function flowTranscriptLabel(detail: AgentiflowDetailData, path: string): string {
  const lead = detail.lead_transcripts.find((t) => t.path === path);
  if (lead) return `lead · round ${lead.round}`;
  const unit = detail.units.find((u) => u.transcript_path === path);
  if (unit) return parseCodename(unit.codename).leaf;
  const base = path.split('/').pop() ?? path;
  return base.replace(/\.jsonl$/, '');
}

/** The window a Token flow selection opens: which calls, when, and what each
 *  tab narrows to, with a Clear. */
function FlowWindowBar({ window: w, onClear }: { window: TurnWindow; onClear: () => void }) {
  const rounds = w.labels.map((l) => l.replace(/^round /, ''));
  return (
    <div
      role="status"
      data-testid="turn-window"
      className="gx-window flex flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-brand-500/30 bg-brand-500/5 px-3 py-1.5 text-note text-ink-dim"
    >
      <span className="font-medium text-ink">
        Turns {w.turns[0].toLocaleString()}–{w.turns[1].toLocaleString()}
      </span>
      {w.span && <span className="tabular-nums">{spanLabel(w.span)}</span>}
      {rounds.length > 0 && (
        <span>
          {rounds.length === 1 ? 'round' : 'rounds'} <span className="font-mono text-ink">{rounds.join(', ')}</span>
        </span>
      )}
      <span className="text-ink-mute">
        {w.span
          ? 'transcript: these turns · events, findings, messages, network: this time'
          : 'no times recorded for these turns · transcript: these turns'}
      </span>
      <button
        type="button"
        onClick={onClear}
        className="ml-auto inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-medium text-ink transition-colors hover:bg-surface-hover"
      >
        <X size={12} aria-hidden />
        Clear
      </button>
    </div>
  );
}
