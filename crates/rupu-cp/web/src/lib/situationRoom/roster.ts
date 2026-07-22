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
  normFindingSeverity,
  type FindingOut,
  type FindingsSummary,
  type ProjectRow,
} from '../api';

/** Per-run live state, distilled from the event stream by the page. */
export interface RunActivity {
  runId: string;
  state: 'running' | 'awaiting' | 'done' | 'failed';
  /** Short human label of what the run is doing right now (agent · step). */
  action?: string;
  /** ms since epoch of the last event seen for this run. */
  ts: number;
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
