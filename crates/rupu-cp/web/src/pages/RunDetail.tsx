// Run detail — the live view. Owns the ONE SSE subscription for the run and
// fans the event stream down to both the RunGraph (live step status, derived
// via buildRunGraphModel) and the RunEventFeed (scrolling timeline). The
// browser twin of rupu's live TUI run view.
//
// Layout: a persistent header + an always-on graph + "Token usage by turn"
// chart (chrome that never hides), then a tab panel — Transcript · Events ·
// Findings — that FOLLOWS the step selected in the graph. Selecting a for_each
// step swaps the Transcript body for the units/transcript file-browser.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, FileText, ListOrdered, Pause, ShieldAlert } from 'lucide-react';
import {
  api,
  isKnownRunEvent,
  type FindingsResponse,
  type RunEvent,
  type RunGraphResponse,
  type RunRecord,
  type UsageTimelinePoint,
} from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { TabBar, TabButton } from '../components/TabBar';
import { ListCard } from '../components/lists/ListCard';
import { FindingMetrics } from '../components/findings/FindingMetrics';
import { FindingRow } from '../components/findings/FindingRow';
import RunGraph, { type NodeSelection } from '../components/RunGraph';
import RunEventFeed, { type ConnectionState, type SeqEvent } from '../components/RunEventFeed';
import TranscriptPanel from '../components/TranscriptPanel';
import StepTranscriptBrowser from '../components/run/StepTranscriptBrowser';
import RunUsageTimeline from '../components/charts/RunUsageTimeline';
import { buildRunGraphModel, type GraphNode, type RunGraphModel } from '../lib/runGraphModel';
import { layoutGraph, type Pos } from '../lib/graphLayout';
import { absoluteTime } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';

const MAX_EVENTS = 2000;

type Tab = 'transcript' | 'events' | 'findings';

/**
 * A single selection cursor that the whole tab panel follows. `unitIndex` is an
 * optional hint for a for_each unit (the file-browser owns its own unit
 * selection, so this is informational); `null` means "whole run".
 */
type Selection = { stepId: string; unitIndex?: number } | null;

/** A for_each node with a populated fan-out (units list). */
function fanoutOf(node: GraphNode | undefined): GraphNode['fanout'] | null {
  if (!node) return null;
  if (node.kind === 'for_each' || node.fanout) return node.fanout ?? null;
  return null;
}

/**
 * Default step selection on (re)load: prefer a running node, else the last
 * completed step that recorded a transcript. Returns null when nothing
 * selectable exists yet (→ "whole run").
 */
function defaultSelection(model: RunGraphModel): Selection {
  const running = model.nodes.find((n) => n.state === 'running' && n.transcriptPath);
  if (running) return { stepId: running.id };
  for (let i = model.nodes.length - 1; i >= 0; i--) {
    const n = model.nodes[i];
    if (n.transcriptPath || n.fanout) return { stepId: n.id };
  }
  return null;
}

/** Typed accessor for an event's `step_id` (run-level events carry none). */
function eventStepId(ev: RunEvent): string | undefined {
  const v = (ev as { step_id?: unknown }).step_id;
  return typeof v === 'string' ? v : undefined;
}

