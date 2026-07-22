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
  type DashboardResponse,
  type FindingOut,
  type FindingsSummary,
  type ProjectRow,
  type RunEvent,
} from '../lib/api';
import { type ConnectionState } from '../components/RunEventFeed';
import { cardFromEvent, cardFromFinding, type StreamCard } from '../lib/situationRoom/cards';
import { buildRoster, buildVitals, deriveActivity, reconcileActivity } from '../lib/situationRoom/roster';
import PulseStrip from '../components/situationRoom/PulseStrip';
import EventStream from '../components/situationRoom/EventStream';
import ProjectRoster from '../components/situationRoom/ProjectRoster';

const PAGE_SIZE = 200;
const MAX_EVENTS = 5000;
const AGG_POLL_MS = 15_000; // findings / projects / dashboard refresh cadence
const SPARK_TICK_MS = 5_000; // events/min sampling window
const SPARK_LEN = 16;
const FRESH_MS = 2500;
const STREAM_FINDINGS_CAP = 60; // most-recent findings merged into the stream
const STALE_RUN_MS = 2 * 60_000; // silent this long + still live-looking → re-check run.json
const STALE_RECHECK_MS = 60_000; // per-run floor between those re-checks

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

/** Persisted `run.json` statuses that mean the run is over — used to stop
 *  the lazy `getRun` resolution from re-checking a run that already ended. */
const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled', 'rejected']);

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

  // The stream shows the most-recent findings only (getFindings can return
  // hundreds across all projects); the roster + pulse still count the full set.
  const findingCards = useMemo(
    () =>
      [...findings]
        .sort((a, b) => Date.parse(b.declared_at) - Date.parse(a.declared_at))
        .slice(0, STREAM_FINDINGS_CAP)
        .map(cardFromFinding),
    [findings],
  );

  const cards = useMemo(
    () => [...eventCards, ...findingCards].sort((a, b) => b.ts - a.ts),
    [eventCards, findingCards],
  );

  const activity = useMemo(
    () => reconcileActivity(deriveActivity(items), runStatus),
    [items, runStatus],
  );
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
