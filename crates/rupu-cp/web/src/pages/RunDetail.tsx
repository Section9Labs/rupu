// Run detail — the live view. Owns the ONE SSE subscription for the run and
// fans the event stream down to both the RunGraph (live step status, derived
// via buildRunGraphModel) and the RunEventFeed (scrolling timeline). The
// browser twin of rupu's live TUI run view.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, GitBranch, ListOrdered, Pause } from 'lucide-react';
import {
  api,
  isKnownRunEvent,
  type RunEvent,
  type RunGraphResponse,
  type RunRecord,
} from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { TabBar, TabButton } from '../components/TabBar';
import RunGraph, { type NodeSelection } from '../components/RunGraph';
import FanoutDrill from '../components/FanoutDrill';
import RunEventFeed, { type ConnectionState, type SeqEvent } from '../components/RunEventFeed';
import TranscriptPanel from '../components/TranscriptPanel';
import RunUsageTimeline from '../components/charts/RunUsageTimeline';
import { buildTurnSeries, type TurnUsagePoint } from '../components/transcript/turnSeries';
import { buildRunGraphModel, type RunGraphModel, type UnitView } from '../lib/runGraphModel';
import { layoutGraph, type Pos } from '../lib/graphLayout';
import { absoluteTime } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';

const MAX_EVENTS = 2000;

type Tab = 'graph' | 'events';

/**
 * Default transcript selection on (re)load: prefer a running node's transcript,
 * else the last completed step that recorded one. Returns null when nothing
 * with a transcript exists yet.
 */
function defaultSelection(model: RunGraphModel): NodeSelection | null {
  const running = model.nodes.find((n) => n.state === 'running' && n.transcriptPath);
  if (running) {
    return { path: running.transcriptPath ?? null, live: true, label: running.id };
  }
  for (let i = model.nodes.length - 1; i >= 0; i--) {
    const n = model.nodes[i];
    if (n.transcriptPath) {
      return { path: n.transcriptPath, live: false, label: n.id };
    }
  }
  return null;
}

