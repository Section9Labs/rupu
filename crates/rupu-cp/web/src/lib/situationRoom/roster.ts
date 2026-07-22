// Situation Room — pure aggregation for the right-hand project roster and the
// header pulse strip. Folds four REAL sources into view models:
//   • projects   — `GET /api/projects` (ws_id, name, repo_remote, branch)
//   • runToWs    — run_id → ws_id, learned lazily via `getRun` (events carry
//                  only run_id, never a project id — see the data-surface map)
//   • activity   — per-run live state distilled from the event stream
//   • findings   — `GET /api/findings` (REST; grouped here by ws_id)
//
// No fabricated numbers: a project with no active run reads "idle", and
// severity counts come straight from the findings list.

import {
  isKnownRunEvent,
  normFindingSeverity,
  type FindingOut,
  type FindingsSummary,
  type ProjectRow,
  type RunEvent,
} from '../api';

/** Per-run live state, distilled from the event stream by the page. */
export interface RunActivity {
  runId: string;
  state: 'running' | 'awaiting' | 'paused' | 'done' | 'failed';
  /** Short human label of what the run is doing right now (agent · step). */
  action?: string;
  /** ms since epoch of the last event seen for this run. */
  ts: number;
}

/** Latest state per run, folded from the (newest-first) event list — drives
 *  the roster's status + current action. The default for any mid-flight
 *  event is `running`; pause/terminal events flip it. */
export function deriveActivity(items: { ts: number; event: RunEvent }[]): Map<string, RunActivity> {
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
        case 'run_paused': state = 'paused'; action = 'paused'; break;
        case 'step_paused': state = 'paused'; action = `paused · ${event.step_id}`; break;
        case 'step_started': action = event.agent ? `${event.agent} · ${event.step_id}` : event.step_id; break;
        case 'step_working': action = event.note?.trim() || event.step_id; break;
        case 'step_completed': action = event.step_id; break;
        case 'step_resumed': action = event.step_id; break;
        case 'panel_round': action = `${event.step_id} · round ${event.round}`; break;
        default: break;
      }
    }
    out.set(runId, { runId, state, action, ts });
  }
  return out;
}

const TERMINAL_RUN_STATUSES = new Set(['completed', 'failed', 'cancelled', 'rejected']);

/**
 * Reconcile event-derived activity with the authoritative persisted run
 * status (`run.json`, learned via `getRun`). Some runs' `events.jsonl` ends
 * mid-step with no terminal event — historically every cancel did this (the
 * store didn't append one), and a crashed runner still does — so a
 * live-looking fold must yield to a terminal persisted status instead of
 * spinning forever. Already-terminal activity is never rewritten: the event
 * stream is fresher than a status fetched once per run.
 */
export function reconcileActivity(
  activity: Map<string, RunActivity>,
  persistedStatus: Map<string, string>,
): Map<string, RunActivity> {
  let out = activity;
  for (const [runId, act] of activity) {
    if (act.state === 'done' || act.state === 'failed') continue;
    const status = persistedStatus.get(runId);
    if (!status || !TERMINAL_RUN_STATUSES.has(status)) continue;
    if (out === activity) out = new Map(activity);
    out.set(runId, {
      ...act,
      state: status === 'completed' || status === 'cancelled' ? 'done' : 'failed',
      action: undefined,
    });
  }
  return out;
}

export interface SevCounts {
  critical: number;
  high: number;
  medium: number;
  low: number;
  info: number;
  total: number;
}

export interface RosterProject {
  wsId: string;
  name: string;
  branch?: string;
  status: 'running' | 'await' | 'idle';
  action?: string;
  activeRuns: number;
  findings: SevCounts;
  lastActiveTs?: number;
}

function emptySev(): SevCounts {
  return { critical: 0, high: 0, medium: 0, low: 0, info: 0, total: 0 };
}