export default function RunDetail() {
  const { id = '' } = useParams<{ id: string }>();

  const [graph, setGraph] = useState<RunGraphResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('transcript');

  // Live state, fed by the single SSE subscription below.
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  // The latest run status as overridden by run_completed / run_failed events.
  const [liveRunStatus, setLiveRunStatus] = useState<RunRecord['status'] | null>(null);
  // Monotonically-increasing sequence counter — stable key source for SeqEvent.
  const seqRef = useRef<number>(0);

  // Aggregated per-turn token series for the "Token usage by turn" timeline —
  // a global index across all of the run's steps (see effect below).
  const [series, setSeries] = useState<UsageTimelinePoint[]>([]);

  // Scoped findings for THIS run — lazy-loaded when the Findings tab is first
  // opened. `null` = not yet loaded / loading.
  const [findings, setFindings] = useState<FindingsResponse | null>(null);
  const [findingsError, setFindingsError] = useState<string | null>(null);
  const findingsRequestedRef = useRef(false);

  // The ONE selection cursor that the tab panel follows. Null = whole run.
  const [selection, setSelection] = useState<Selection>(null);
  // Guards the one-time default seed so live model rebuilds don't override the
  // user's manual selection on every SSE event.
  const seededSelRef = useRef(false);

  // Initial fetch of the run-graph (run record + workflow DAG + checkpoints).
  useEffect(() => {
    let cancelled = false;
    setGraph(null);
    setLoadError(null);
    setSelection(null);
    seededSelRef.current = false;
    setFindings(null);
    setFindingsError(null);
    findingsRequestedRef.current = false;
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

  // Aggregated per-turn usage series across all of the run's steps, served by
  // the backend's usage-timeline endpoint. One fetch per run id.
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSeries([]);
    api
      .getRunUsageTimeline(id)
      .then((pts) => {
        if (cancelled) return;
        setSeries(pts);
      })
      .catch(() => {
        if (cancelled) return;
        setSeries([]);
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Lazy-load this run's findings the first time the Findings tab is opened.
  // Keyed on (id, tab); the ref guard ensures a single fetch per run id.
  useEffect(() => {
    if (!id || tab !== 'findings' || findingsRequestedRef.current) return;
    findingsRequestedRef.current = true;
    let cancelled = false;
    setFindings(null);
    setFindingsError(null);
    api
      .getFindings({ runId: id })
      .then((res) => {
        if (cancelled) return;
        setFindings(res);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setFindingsError(e instanceof Error ? e.message : 'Failed to load findings');
      });
    return () => {
      cancelled = true;
    };
  }, [id, tab]);

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

  // Seed the default selection ONCE, the first time the model resolves a
  // selectable node. After that the user (or a click) owns the selection — we
  // never auto-override it on later live rebuilds.
  useEffect(() => {
    if (seededSelRef.current || !model) return;
    const initial = defaultSelection(model);
    if (initial) {
      setSelection(initial);
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
      .join('\0');
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
  const isRunning = effectiveStatus === 'running' || effectiveStatus === 'pending';

  // ---- Selection-driven derivations (safe when model is null) --------------

  // The selected node, its fan-out (if any), and its resolved transcript path.
  const selectedNode = useMemo<GraphNode | null>(
    () => (model && selection ? model.nodeById(selection.stepId) ?? null : null),
    [model, selection],
  );
  const selectedFanout = useMemo(() => fanoutOf(selectedNode ?? undefined), [selectedNode]);
  const selectedTranscriptPath = selectedNode?.transcriptPath ?? null;

  // Events filtered to the selected step (run-level events, which carry no
  // step_id, drop out naturally); the whole feed when nothing is selected.
  const feedEvents = useMemo<SeqEvent[]>(() => {
    if (!selection) return events;
    return events.filter((e) => eventStepId(e.event) === selection.stepId);
  }, [events, selection]);

  // RunGraph wiring → selection cursor. A normal step's NodeSelection.label is
  // its node id (== step id); for_each expand/open-unit pass the step id (and
  // optionally a unit index) directly.
  //
  // A unit-square click fires BOTH onOpenUnit(stepId, index) and a spurious
  // onSelectNode({ label: unit.key }) from RunGraph. Guard against the latter:
  // ignore any label that does not resolve to a real graph node, so the unit
  // key (e.g. 'a.rs') is a no-op and onOpenUnit's correct cursor stands.
  function selectFromNode(sel: NodeSelection) {
    if (!model || !model.nodeById(sel.label)) return;
    setSelection({ stepId: sel.label });
  }
  function selectStep(stepId: string) {
    setSelection({ stepId });
  }
  function selectUnit(stepId: string, index: number) {
    setSelection({ stepId, unitIndex: index });
  }

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

  const findingsCount = findings?.findings.length ?? 0;
  const selectedLabel = selection ? selection.stepId : 'whole run';

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

        {/* Persistent chrome: the run graph — always visible, drives selection. */}
        <section className="mt-3" data-testid="run-graph-chrome">
          <RunGraph
            model={model}
            positions={positions}
            onSelectNode={selectFromNode}
            onExpandFanout={selectStep}
            onOpenUnit={selectUnit}
          />
        </section>

        {/* Persistent chrome: per-turn usage chart — always visible. */}
        <section
          className="mt-3 bg-panel border border-border rounded-xl shadow-card px-4 py-3"
          data-testid="usage-timeline-chrome"
        >
          <h2 className="text-xs font-semibold text-ink-dim uppercase tracking-wide mb-2">
            Token usage by turn
          </h2>
          <RunUsageTimeline series={series} separators />
        </section>
      </div>

      <div className="mt-4">
        <TabBar>
          <TabButton active={tab === 'transcript'} onClick={() => setTab('transcript')} icon={FileText} label="Transcript" />
          <TabButton active={tab === 'events'} onClick={() => setTab('events')} icon={ListOrdered} label="Events" />
          <TabButton active={tab === 'findings'} onClick={() => setTab('findings')} icon={ShieldAlert} label={findingsCount > 0 ? `Findings (${findingsCount})` : 'Findings'} />
        </TabBar>
      </div>

      <div className="px-8 pt-2 text-[11px] text-ink-dim">
        selected: <span className="font-mono text-ink-mute">{selectedLabel}</span>
      </div>

      <div className="min-h-0 flex-1 px-8 pb-6 pt-3">
        {tab === 'transcript' && (
          <div className="flex h-full min-h-0 flex-col overflow-auto">
            {selection && selectedFanout ? (
              <StepTranscriptBrowser
                stepId={selection.stepId}
                units={selectedFanout.units}
                initialUnitIndex={selection.unitIndex}
              />
            ) : selection && selectedTranscriptPath ? (
              <TranscriptPanel key={selectedTranscriptPath} path={selectedTranscriptPath} live={isRunning} />
            ) : selection ? (
              <div className="flex h-full min-h-[120px] items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim">
                No transcript yet for {selection.stepId}.
              </div>
            ) : (
              <div className="flex h-full min-h-[120px] items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim">
                Select a step in the graph to view its transcript.
              </div>
            )}
          </div>
        )}
        {tab === 'events' && (
          <div className="h-full min-h-0">
            <RunEventFeed events={feedEvents} connection={connection} />
          </div>
        )}
        {tab === 'findings' && (
          <div className="h-full min-h-0 overflow-auto">
            {findingsError ? (
              <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
                {findingsError}
              </div>
            ) : findings === null ? (
              <p className="text-sm text-ink-dim">Loading findings…</p>
            ) : findings.findings.length === 0 ? (
              <div className="space-y-1">
                <p className="text-sm text-ink-mute">No findings recorded for this run.</p>
                <p className="text-[11px] text-ink-mute">
                  (findings require a workflow with a coverage target)
                </p>
              </div>
            ) : (
              <div className="space-y-4">
                <FindingMetrics summary={findings.summary} />
                <ListCard>
                  {findings.findings.map((f) => (
                    <FindingRow
                      key={`${f.target_id}/${f.id}`}
                      finding={f}
                      project={f.project}
                      targetId={f.target_id}
                    />
                  ))}
                </ListCard>
              </div>
            )}
          </div>
        )}
      </div>
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
