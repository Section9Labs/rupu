// Live Events — the "Situation Room" fullscreen wall. Three real-data zones:
//
//   • PulseStrip  — fleet vitals (active runs / awaiting from GET /api/dashboard,
//                   findings-by-severity from GET /api/findings, events/min +
//                   errors derived from the live stream).
//   • EventStream — center: the event rail (components/events), newest first,
//                   MERGING the SSE + history event firehose (GET
//                   /api/events[/stream]) with the REST findings list
//                   (findings are not on the event wire), under the Needs-you
//                   deck (gates answered here, failures and severe findings
//                   until seen).
//   • ProjectRoster — right: one card per project (GET /api/projects), status +
//                   current action folded from the stream, findings pips.
//
// Live events carry only `run_id` (no project), so we resolve run_id →
// workspace_id lazily via GET /api/runs/:id and cache it; history rows come
// with their run's workflow, project and codename. Nothing is fabricated: a
// source that fails to load degrades to zeros / "idle", never demo data.
//
// The stream filters through the query line (`lib/query/lists/events.ts`).
// The server filters the history it pages (`GET /api/events?q=`), so a query
// reaches events the unfiltered pages never loaded; the page applies the
// same query to everything it holds (live frames, the unfiltered history,
// findings). The roster and pulse always read the unfiltered stream.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  type DashboardResponse,
  type FindingOut,
  type FindingsSummary,
  type ProjectRow,
  type RunEvent,
} from '../lib/api';
import { type ConnectionState } from '../components/RunEventFeed';
import { agentUnitIndex, cardFromEvent, cardFromFinding, unitKeyIndex, type StreamCard } from '../lib/situationRoom/cards';
import { buildRoster, buildVitals, deriveActivity, reconcileActivity } from '../lib/situationRoom/roster';
import PulseStrip from '../components/situationRoom/PulseStrip';
import EventStream from '../components/situationRoom/EventStream';
import ProjectRoster from '../components/situationRoom/ProjectRoster';
import { UnscopedNote } from '../components/customers/UnscopedNote';
import { ListQueryBar, useFilteredRows, useListQuery } from '../components/query/ListQueryBar';
import { EVENT_FIELDS, EVENT_HISTORY_NOTE, eventRow, findingRow, type EventRow } from '../lib/query/lists/events';
import { identityOf } from '../lib/events/identity';
import type { GateDecision } from '../components/graph/GateActions';
import { IDLE_GATE, type EventGate } from '../components/events/GateButtons';
import { parseQuery } from '../lib/findingQuery/grammar';

const PAGE_SIZE = 200;
const MAX_EVENTS = 5000;
const AGG_POLL_MS = 15_000; // findings / projects / dashboard refresh cadence
const SPARK_TICK_MS = 5_000; // events/min sampling window
const SPARK_LEN = 16;
const STREAM_FINDINGS_CAP = 60; // most-recent findings merged into the stream
const STALE_RUN_MS = 2 * 60_000; // silent this long + still live-looking → re-check run.json
const STALE_RECHECK_MS = 60_000; // per-run floor between those re-checks

interface EventItem {
  key: string;
  ts: number;
  event: RunEvent;
}

/** History rows as items, skipping any `seen` already holds (and adding the
 *  rest to it). */
function tagRows(rows: RunEvent[], seen: Set<string>): EventItem[] {
  const out: EventItem[] = [];
  for (const ev of rows) {
    const id = identityOf(ev);
    if (seen.has(id)) continue;
    seen.add(id);
    out.push({ key: id, ts: tsOf(ev) ?? Date.now(), event: ev });
  }
  return out;
}
function tsOf(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : undefined;
}
function posOf(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.pos === 'number' ? raw.pos : undefined;
}

/** One gate: its run and step. */
function gateKey(c: StreamCard): string {
  return `${c.approvable?.runId ?? c.runId ?? ''}\u0000${c.approvable?.stepId ?? c.stepId ?? ''}`;
}

/** Persisted `run.json` statuses that mean the run is over — used to stop
 *  the lazy `getRun` resolution from re-checking a run that already ended. */
const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled', 'rejected']);

