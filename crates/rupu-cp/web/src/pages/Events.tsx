// Live Events — the "Situation Room" fullscreen wall. Three real-data zones:
//
//   • PulseStrip  — fleet vitals (active runs / awaiting from GET /api/dashboard,
//                   findings-by-severity from GET /api/findings, events/min +
//                   errors derived from the live stream).
//   • EventStream — center: a newest-first editorial stream MERGING the SSE +
//                   history event firehose (GET /api/events[/stream]) with the
//                   REST findings list (findings are not on the event wire).
//   • ProjectRoster — right: one card per project (GET /api/projects), status +
//                   current action folded from the stream, findings pips.
//
// Events carry only `run_id` (no project), so we resolve run_id → workspace_id
// lazily via GET /api/runs/:id and cache it. Nothing is fabricated: a source
// that fails to load degrades to zeros / "idle", never demo data.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  isKnownRunEvent,
  type DashboardResponse,
  type FindingOut,
  type FindingsSummary,
  type ProjectRow,
  type RunEvent,
} from '../lib/api';
import { type ConnectionState } from '../components/RunEventFeed';
import { cardFromEvent, cardFromFinding, type StreamCard } from '../lib/situationRoom/cards';
import { buildRoster, buildVitals, type RunActivity } from '../lib/situationRoom/roster';
import PulseStrip from '../components/situationRoom/PulseStrip';
import EventStream from '../components/situationRoom/EventStream';
import ProjectRoster from '../components/situationRoom/ProjectRoster';

const PAGE_SIZE = 200;
const MAX_EVENTS = 5000;
const AGG_POLL_MS = 15_000; // findings / projects / dashboard refresh cadence
const SPARK_TICK_MS = 5_000; // events/min sampling window
const SPARK_LEN = 16;
const FRESH_MS = 2500;

interface EventItem {
  key: string;
  ts: number;
  event: RunEvent;
}

