// Run detail — the live view. Owns the ONE SSE subscription for the run and
// fans the event stream down to both the RunGraph (live step status, derived
// via buildRunGraphModel) and the RunEventFeed (scrolling timeline). The
// browser twin of rupu's live TUI run view.
//
// Layout: a persistent header + an always-on graph + "Token usage by turn"
// chart (chrome that never hides), then a tab panel — Transcript · Events ·
// Findings — that FOLLOWS the step selected in the graph. Selecting a for_each
// step swaps the Transcript body for the units/transcript file-browser.
//
// REMOTE HOSTS: when ?host= is set to a non-local host id, getRunGraph and
// getRunUsageTimeline are called with the host parameter. All control/SSE
// calls also include the host param.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Archive, ArrowLeft, FileText, ListOrdered, Pause, ShieldAlert, Trash2 } from 'lucide-react';
import {
  api,
  ApiError,
  isKnownRunEvent,
  type FindingsResponse,
  type RunEvent,
  type RunGraphResponse,
  type RunRecord,
  type UsageTimelinePoint,
} from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { Button } from '../components/ui/Button';
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

/**
 * Fetch the run-graph, tolerating a transient 404 right after a launch — the
 * spawned child writes `run.json` asynchronously, so a freshly-navigated run id
 * may briefly 404 before its record lands. Retries a few times on 404 only;
 * any other error (or a final 404) propagates to the caller.
 */
async function fetchRunGraphWithRetry(
  id: string,
  isCancelled: () => boolean,
  host?: string,
): Promise<RunGraphResponse> {
  const delaysMs = [300, 600, 1000, 1500];
  for (let attempt = 0; ; attempt++) {
    try {
      return await api.getRunGraph(id, host ? { host } : undefined);
    } catch (e: unknown) {
      const is404 = e instanceof ApiError && e.status === 404;
      if (!is404 || attempt >= delaysMs.length || isCancelled()) throw e;
      await new Promise((r) => setTimeout(r, delaysMs[attempt]));
      if (isCancelled()) throw e;
    }
  }
}

/** Typed accessor for an event's `step_id` (run-level events carry none). */
function eventStepId(ev: RunEvent): string | undefined {
  const v = (ev as { step_id?: unknown }).step_id;
  return typeof v === 'string' ? v : undefined;
}