export default function Events() {
  const [items, setItems] = useState<EventItem[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [hasMoreOlder, setHasMoreOlder] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);

  const [findings, setFindings] = useState<FindingOut[]>([]);
  const [findingsSummary, setFindingsSummary] = useState<FindingsSummary | null>(null);
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [dashboard, setDashboard] = useState<DashboardResponse | null>(null);
  const [runToWs, setRunToWs] = useState<Map<string, string>>(new Map());
  // run_id → workflow_name, learned from the same lazy getRun call as the
  // workspace. Lets each event surface which workflow it belongs to.
  const [runToWorkflow, setRunToWorkflow] = useState<Map<string, string>>(new Map());
  // run_id → run codename (server-minted, e.g. `jade-reef`), from the same
  // getRun — gives run-level cards (started/awaiting/completed) their crew.
  const [runToCodename, setRunToCodename] = useState<Map<string, string>>(new Map());
  // Authoritative *terminal* run.json statuses (completed/failed/cancelled/
  // rejected), learned via the same lazy getRun calls — reconciles runs whose
  // event log ended mid-step so they don't spin forever.
  const [runStatus, setRunStatus] = useState<Map<string, string>>(new Map());
  const [spark, setSpark] = useState<number[]>(() => Array(SPARK_LEN).fill(0));
  const [eventsPerMin, setEventsPerMin] = useState(0);

  const seenRef = useRef<Set<string>>(new Set());
  const itemsRef = useRef<EventItem[]>([]);
  itemsRef.current = items;
  const requestedRunsRef = useRef<Set<string>>(new Set());
  const statusCheckedRef = useRef<Map<string, number>>(new Map());
  const sparkCounterRef = useRef(0);

  // ── initial event history (so an idle page is never empty) ──
  useEffect(() => {
    let cancelled = false;
    api.getEvents(PAGE_SIZE).then((rows) => {
      if (cancelled) return;
      seenRef.current = new Set();
      setItems(tagRows(rows, seenRef.current));
      setHasMoreOlder(rows.length >= PAGE_SIZE);
    }).catch(() => {/* live SSE still populates the page */});
    return () => { cancelled = true; };
  }, []);

  // ── the query, and the history the server filters by it ──
  const [q, setQ] = useListQuery();
  const query = useMemo(() => (q.trim() && parseQuery(q, EVENT_FIELDS).ok ? q : null), [q]);
  const queryRef = useRef(query);
  queryRef.current = query;
  const [qItems, setQItems] = useState<EventItem[]>([]);
  const qItemsRef = useRef<EventItem[]>([]);
  qItemsRef.current = qItems;
  const qSeenRef = useRef<Set<string>>(new Set());
  const [qHasMoreOlder, setQHasMoreOlder] = useState(false);
  const [qError, setQError] = useState<string | null>(null);
  useEffect(() => {
    qSeenRef.current = new Set();
    setQItems([]);
    setQHasMoreOlder(false);
    setQError(null);
    if (query === null) return;
    let cancelled = false;
    api.getEvents(PAGE_SIZE, undefined, undefined, undefined, query).then((rows) => {
      if (cancelled) return;
      setQItems(tagRows(rows, qSeenRef.current));
      setQHasMoreOlder(rows.length >= PAGE_SIZE);
    }).catch((e: unknown) => {
      if (!cancelled) setQError(e instanceof Error ? e.message : String(e));
    });
    return () => { cancelled = true; };
  }, [query]);

  // ── live SSE firehose ──
  useEffect(() => {
    setConnection('connecting');
    return api.subscribeEvents(
      (ev) => {
        setConnection('live');
        const id = identityOf(ev);
        if (seenRef.current.has(id)) return;
        seenRef.current.add(id);
        sparkCounterRef.current += 1;
        const item: EventItem = { key: id, ts: tsOf(ev) ?? Date.now(), event: ev };
        setItems((prev) => [item, ...prev].slice(0, MAX_EVENTS));
      },
      undefined,
      () => setConnection('reconnecting'),
      // `open` — not the first frame — is the honest "live" signal: a
      // healthy stream with no runs emitting stays idle indefinitely, and
      // before this the pill sat on "Connecting" against a perfectly good
      // connection until an event happened to arrive.
      () => setConnection('live'),
    );
  }, []);

  // ── aggregate polling: findings, projects, dashboard ──
  useEffect(() => {
    let cancelled = false;
    const load = () => {
      api.getFindings().then((r) => {
        if (cancelled) return;
        setFindings(r.findings);
        setFindingsSummary(r.summary);
      }).catch(() => {});
      api.getProjects().then((p) => { if (!cancelled) setProjects(p); }).catch(() => {});
      api.getDashboard().then((d) => { if (!cancelled) setDashboard(d); }).catch(() => {});
    };
    load();
    const t = setInterval(load, AGG_POLL_MS);
    return () => { cancelled = true; clearInterval(t); };
  }, []);

  // ── events/min sampling + sparkline ──
  useEffect(() => {
    const t = setInterval(() => {
      const n = sparkCounterRef.current;
      sparkCounterRef.current = 0;
      setSpark((prev) => [...prev.slice(1), n]);
      setEventsPerMin(Math.round((n * 60_000) / SPARK_TICK_MS));
    }, SPARK_TICK_MS);
    return () => clearInterval(t);
  }, []);

  // ── lazy run_id → workspace_id + terminal-status resolution ──
  // Events carry no project, so each run is resolved once via GET
  // /api/runs/:id. The same response's `status` (previously discarded) is
  // kept when terminal: a run whose events.jsonl ended mid-step — every
  // cancel before the store appended terminal events, or a crashed runner —
  // never tells the stream it ended, so the persisted run.json status is
  // the only closing signal. Runs that still look live after going silent
  // are re-checked; `dashboard` is in the deps purely as a poll tick (it
  // refreshes every AGG_POLL_MS) so the re-check fires even with no new
  // events arriving.
  useEffect(() => {
    const now = Date.now();
    const newestTs = new Map<string, number>();
    for (const i of items) {
      if (!newestTs.has(i.event.run_id)) newestTs.set(i.event.run_id, i.ts); // items are newest-first
    }
    const fetchRun = (runId: string) => {
      api.getRun(runId).then((res) => {
        const ws = res.run.workspace_id;
        if (ws) setRunToWs((prev) => new Map(prev).set(runId, ws));
        const wf = res.run.workflow_name;
        if (typeof wf === 'string' && wf && !wf.startsWith('agent:')) {
          setRunToWorkflow((prev) => new Map(prev).set(runId, wf));
        }
        const codename = res.run.codename;
        if (typeof codename === 'string' && codename) {
          setRunToCodename((prev) => new Map(prev).set(runId, codename));
        }
        const status = res.run.status;
        if (typeof status === 'string' && TERMINAL_STATUSES.has(status)) {
          setRunStatus((prev) => new Map(prev).set(runId, status));
        }
      }).catch(() => {/* run may be gone; leave unattributed */});
    };
    let budget = 12;
    for (const runId of newestTs.keys()) {
      if (budget === 0) return;
      if (runToWs.has(runId) || requestedRunsRef.current.has(runId)) continue;
      requestedRunsRef.current.add(runId);
      statusCheckedRef.current.set(runId, now);
      fetchRun(runId);
      budget -= 1;
    }
    for (const [runId, ts] of newestTs) {
      if (budget === 0) return;
      if (runStatus.has(runId)) continue; // already known-terminal
      if (now - ts < STALE_RUN_MS) continue; // still chatty — events will close it
      const checked = statusCheckedRef.current.get(runId) ?? 0;
      if (now - checked < STALE_RECHECK_MS) continue;
      statusCheckedRef.current.set(runId, now);
      fetchRun(runId);
      budget -= 1;
    }
  }, [items, runToWs, runStatus, dashboard]);

  // ── load older event history (paged; the query's own pages under one) ──
  const loadOlder = useCallback(async () => {
    const q = queryRef.current;
    const cur = q === null ? itemsRef.current : qItemsRef.current;
    const setMore = q === null ? setHasMoreOlder : setQHasMoreOlder;
    if (cur.length === 0) return;
    const oldest = cur[cur.length - 1].event;
    const oldestTs = tsOf(oldest);
    if (oldestTs == null) { setMore(false); return; }
    setLoadingOlder(true);
    try {
      const older = await api.getEvents(PAGE_SIZE, oldestTs, oldest.run_id, posOf(oldest), q ?? undefined);
      if (q !== queryRef.current) return;
      if (older.length === 0) { setMore(false); return; }
      if (q === null) {
        const tagged = tagRows(older, seenRef.current);
        setItems((prev) => [...prev, ...tagged]);
      } else {
        const tagged = tagRows(older, qSeenRef.current);
        setQItems((prev) => [...prev, ...tagged]);
      }
      if (older.length < PAGE_SIZE) setMore(false);
    } catch {/* operator can scroll to retry */}
    finally { setLoadingOlder(false); }
  }, []);

  // ── derived view models ──
  const wsById = useMemo(() => {
    const m = new Map<string, ProjectRow>();
    for (const p of projects) m.set(p.ws_id, p);
    return m;
  }, [projects]);

  // What history rows said about their runs, for the live frames of the
  // same runs (which carry no attribution of their own).
  const historyAttribution = useMemo(() => {
    const m = new Map<string, { project?: string; workflow?: string; runCodename?: string }>();
    for (const { event } of [...items, ...qItems]) {
      if (m.has(event.run_id)) continue;
      const raw = event as Record<string, unknown>;
      const str = (v: unknown) => (typeof v === 'string' && v !== '' ? v : undefined);
      const a = { project: str(raw.project), workflow: str(raw.workflow), runCodename: str(raw.run_codename) };
      if (a.project || a.workflow || a.runCodename) m.set(event.run_id, a);
    }
    return m;
  }, [items, qItems]);

  // Every loaded event's unit_key, so a card the query keeps is labelled as
  // fully as the same card unfiltered.
  const unitKeys = useMemo(() => unitKeyIndex([...items, ...qItems].map((i) => i.event)), [items, qItems]);

  const rowsOf = useCallback(
    (list: EventItem[]): EventRow[] => {
      const out: EventRow[] = [];
      const ctx = { crewByRun: runToCodename, unitKeys, agentUnits: agentUnitIndex(list.map((i) => i.event)) };
      for (const { key, ts, event } of list) {
        const c = cardFromEvent(event, ts, key, ctx);
        if (!c) continue;
        const run = c.runId;
        const ws = run ? runToWs.get(run) : undefined;
        const learned = run ? historyAttribution.get(run) : undefined;
        out.push(
          eventRow(c, event, {
            project: learned?.project ?? (ws ? wsById.get(ws)?.name : undefined),
            workflow: learned?.workflow ?? (run ? runToWorkflow.get(run) : undefined),
            runCodename: learned?.runCodename ?? (run ? runToCodename.get(run) : undefined),
          }),
        );
      }
      return out;
    },
    [runToCodename, unitKeys, runToWs, runToWorkflow, wsById, historyAttribution],
  );

  const eventRows = useMemo(() => rowsOf(items), [rowsOf, items]);
  // Under a query: the server's matches merged with everything held.
  const streamEventRows = useMemo(() => {
    if (query === null) return eventRows;
    const fromServer = new Set(qItems.map((i) => i.key));
    return [...rowsOf(qItems), ...eventRows.filter((r) => !fromServer.has(r.card.key))];
  }, [query, eventRows, rowsOf, qItems]);

  const findingsByDate = useMemo(
    () => [...findings].sort((a, b) => Date.parse(b.declared_at) - Date.parse(a.declared_at)).map(cardFromFinding),
    [findings],
  );
  // The stream shows the most-recent findings only (getFindings can return
  // hundreds across all projects); a query searches every one. The roster +
  // pulse count the full set either way.
  const findingRows = useMemo(
    () => (query === null ? findingsByDate.slice(0, STREAM_FINDINGS_CAP) : findingsByDate).map(findingRow),
    [query, findingsByDate],
  );

  const allRows = useMemo(() => [...streamEventRows, ...findingRows], [streamEventRows, findingRows]);
  const { rows: matched, error: queryError, facets } = useFilteredRows(allRows, q, EVENT_FIELDS);
  const cards = useMemo(() => matched.map((r) => r.card).sort((a, b) => b.ts - a.ts), [matched]);

  const activity = useMemo(
    () => reconcileActivity(deriveActivity(items), runStatus),
    [items, runStatus],
  );
  const roster = useMemo(
    () => buildRoster(projects, runToWs, activity, findings),
    [projects, runToWs, activity, findings],
  );

  const resolveProject = useCallback(
    (card: StreamCard): { project?: string; workflow?: string } => {
      const workflow = card.workflow ?? (card.runId ? runToWorkflow.get(card.runId) : undefined);
      if (card.projectName) return { project: card.projectName, workflow };
      if (card.runId) {
        const ws = runToWs.get(card.runId);
        const p = ws ? wsById.get(ws) : undefined;
        if (p) return { project: p.name, workflow };
      }
      return { workflow };
    },
    [runToWs, runToWorkflow, wsById],
  );

  const errors = useMemo(() => eventRows.filter((r) => r.card.group === 'error').length, [eventRows]);
  const projectsLiveCount = useMemo(() => roster.filter((r) => r.status !== 'idle').length, [roster]);

  const vitals = useMemo(
    () => buildVitals({
      activeRuns: dashboard?.active.running,
      awaiting: dashboard?.active.awaiting_approval,
      findings: findingsSummary,
      projectsLive: projectsLiveCount,
      projectsTotal: projects.length,
      errors,
      eventsPerMin,
    }),
    [dashboard, findingsSummary, projectsLiveCount, projects.length, errors, eventsPerMin],
  );

  // Gate answers given here: pending-state, never optimistic. A decision
  // stays "approved · resuming…" until the run's own events close the gate.
  const [gateStates, setGateStates] = useState<ReadonlyMap<string, GateDecision>>(new Map());
  const decide = useCallback((card: StreamCard, decision: 'approved' | 'rejected') => {
    const runId = card.approvable?.runId ?? card.runId;
    if (!runId) return;
    const key = gateKey(card);
    const stepId = card.approvable?.stepId ?? card.stepId;
    setGateStates((m) => new Map(m).set(key, { pending: true, decision: null, error: null }));
    const req = decision === 'approved'
      ? api.approveRun(runId, undefined, undefined, stepId)
      : api.rejectRun(runId, 'Rejected from Live Events', undefined, stepId);
    req.then(
      () => setGateStates((m) => new Map(m).set(key, { pending: false, decision, error: null })),
      (e: unknown) =>
        setGateStates((m) =>
          new Map(m).set(key, { pending: false, decision: null, error: e instanceof Error ? e.message : String(e) }),
        ),
    );
  }, []);
  const gate = useMemo<EventGate>(
    () => ({
      state: (c) => gateStates.get(gateKey(c)) ?? IDLE_GATE,
      approve: (c) => decide(c, 'approved'),
      reject: (c) => decide(c, 'rejected'),
    }),
    [gateStates, decide],
  );
  const terminal = useCallback((runId: string) => runStatus.get(runId), [runStatus]);
  const isRunLive = useCallback((runId: string) => activity.get(runId)?.state === 'running', [activity]);
  const liveRuns = useMemo(() => [...activity.values()].filter((a) => a.state === 'running').length, [activity]);

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden">
      <PulseStrip vitals={vitals} connection={connection} spark={spark} />
      <UnscopedNote className="mx-4 mt-2 shrink-0">Live events cover every customer.</UnscopedNote>
      <div className="flex flex-1 min-h-0">
        <EventStream
          cards={cards}
          filtered={q.trim() !== ''}
          queryBar={
            <ListQueryBar
              className="mb-0"
              value={q}
              onChange={setQ}
              fields={EVENT_FIELDS}
              facets={facets}
              error={queryError}
              alerts={[
                ...(query !== null ? [EVENT_HISTORY_NOTE] : []),
                ...(qError ? [`The filtered history couldn't load: ${qError}`] : []),
              ]}
              label="Filter events"
              placeholder="Filter events… e.g. group:error workflow:nightly · agent:recon since:1h"
            />
          }
          resolve={resolveProject}
          gate={gate}
          terminal={terminal}
          isRunLive={isRunLive}
          liveRuns={liveRuns}
          hasMoreOlder={query === null ? hasMoreOlder : qHasMoreOlder}
          loadingOlder={loadingOlder}
          onLoadOlder={loadOlder}
        />
        <ProjectRoster roster={roster} />
      </div>
    </div>
  );
}