/** Deterministic stringify (sorted keys) → stable content identity. */
function stableStringify(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(stableStringify).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).sort().map((k) => `${JSON.stringify(k)}:${stableStringify(o[k])}`).join(',')}}`;
  }
  return JSON.stringify(v);
}
/** Content identity excluding `ts`/`pos` — the SSE firehose replays a run's
 *  already-written events (different `ts`, no `pos`), so those can't be part
 *  of identity. Used for history↔live dedup and as the stable React key. */
function identityOf(ev: RunEvent): string {
  const raw = ev as Record<string, unknown>;
  const rest: Record<string, unknown> = {};
  for (const k of Object.keys(raw)) {
    if (k === 'ts' || k === 'pos') continue;
    rest[k] = raw[k];
  }
  return stableStringify(rest);
}
function tsOf(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : undefined;
}
function posOf(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.pos === 'number' ? raw.pos : undefined;
}

/** Latest state per run, folded from the (newest-first) event list — drives
 *  the roster's status + current action. */
function deriveActivity(items: EventItem[]): Map<string, RunActivity> {
  const out = new Map<string, RunActivity>();
  for (const { ts, event } of items) {
    const runId = event.run_id;
    if (out.has(runId)) continue; // first hit = newest event for this run
    let state: RunActivity['state'] = 'running';
    let action: string | undefined;
    if (isKnownRunEvent(event)) {
      switch (event.type) {
        case 'run_completed': state = 'done'; break;
        case 'run_failed': state = 'failed'; break;
        case 'step_awaiting_approval': state = 'awaiting'; action = `awaiting · ${event.step_id}`; break;
        case 'step_started': action = event.agent ? `${event.agent} · ${event.step_id}` : event.step_id; break;
        case 'step_working': action = event.note?.trim() || event.step_id; break;
        case 'step_completed': action = event.step_id; break;
        case 'panel_round': action = `${event.step_id} · round ${event.round}`; break;
        default: break;
      }
    }
    out.set(runId, { runId, state, action, ts });
  }
  return out;
}

export default function Events() {
  const [items, setItems] = useState<EventItem[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [hasMoreOlder, setHasMoreOlder] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [freshKeys, setFreshKeys] = useState<ReadonlySet<string>>(new Set());

  const [findings, setFindings] = useState<FindingOut[]>([]);
  const [findingsSummary, setFindingsSummary] = useState<FindingsSummary | null>(null);
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [dashboard, setDashboard] = useState<DashboardResponse | null>(null);
  const [runToWs, setRunToWs] = useState<Map<string, string>>(new Map());
  const [spark, setSpark] = useState<number[]>(() => Array(SPARK_LEN).fill(0));
  const [eventsPerMin, setEventsPerMin] = useState(0);

  const seenRef = useRef<Set<string>>(new Set());
  const itemsRef = useRef<EventItem[]>([]);
  itemsRef.current = items;
  const requestedRunsRef = useRef<Set<string>>(new Set());
  const sparkCounterRef = useRef(0);

  // ── initial event history (so an idle page is never empty) ──
  useEffect(() => {
    let cancelled = false;
    api.getEvents(PAGE_SIZE).then((rows) => {
      if (cancelled) return;
      seenRef.current = new Set();
      const tagged: EventItem[] = [];
      for (const ev of rows) {
        const id = identityOf(ev);
        if (seenRef.current.has(id)) continue;
        seenRef.current.add(id);
        tagged.push({ key: id, ts: tsOf(ev) ?? Date.now(), event: ev });
      }
      setItems(tagged);
      setHasMoreOlder(rows.length >= PAGE_SIZE);
    }).catch(() => {/* live SSE still populates the page */});
    return () => { cancelled = true; };
  }, []);

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
        setFreshKeys((prev) => {
          const next = new Set(prev);
          next.add(id);
          setTimeout(() => setFreshKeys((p) => { const n = new Set(p); n.delete(id); return n; }), FRESH_MS);
          return next;
        });
      },
      undefined,
      () => setConnection('reconnecting'),
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

  // ── lazy run_id → workspace_id resolution (events carry no project) ──
  useEffect(() => {
    const distinct = new Set(items.map((i) => i.event.run_id));
    const missing = [...distinct].filter((r) => !runToWs.has(r) && !requestedRunsRef.current.has(r));
    if (missing.length === 0) return;
    for (const runId of missing.slice(0, 12)) {
      requestedRunsRef.current.add(runId);
      api.getRun(runId).then((res) => {
        const ws = res.run.workspace_id;
        if (ws) setRunToWs((prev) => new Map(prev).set(runId, ws));
      }).catch(() => {/* run may be gone; leave unattributed */});
    }
  }, [items, runToWs]);

  // ── load older event history (paged) ──
  const loadOlder = useCallback(async () => {
    const cur = itemsRef.current;
    if (cur.length === 0) return;
    const oldest = cur[cur.length - 1].event;
    const oldestTs = tsOf(oldest);
    if (oldestTs == null) { setHasMoreOlder(false); return; }
    setLoadingOlder(true);
    try {
      const older = await api.getEvents(PAGE_SIZE, oldestTs, oldest.run_id, posOf(oldest));
      if (older.length === 0) { setHasMoreOlder(false); return; }
      const tagged: EventItem[] = [];
      for (const ev of older) {
        const id = identityOf(ev);
        if (seenRef.current.has(id)) continue;
        seenRef.current.add(id);
        tagged.push({ key: id, ts: tsOf(ev) ?? Date.now(), event: ev });
      }
      setItems((prev) => [...prev, ...tagged]);
      if (older.length < PAGE_SIZE) setHasMoreOlder(false);
    } catch {/* operator can scroll to retry */}
    finally { setLoadingOlder(false); }
  }, []);

  // ── derived view models ──
  const eventCards = useMemo(() => {
    const out: StreamCard[] = [];
    for (const { key, ts, event } of items) {
      const c = cardFromEvent(event, ts, key);
      if (c) out.push(c);
    }
    return out;
  }, [items]);

  const findingCards = useMemo(() => findings.map(cardFromFinding), [findings]);

  const cards = useMemo(
    () => [...eventCards, ...findingCards].sort((a, b) => b.ts - a.ts),
    [eventCards, findingCards],
  );

  const activity = useMemo(() => deriveActivity(items), [items]);
  const roster = useMemo(
    () => buildRoster(projects, runToWs, activity, findings),
    [projects, runToWs, activity, findings],
  );

  const wsById = useMemo(() => {
    const m = new Map<string, ProjectRow>();
    for (const p of projects) m.set(p.ws_id, p);
    return m;
  }, [projects]);

  const resolveProject = useCallback(
    (card: StreamCard): { label?: string; branch?: string } => {
      if (card.projectName) return { label: card.projectName };
      if (card.runId) {
        const ws = runToWs.get(card.runId);
        const p = ws ? wsById.get(ws) : undefined;
        if (p) return { label: p.name, branch: p.branch ?? undefined };
      }
      return {};
    },
    [runToWs, wsById],
  );

  const errors = useMemo(() => cards.filter((c) => c.group === 'error').length, [cards]);
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

  const onApprove = useCallback((runId: string) => api.approveRun(runId), []);
  const onReject = useCallback((runId: string) => api.rejectRun(runId, 'Rejected from Live Events'), []);

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden">
      <PulseStrip vitals={vitals} connection={connection} spark={spark} />
      <div className="flex flex-1 min-h-0">
        <EventStream
          cards={cards}
          freshKeys={freshKeys}
          resolve={resolveProject}
          onApprove={onApprove}
          onReject={onReject}
          hasMoreOlder={hasMoreOlder}
          loadingOlder={loadingOlder}
          onLoadOlder={loadOlder}
        />
        <ProjectRoster roster={roster} />
      </div>
    </div>
  );
}