export default function RunDetail() {
  const { id = '' } = useParams<{ id: string }>();

  const [graph, setGraph] = useState<RunGraphResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('graph');

  // Live state, fed by the single SSE subscription below.
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  // The latest run status as overridden by run_completed / run_failed events.
  const [liveRunStatus, setLiveRunStatus] = useState<RunRecord['status'] | null>(null);
  // Monotonically-increasing sequence counter — stable key source for SeqEvent.
  const seqRef = useRef<number>(0);

  // Fan-out drill-in: which step's units are open, or null.
  const [drillStepId, setDrillStepId] = useState<string | null>(null);

  // Per-turn token series for the "Token usage by turn" timeline. Sourced from
  // the run's primary transcript (see effect below).
  const [series, setSeries] = useState<TurnUsagePoint[]>([]);

  // Selected transcript for the bottom split pane. Null until a node is clicked
  // or the default is seeded once the model first becomes available.
  const [sel, setSel] = useState<NodeSelection | null>(null);
  // Guards the one-time default seed so live model rebuilds don't override the
  // user's manual selection on every SSE event.
  const seededSelRef = useRef(false);

  // Initial fetch of the run-graph (run record + workflow DAG + checkpoints).
  useEffect(() => {
    let cancelled = false;
    setGraph(null);
    setLoadError(null);
    setSel(null);
    seededSelRef.current = false;
    api
      .getRunGraph(id)
      .then((res) => {
        if (cancelled) return;
        setGraph(res);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setLoadError(e instanceof Error ? e.message : 'Failed to load run');
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Per-turn usage series. The graph response has no transcript events, so we
  // fetch the run's PRIMARY transcript once and build the series from it.
  // v1 limitation: "primary" = the first step result that recorded a transcript
  // path. For a single-agent run that's the whole run; for a multi-step workflow
  // it's only the first step's turns. A future iteration could merge all steps'
  // series into one timeline.
  useEffect(() => {
    let cancelled = false;
    setSeries([]);
    const path = graph?.step_results.find((s) => s.transcript_path)?.transcript_path;
    if (!path) return;
    api
      .getTranscript(path)
      .then((r) => {
        if (cancelled) return;
        setSeries(buildTurnSeries(r.events));
      })
      .catch(() => {
        if (cancelled) return;
        setSeries([]);
      });
    return () => {
      cancelled = true;
    };
  }, [graph]);

  // ONE SSE subscription per open run — shared by graph + feed.
  useEffect(() => {
    if (!id) return;
    setEvents([]);
    seqRef.current = 0;
    setLiveRunStatus(null);
    setConnection('connecting');

    const unsubscribe = api.subscribeRunLog(
      id,
      (ev) => {
        setConnection('live');
        const seq = ++seqRef.current;
        setEvents((prev) => {
          const next = prev.length >= MAX_EVENTS ? prev.slice(prev.length - MAX_EVENTS + 1) : prev;
          return [...next, { seq, event: ev }];
        });
        if (isKnownRunEvent(ev)) {
          if (ev.type === 'run_completed') setLiveRunStatus(ev.status);
          else if (ev.type === 'run_failed') setLiveRunStatus('failed');
        }
      },
      () => setConnection('reconnecting'),
    );
    return unsubscribe;
  }, [id]);

  // Plain RunEvent[] for the model builder (drop the seq wrapper).
  const rawEvents = useMemo<RunEvent[]>(() => events.map((e) => e.event), [events]);

  // Merge skeleton + checkpoints + live events into the render model. Cheap;
  // recompute on every event so the graph reflects live state.
  const model = useMemo(
    () => (graph ? buildRunGraphModel(graph, rawEvents) : null),
    [graph, rawEvents],
  );

  // Seed the default transcript selection ONCE, the first time the model
  // resolves a node that has a transcript. After that the user (or a click)
  // owns the selection — we never auto-override it on later live rebuilds.
  useEffect(() => {
    if (seededSelRef.current || !model) return;
    const initial = defaultSelection(model);
    if (initial) {
      setSel(initial);
      seededSelRef.current = true;
    }
  }, [model]);

  // Stable identity of the node-id SET. Layout is expensive (dagre), so we
  // recompute it ONLY when the set of node ids changes — not on every event.
  const nodeIdsKey = useMemo(() => {
    if (!model) return '';
    return model.nodes
      .map((n) => n.id)
      .sort()
      .join('\u0000');
  }, [model]);

  // Keyed on the node-id SET, NOT the model, so dagre only re-runs when nodes
  // are added/removed — never on a per-event status change. `model` is read
  // inside but intentionally omitted from deps; nodeIdsKey is its proxy.
  const positions = useMemo<Map<string, Pos>>(() => {
    if (!model) return new Map<string, Pos>();
    return layoutGraph(model);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodeIdsKey]);

  const run = graph?.run ?? null;

  // Live awaiting info: prefer a live awaiting node from the model, else the
  // persisted record.
  const awaiting = useMemo(() => {
    if (model) {
      const node = model.nodes.find((n) => n.state === 'awaiting_approval');
      if (node) {
        return { stepId: node.id, reason: run?.approval_prompt ?? 'Awaiting approval' };
      }
    }
    return run?.awaiting_step_id
      ? { stepId: run.awaiting_step_id, reason: run.approval_prompt ?? 'Awaiting approval' }
      : undefined;
  }, [model, run]);

  const effectiveStatus = liveRunStatus ?? run?.status ?? 'pending';

  // Units for the open drill, pulled live from the model.
  const drillUnits = useMemo<UnitView[]>(() => {
    if (!model || !drillStepId) return [];
    return model.nodeById(drillStepId)?.fanout?.units ?? [];
  }, [model, drillStepId]);

  if (loadError) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {loadError}
        </div>
      </div>
    );
  }

  if (!run || !model) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading run…</div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="px-8 pt-6">
        <BackLink />
        <header className="mt-3 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-3">
              <h1 className="truncate text-2xl font-semibold text-ink">{run.workflow_name}</h1>
              <StatusPill status={effectiveStatus} />
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-0.5 text-[11px] text-ink-dim">
              <span className="font-mono">{run.id}</span>
              <span>started {absoluteTime(run.started_at)}</span>
              {run.finished_at && <span>finished {absoluteTime(run.finished_at)}</span>}
            </div>
            {graph?.usage && (
              <div className="mt-1.5 flex items-center gap-4 text-xs text-ink-dim tabular-nums">
                <span><span className="text-ink-mute">in</span> {formatTokens(graph.usage.input_tokens)}</span>
                <span><span className="text-ink-mute">out</span> {formatTokens(graph.usage.output_tokens)}</span>
                {graph.usage.cached_tokens > 0 && (
                  <span><span className="text-ink-mute">cached</span> {formatTokens(graph.usage.cached_tokens)}</span>
                )}
                <span><span className="text-ink-mute">total</span> {formatTokens(graph.usage.total_tokens)}</span>
                <span className="font-medium text-ink">
                  {formatCost(graph.usage.cost_usd)}{graph.usage.cost_usd !== null && !graph.usage.priced ? '*' : ''}
                </span>
              </div>
            )}
          </div>
        </header>

        {run.error_message && (
          <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
            {run.error_message}
          </div>
        )}

        {awaiting && (
          <div className="mt-3 flex items-start gap-3 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3">
            <Pause size={16} className="mt-0.5 shrink-0 text-amber-600" />
            <div className="min-w-0">
              <div className="text-sm font-medium text-amber-800">
                Awaiting approval · <span className="font-mono">{awaiting.stepId}</span>
              </div>
              <p className="mt-0.5 break-words text-[12px] text-amber-700">{awaiting.reason}</p>
              <p className="mt-1 text-[11px] text-amber-600/80">
                Approve / Reject controls arrive in a later phase — view only for now.
              </p>
            </div>
          </div>
        )}

        <section className="mt-3 bg-panel border border-border rounded-xl shadow-card px-4 py-3">
          <h2 className="text-xs font-semibold text-ink-dim uppercase tracking-wide mb-2">
            Token usage by turn
          </h2>
          <RunUsageTimeline series={series} />
        </section>
      </div>

      <div className="mt-4">
        <TabBar>
          <TabButton active={tab === 'graph'} onClick={() => setTab('graph')} icon={GitBranch} label="Graph" />
          <TabButton active={tab === 'events'} onClick={() => setTab('events')} icon={ListOrdered} label="Events" />
        </TabBar>
      </div>

      <div className="min-h-0 flex-1 px-8 py-6">
        {tab === 'graph' ? (
          <div className="flex h-full min-h-0 flex-col gap-4">
            {/* Top pane: the run graph (~55%). */}
            <div className="min-h-0 flex-[55] overflow-auto">
              <RunGraph
                model={model}
                positions={positions}
                onOpenUnit={(stepId) => setDrillStepId(stepId)}
                onExpandFanout={(stepId) => setDrillStepId(stepId)}
                onSelectNode={setSel}
              />
            </div>
            {/* Bottom pane: the selected node's transcript (~45%, scrolls). */}
            <div className="min-h-0 flex-[45] overflow-auto">
              {sel?.path ? (
                <TranscriptPanel key={sel.path} path={sel.path} live={sel.live} />
              ) : (
                <div className="flex h-full min-h-[120px] items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim">
                  {sel
                    ? `No transcript yet for ${sel.label}.`
                    : 'Select a step or unit to view its transcript.'}
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="h-full min-h-0">
            <RunEventFeed events={events} connection={connection} />
          </div>
        )}
      </div>

      {drillStepId && (
        <FanoutDrill
          stepId={drillStepId}
          units={drillUnits}
          onClose={() => setDrillStepId(null)}
          onSelectUnit={setSel}
        />
      )}
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/runs"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Runs
    </Link>
  );
}