export default function RunDetail() {
  const { id = '' } = useParams<{ id: string }>();
  const [searchParams] = useSearchParams();
  const host = searchParams.get('host') ?? undefined;
  const navigate = useNavigate();

  // Full graph state (used for both local and remote runs).
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
  // Only populated for local runs (remote gating skips this fetch).
  const [series, setSeries] = useState<UsageTimelinePoint[]>([]);

  // Scoped findings for THIS run — lazy-loaded when the Findings tab is first
  // opened. `null` = not yet loaded / loading.
  const [findings, setFindings] = useState<FindingsResponse | null>(null);
  const [findingsError, setFindingsError] = useState<string | null>(null);
  const findingsRequestedRef = useRef(false);

  // Approval-gate local state. `gateDecision` reflects an optimistic local
  // decision (the live stream / next poll catches up); `gatePending` disables
  // the controls mid-request; `gateError` surfaces a failed approve/reject.
  const [gateDecision, setGateDecision] = useState<'approved' | 'rejected' | null>(null);
  const [gatePending, setGatePending] = useState(false);
  const [gateError, setGateError] = useState<string | null>(null);
  const [rejectOpen, setRejectOpen] = useState(false);
  const [rejectReason, setRejectReason] = useState('');
  // Permission mode the run resumes in once approved (worker honours it).
  const [approveMode, setApproveMode] = useState<'ask' | 'bypass' | 'readonly'>('ask');

  // Cancel local state — `cancelPending` disables the controls mid-request;
  // `cancelError` surfaces a failed cancel.
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelError, setCancelError] = useState<string | null>(null);

  // Pause local state — pausing is synchronous on this CP (the run's own
  // status flips to `paused` in the same response), so a success optimistically
  // updates `liveRunStatus` the same way Cancel does.
  const [pausePending, setPausePending] = useState(false);
  const [pauseError, setPauseError] = useState<string | null>(null);

  // Resume local state — resuming is marker-only (mirrors Approve): the run
  // STAYS `paused` until a worker picks it up and the live stream emits
  // `run_resumed`, so a success only flips `resumeRequested` (never
  // `liveRunStatus` directly). `resumeReadOnly` is set on a 501 (no launcher —
  // `rupu cp serve` isn't running); `resumeError` surfaces any other failure.
  const [resumePending, setResumePending] = useState(false);
  const [pausedResumeRequested, setPausedResumeRequested] = useState(false);
  const [resumeReadOnly, setResumeReadOnly] = useState(false);
  const [resumeError, setResumeError] = useState<string | null>(null);

  // Archive / delete local state (terminal runs only).
  const [actionPending, setActionPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  // The ONE selection cursor that the tab panel follows. Null = whole run.
  const [selection, setSelection] = useState<Selection>(null);
  // Guards the one-time default seed so live model rebuilds don't override the
  // user's manual selection on every SSE event.
  const seededSelRef = useRef(false);

  // Initial fetch: always use getRunGraph (with retry), passing the host for remote runs.
  useEffect(() => {
    let cancelled = false;
    setGraph(null);
    setLoadError(null);
    setSelection(null);
    setGateDecision(null);
    setGatePending(false);
    setGateError(null);
    setRejectOpen(false);
    setRejectReason('');
    setApproveMode('ask');
    setCancelPending(false);
    setCancelError(null);
    setPausePending(false);
    setPauseError(null);
    setResumePending(false);
    setPausedResumeRequested(false);
    setResumeReadOnly(false);
    setResumeError(null);
    setActionPending(false);
    setActionError(null);
    seededSelRef.current = false;
    setFindings(null);
    setFindingsError(null);
    findingsRequestedRef.current = false;

    fetchRunGraphWithRetry(id, () => cancelled, host)
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
  }, [id, host]);

  // Aggregated per-turn usage series — fetched for both local and remote runs.
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSeries([]);
    api
      .getRunUsageTimeline(id, host ? { host } : undefined)
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
  }, [id, host]);

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
  // Thread the host param for remote runs.
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
          else if (ev.type === 'run_paused') setLiveRunStatus('paused');
          else if (ev.type === 'run_resumed') setLiveRunStatus('running');
        }
      },
      () => setConnection('reconnecting'),
      host ? { host } : undefined,
    );
    return unsubscribe;
  }, [id, host]);

  // Plain RunEvent[] for the model builder (drop the seq wrapper).
  const rawEvents = useMemo<RunEvent[]>(() => events.map((e) => e.event), [events]);

  // Merge skeleton + checkpoints + live events into the render model. Cheap;
  // recompute on every event so the graph reflects live state.
  // Built for both local and remote runs via the host-aware graph endpoint.
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

  // The effective run record from the graph.
  const run = graph?.run ?? null;
  // Usage for the header row from the graph.
  const displayUsage = graph?.usage;

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
  // Pause is only offered while the run is actively `running` (not merely
  // `pending`, and not `awaiting_approval` — those have their own gate).
  const isPausable = effectiveStatus === 'running';
  const isPaused = effectiveStatus === 'paused';
  // Cancel is offered on any non-terminal run.
  const cancellable =
    effectiveStatus === 'running' ||
    effectiveStatus === 'pending' ||
    effectiveStatus === 'awaiting_approval';
  const cancelled = effectiveStatus === 'cancelled';

  // Approval recorded — either persisted (resume_requested_at) or optimistic.
  const resumeRequested = Boolean(run?.resume_requested_at) || gateDecision === 'approved';
  const rejected = gateDecision === 'rejected';

  async function onApprove() {
    if (!run || gatePending) return;
    setGatePending(true);
    setGateError(null);
    try {
      if (host) {
        await api.approveRun(run.id, approveMode, host);
      } else {
        await api.approveRun(run.id, approveMode);
      }
      setGateDecision('approved');
    } catch (e: unknown) {
      setGateError(e instanceof Error ? e.message : 'Failed to approve run');
    } finally {
      setGatePending(false);
    }
  }

  async function onCancel() {
    if (!run || cancelPending) return;
    if (!window.confirm('Cancel this run?')) return;
    setCancelPending(true);
    setCancelError(null);
    try {
      if (host) {
        await api.cancelRun(run.id, undefined, host);
      } else {
        await api.cancelRun(run.id);
      }
      // Optimistic — the live stream / next poll reconciles the terminal status.
      setLiveRunStatus('cancelled');
    } catch (e: unknown) {
      setCancelError(e instanceof Error ? e.message : 'Failed to cancel run');
    } finally {
      setCancelPending(false);
    }
  }

  async function onPause() {
    if (!run || pausePending) return;
    setPausePending(true);
    setPauseError(null);
    try {
      if (host) {
        await api.pauseRun(run.id, host);
      } else {
        await api.pauseRun(run.id);
      }
      // Optimistic — pausing is synchronous on this CP (the response already
      // carries `status: "paused"`); the live stream / next poll reconciles.
      setLiveRunStatus('paused');
    } catch (e: unknown) {
      setPauseError(e instanceof Error ? e.message : 'Failed to pause run');
    } finally {
      setPausePending(false);
    }
  }

  async function onResume() {
    if (!run || resumePending) return;
    setResumePending(true);
    setResumeError(null);
    setResumeReadOnly(false);
    try {
      if (host) {
        await api.resumeRun(run.id, host);
      } else {
        await api.resumeRun(run.id);
      }
      // Marker-only (mirrors Approve) — the run stays `paused` until a worker
      // picks it up and the live stream emits `run_resumed`.
      setPausedResumeRequested(true);
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setResumeReadOnly(true);
      } else {
        setResumeError(e instanceof Error ? e.message : 'Failed to resume run');
      }
    } finally {
      setResumePending(false);
    }
  }

  async function onArchive() {
    if (actionPending) return;
    setActionPending(true);
    setActionError(null);
    try {
      await api.archiveRun(id);
      navigate('/runs');
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
      setActionPending(false);
    }
  }

  async function onDelete() {
    if (actionPending) return;
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    setActionPending(true);
    setActionError(null);
    try {
      await api.deleteRun(id);
      navigate('/runs');
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
      setActionPending(false);
    }
  }

  async function onReject() {
    if (!run || gatePending) return;
    setGatePending(true);
    setGateError(null);
    try {
      if (host) {
        await api.rejectRun(run.id, rejectReason, host);
      } else {
        await api.rejectRun(run.id, rejectReason);
      }
      setGateDecision('rejected');
      setRejectOpen(false);
    } catch (e: unknown) {
      setGateError(e instanceof Error ? e.message : 'Failed to reject run');
    } finally {
      setGatePending(false);
    }
  }

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
        <div className="mt-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {loadError}
        </div>
      </div>
    );
  }

  // Loading: we need both run and model.
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
    // min-h-full (not h-full): the page grows past the viewport so the parent
    // <main> scrolls — the tall chrome (graph + usage chart) no longer squeezes
    // the tab panel into a sliver. The panel below gets its own generous height.
    <div className="flex min-h-full flex-col">
      <div className="px-8 pt-6">
        <BackLink />
        <header className="mt-3 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-3">
              <h1 className="truncate text-2xl font-semibold text-ink">{run.workflow_name}</h1>
              <StatusPill status={effectiveStatus} />
              {host && host !== 'local' && (
                <span className="rounded bg-info-bg px-1.5 py-0.5 text-note font-medium text-info ring-1 ring-info/30 font-mono">
                  {host}
                </span>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-0.5 text-note text-ink-dim">
              <span className="font-mono">{run.id}</span>
              <span>started {absoluteTime(run.started_at)}</span>
              {run.finished_at && <span>finished {absoluteTime(run.finished_at)}</span>}
            </div>
            {displayUsage && (
              <div className="mt-1.5 flex items-center gap-4 text-xs text-ink-dim tabular-nums">
                <span><span className="text-ink-mute">in</span> {formatTokens(displayUsage.input_tokens)}</span>
                <span><span className="text-ink-mute">out</span> {formatTokens(displayUsage.output_tokens)}</span>
                {displayUsage.cached_tokens > 0 && (
                  <span><span className="text-ink-mute">cached</span> {formatTokens(displayUsage.cached_tokens)}</span>
                )}
                <span><span className="text-ink-mute">total</span> {formatTokens(displayUsage.total_tokens)}</span>
                <span className="font-medium text-ink">
                  {formatCost(displayUsage.cost_usd)}{displayUsage.cost_usd !== null && !displayUsage.priced ? '*' : ''}
                </span>
              </div>
            )}
          </div>

          {isRunning && (
            <div className="flex shrink-0 flex-col items-end gap-1">
              <div className="flex items-center gap-2">
                {isPausable && (
                  <Button
                    variant="secondary"
                    onClick={() => void onPause()}
                    disabled={pausePending}
                    aria-label="Pause run"
                    className="gap-1.5"
                  >
                    <Pause size={14} /> {pausePending ? 'Pausing…' : 'Pause'}
                  </Button>
                )}
                <Button
                  variant="danger-outline"
                  onClick={onCancel}
                  disabled={cancelPending}
                  aria-label="Cancel run"
                >
                  {cancelPending ? 'Cancelling…' : 'Cancel'}
                </Button>
              </div>
              {pauseError && (
                <p className="text-note font-medium text-err" role="alert">
                  {pauseError}
                </p>
              )}
              {cancelError && (
                <p className="text-note font-medium text-err" role="alert">
                  {cancelError}
                </p>
              )}
            </div>
          )}
          {!isRunning && (
            <div className="flex shrink-0 flex-col items-end gap-1">
              <div className="flex items-center gap-2">
                <Button
                  variant="secondary"
                  onClick={() => void onArchive()}
                  disabled={actionPending}
                  className="gap-1.5"
                >
                  <Archive size={14} /> Archive
                </Button>
                <Button
                  variant="danger-outline"
                  onClick={() => void onDelete()}
                  disabled={actionPending}
                  className="gap-1.5"
                >
                  <Trash2 size={14} /> Delete
                </Button>
              </div>
              {actionError && (
                <p role="alert" className="text-ui font-medium text-err">
                  {actionError}
                </p>
              )}
            </div>
          )}
        </header>

        {run.error_message && (
          <div className="mt-3 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
            {run.error_message}
          </div>
        )}

        {awaiting && (
          <div className="mt-3 flex items-start gap-3 rounded-lg border border-warn/30 bg-warn-bg px-4 py-3">
            <Pause size={16} className="mt-0.5 shrink-0 text-warn" />
            <div className="min-w-0">
              <div className="text-sm font-medium text-warn">
                Awaiting approval · <span className="font-mono">{awaiting.stepId}</span>
              </div>
              <p className="mt-0.5 break-words text-ui text-warn">{awaiting.reason}</p>

              {cancelled ? (
                <div className="mt-2 text-ui font-medium text-ink">Cancelled.</div>
              ) : resumeRequested ? (
                <div className="mt-2 flex items-center gap-2 text-ui font-medium text-ok">
                  <span
                    className="inline-block h-2 w-2 animate-pulse rounded-full bg-ok"
                    aria-hidden="true"
                  />
                  Approved — resuming…
                </div>
              ) : rejected ? (
                <div className="mt-2 text-ui font-medium text-err">Rejected.</div>
              ) : (
                <div className="mt-2 space-y-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <label htmlFor="approve-mode" className="text-ui font-medium text-warn">
                      Resume mode
                    </label>
                    <select
                      id="approve-mode"
                      value={approveMode}
                      onChange={(e) =>
                        setApproveMode(e.target.value as 'ask' | 'bypass' | 'readonly')
                      }
                      disabled={gatePending}
                      aria-label="Resume mode"
                      className="rounded-md border border-warn/30 bg-panel px-2 py-1.5 text-ui font-medium text-ink focus:border-warn/30 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      <option value="ask">Ask</option>
                      <option value="bypass">Bypass</option>
                      <option value="readonly">Read-only</option>
                    </select>
                    <button
                      type="button"
                      onClick={onApprove}
                      disabled={gatePending}
                      aria-label="Approve run"
                      className="inline-flex items-center rounded-md bg-ok px-3 py-1.5 text-ui font-medium text-white hover:bg-ok disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      {gatePending ? 'Working…' : 'Approve'}
                    </button>
                    <Button
                      variant="danger-outline"
                      onClick={() => {
                        setRejectOpen((v) => !v);
                        setGateError(null);
                      }}
                      disabled={gatePending}
                      aria-label="Reject run"
                    >
                      Reject
                    </Button>
                    {cancellable && (
                      <Button
                        variant="secondary"
                        onClick={onCancel}
                        disabled={cancelPending}
                        aria-label="Cancel run"
                      >
                        {cancelPending ? 'Cancelling…' : 'Cancel'}
                      </Button>
                    )}
                  </div>

                  {cancelError && (
                    <p className="text-note font-medium text-err" role="alert">
                      {cancelError}
                    </p>
                  )}

                  {rejectOpen && (
                    <div className="flex flex-wrap items-center gap-2">
                      <input
                        type="text"
                        value={rejectReason}
                        onChange={(e) => setRejectReason(e.target.value)}
                        placeholder="Reason (optional)"
                        aria-label="Rejection reason"
                        className="min-w-0 flex-1 rounded-md border border-err/30 bg-panel px-2 py-1 text-ui text-ink placeholder:text-ink-mute focus:border-err/30 focus:outline-none"
                      />
                      <Button
                        variant="danger"
                        onClick={onReject}
                        disabled={gatePending}
                        aria-label="Confirm rejection"
                      >
                        {gatePending ? 'Working…' : 'Confirm reject'}
                      </Button>
                    </div>
                  )}

                  {gateError && (
                    <p className="text-note font-medium text-err" role="alert">
                      {gateError}
                    </p>
                  )}
                </div>
              )}
            </div>
          </div>
        )}

        {isPaused && (
          <div className="mt-3 flex items-start gap-3 rounded-lg border border-status-paused/30 bg-status-paused/10 px-4 py-3">
            <Pause size={16} className="mt-0.5 shrink-0 text-status-paused" />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-status-paused">Paused</div>
              <p className="mt-0.5 text-ui text-status-paused">
                This run is paused at a checkpoint. Resume to continue execution.
              </p>

              {pausedResumeRequested ? (
                <div className="mt-2 flex items-center gap-2 text-ui font-medium text-ok">
                  <span
                    className="inline-block h-2 w-2 animate-pulse rounded-full bg-ok"
                    aria-hidden="true"
                  />
                  Resume requested — resuming…
                </div>
              ) : (
                <div className="mt-2 space-y-2">
                  <button
                    type="button"
                    onClick={() => void onResume()}
                    disabled={resumePending}
                    aria-label="Resume run"
                    className="inline-flex items-center rounded-md bg-ok px-3 py-1.5 text-ui font-medium text-white hover:bg-ok disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    {resumePending ? 'Working…' : 'Resume'}
                  </button>

                  {resumeReadOnly && (
                    <div role="alert" className="rounded-lg border border-warn/30 bg-warn-bg px-3 py-2 text-ui text-warn">
                      This is a read-only deploy — resuming a run requires{' '}
                      <code className="font-mono">rupu cp serve</code>.
                    </div>
                  )}

                  {resumeError && (
                    <p className="text-note font-medium text-err" role="alert">
                      {resumeError}
                    </p>
                  )}
                </div>
              )}
            </div>
          </div>
        )}

        {/* Persistent chrome: the run graph + per-turn usage chart. */}
        <section className="mt-3" data-testid="run-graph-chrome">
          <RunGraph
            model={model!}
            positions={positions}
            onSelectNode={selectFromNode}
            onExpandFanout={selectStep}
            onOpenUnit={selectUnit}
          />
        </section>

        {/* Persistent chrome: per-turn usage chart. */}
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

      <div className="px-8 pt-2 text-note text-ink-dim">
        selected: <span className="font-mono text-ink-mute">{selectedLabel}</span>
      </div>

      {/* Definite, generous height so the transcript / events / findings panels
          have room and own their internal scroll; the whole page scrolls in the
          parent <main>. */}
      <div className="flex h-[65vh] min-h-[420px] flex-col px-8 pb-6 pt-3">
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
            ) : (
              <div className="flex h-full min-h-[120px] items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim">
                {selection
                  ? `No transcript yet for ${selection.stepId}.`
                  : 'Select a step in the graph to view its transcript.'}
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
              <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
                {findingsError}
              </div>
            ) : findings === null ? (
              <p className="text-sm text-ink-dim">Loading findings…</p>
            ) : findings.findings.length === 0 ? (
              <div className="space-y-1">
                <p className="text-sm text-ink-mute">No findings recorded for this run.</p>
                <p className="text-note text-ink-mute">
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
