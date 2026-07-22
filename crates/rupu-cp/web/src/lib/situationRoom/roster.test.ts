// buildRoster / findingsByWorkspace — the pure aggregation behind the project
// roster. Guards: awaiting sorts above running above idle; findings attribute
// to the right workspace; a project with no active run reads idle.

import { describe, it, expect } from 'vitest';
import { buildRoster, findingsByWorkspace, projectsLive, buildVitals, type RunActivity } from './roster';
import type { FindingOut, ProjectRow } from '../api';

function project(ws: string, name: string, extra: Partial<ProjectRow> = {}): ProjectRow {
  return {
    ws_id: ws, name, path: `/repos/${name}`, repo_remote: null, branch: 'main',
    repo_home_url: null, created_at: '2026-01-01T00:00:00Z', last_run_at: null,
    usage: {} as unknown as ProjectRow['usage'], run_count: 0, last_active: null,
    ...extra,
  };
}
function finding(ws: string, sev: string): FindingOut {
  return {
    id: `${ws}-${sev}-${Math.random()}`, ws_id: ws, project: ws, target_id: 't',
    file_path: null, line_range: null, scope: null, summary: 's', severity: sev,
    concern_id: null, evidence: { rationale: 'r' }, declared_by: null, declared_at: '2026-07-21T00:00:00Z',
  };
}
function act(runId: string, state: RunActivity['state'], ts: number, action?: string): RunActivity {
  return { runId, state, ts, action };
}

describe('findingsByWorkspace', () => {
  it('buckets counts per workspace and severity', () => {
    const m = findingsByWorkspace([finding('ws1', 'high'), finding('ws1', 'high'), finding('ws1', 'low'), finding('ws2', 'critical')]);
    expect(m.get('ws1')).toMatchObject({ high: 2, low: 1, total: 3 });
    expect(m.get('ws2')).toMatchObject({ critical: 1, total: 1 });
  });
});

describe('buildRoster', () => {
  const projects = [project('ws1', 'billing-api'), project('ws2', 'notes-svc'), project('ws3', 'idle-app')];
  const runToWs = new Map([['rA', 'ws1'], ['rB', 'ws2']]);

  it('orders awaiting → running → idle and attributes findings', () => {
    const activity = new Map<string, RunActivity>([
      ['rA', act('rA', 'running', 100, 'oracle-sec · audit')],
      ['rB', act('rB', 'awaiting', 200)],
    ]);
    const findings = [finding('ws1', 'high'), finding('ws2', 'critical')];
    const roster = buildRoster(projects, runToWs, activity, findings);

    expect(roster.map((r) => r.name)).toEqual(['notes-svc', 'billing-api', 'idle-app']);
    expect(roster[0].status).toBe('await');
    expect(roster[1].status).toBe('running');
    expect(roster[1].action).toBe('oracle-sec · audit');
    expect(roster[2].status).toBe('idle');
    expect(roster[1].findings.high).toBe(1);
    expect(roster[0].findings.critical).toBe(1);
    expect(projectsLive(roster)).toBe(2);
  });

  it('a run with no known workspace does not attribute to any project', () => {
    const activity = new Map<string, RunActivity>([['orphan', act('orphan', 'running', 100)]]);
    const roster = buildRoster(projects, new Map(), activity, []);
    expect(roster.every((r) => r.status === 'idle')).toBe(true);
  });
});

describe('buildVitals', () => {
  it('degrades missing sources to zeros, never fabricates', () => {
    const v = buildVitals({ projectsLive: 2, projectsTotal: 5, errors: 1, eventsPerMin: 12 });
    expect(v.activeRuns).toBe(0);
    expect(v.awaiting).toBe(0);
    expect(v.findings.total).toBe(0);
    expect(v.projectsLive).toBe(2);
    expect(v.eventsPerMin).toBe(12);
  });
});