/** Group findings by workspace into severity counts. */
export function findingsByWorkspace(findings: FindingOut[]): Map<string, SevCounts> {
  const out = new Map<string, SevCounts>();
  for (const f of findings) {
    const c = out.get(f.ws_id) ?? emptySev();
    c[normFindingSeverity(f.severity)] += 1;
    c.total += 1;
    out.set(f.ws_id, c);
  }
  return out;
}

const STATUS_RANK: Record<RosterProject['status'], number> = { await: 0, running: 1, idle: 2 };

/**
 * Build the roster: one card per project, ordered awaiting → running → idle,
 * then most-recently-active first. `runToWs` + `activity` are keyed by run_id;
 * we resolve each project's active runs through them.
 */
export function buildRoster(
  projects: ProjectRow[],
  runToWs: Map<string, string>,
  activity: Map<string, RunActivity>,
  findings: FindingOut[],
): RosterProject[] {
  const findMap = findingsByWorkspace(findings);

  // Invert run→ws into ws→[activity] so each project sees only its own runs.
  const byWs = new Map<string, RunActivity[]>();
  for (const [runId, act] of activity) {
    const ws = runToWs.get(runId);
    if (!ws) continue;
    const list = byWs.get(ws) ?? [];
    list.push(act);
    byWs.set(ws, list);
  }

  const rows = projects.map((p): RosterProject => {
    const acts = byWs.get(p.ws_id) ?? [];
    const live = acts.filter((a) => a.state === 'running' || a.state === 'awaiting');
    const awaiting = live.some((a) => a.state === 'awaiting');
    const status: RosterProject['status'] = awaiting ? 'await' : live.length > 0 ? 'running' : 'idle';

    // Current action = the most recent live run's action.
    const newestLive = live.slice().sort((a, b) => b.ts - a.ts)[0];
    const projLastActive = p.last_active ? Date.parse(p.last_active) : NaN;
    const lastActiveTs = acts.reduce(
      (mx, a) => Math.max(mx, a.ts),
      Number.isNaN(projLastActive) ? 0 : projLastActive,
    );

    return {
      wsId: p.ws_id,
      name: p.name,
      branch: p.branch ?? undefined,
      status,
      action: newestLive?.action,
      activeRuns: live.length,
      findings: findMap.get(p.ws_id) ?? emptySev(),
      lastActiveTs: lastActiveTs > 0 ? lastActiveTs : undefined,
    };
  });

  return rows.sort((a, b) => {
    if (STATUS_RANK[a.status] !== STATUS_RANK[b.status]) return STATUS_RANK[a.status] - STATUS_RANK[b.status];
    return (b.lastActiveTs ?? 0) - (a.lastActiveTs ?? 0);
  });
}

/** Count of projects with at least one active (running/awaiting) run. */
export function projectsLive(roster: RosterProject[]): number {
  return roster.filter((r) => r.status !== 'idle').length;
}

export interface Vitals {
  activeRuns: number;
  projectsLive: number;
  projectsTotal: number;
  awaiting: number;
  errors: number;
  eventsPerMin: number;
  findings: FindingsSummary;
}

const EMPTY_SUMMARY: FindingsSummary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 };

/**
 * Assemble the pulse-strip vitals from real aggregates. `activeRuns` /
 * `awaiting` come from the dashboard's live counts; `findings` from the
 * findings summary; `errors` and `eventsPerMin` are session-derived by the
 * page from the live stream. Any missing source degrades to zeros, never a
 * fabricated number.
 */
export function buildVitals(input: {
  activeRuns?: number | null;
  awaiting?: number | null;
  findings?: FindingsSummary | null;
  projectsLive: number;
  projectsTotal: number;
  errors: number;
  eventsPerMin: number;
}): Vitals {
  return {
    activeRuns: input.activeRuns ?? 0,
    projectsLive: input.projectsLive,
    projectsTotal: input.projectsTotal,
    awaiting: input.awaiting ?? 0,
    errors: input.errors,
    eventsPerMin: input.eventsPerMin,
    findings: input.findings ?? EMPTY_SUMMARY,
  };
}
