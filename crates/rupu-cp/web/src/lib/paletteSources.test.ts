import { describe, it, expect } from 'vitest';
import {
  rankPalette,
  runItems,
  agentItems,
  autoflowItems,
  findingItems,
  issueItems,
  workerItems,
  type PaletteItem,
} from './paletteSources';
import type {
  RunListRow,
  AgentSummary,
  AutoflowDefRow,
  FindingOut,
  AutoflowClaim,
  WorkerRecord,
} from './api';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const PAGES: PaletteItem[] = [
  { kind: 'page', id: 'runs', title: 'Runs', to: '/runs' },
  { kind: 'page', id: 'agents', title: 'Agents', to: '/agents' },
];

function item(kind: PaletteItem['kind'], title: string, id = title): PaletteItem {
  return { kind, id, title, to: `/${kind}/${id}` };
}

// ---------------------------------------------------------------------------
// rankPalette
// ---------------------------------------------------------------------------

describe('rankPalette', () => {
  it('empty query returns all items, pages first', () => {
    const items = [item('run', 'alpha'), item('agent', 'beta')];
    const groups = rankPalette('', items, PAGES);
    const kinds = groups.map((g) => g.kind);
    expect(kinds[0]).toBe('page');
    // every group present
    expect(kinds).toContain('run');
    expect(kinds).toContain('agent');
    const total = groups.reduce((n, g) => n + g.items.length, 0);
    expect(total).toBe(items.length + PAGES.length);
  });

  it('filters by fuzzy query', () => {
    const items = [
      item('run', 'deploy-prod'),
      item('agent', 'reviewer'),
      item('workflow', 'nightly-audit'),
    ];
    const groups = rankPalette('audit', items, []);
    const titles = groups.flatMap((g) => g.items.map((i) => i.title));
    expect(titles).toEqual(['nightly-audit']);
  });

  it('orders groups by KIND_ORDER (page → run → agent → … → worker)', () => {
    const items = [
      item('worker', 'w1'),
      item('finding', 'f1'),
      item('run', 'r1'),
      item('agent', 'a1'),
    ];
    const groups = rankPalette('', items, PAGES);
    expect(groups.map((g) => g.kind)).toEqual(['page', 'run', 'agent', 'finding', 'worker']);
  });

  it('caps each entity group at 6 items', () => {
    const items = Array.from({ length: 10 }, (_, i) => item('run', `run-${i}`, `id-${i}`));
    const groups = rankPalette('', items, []);
    const runGroup = groups.find((g) => g.kind === 'run')!;
    expect(runGroup.items.length).toBe(6);
  });

  it('never caps the page group — all pages survive an empty query', () => {
    const pages = Array.from({ length: 12 }, (_, i) =>
      item('page', `Page ${i}`, `p-${i}`),
    );
    const groups = rankPalette('', [], pages);
    const pageGroup = groups.find((g) => g.kind === 'page')!;
    expect(pageGroup.items.length).toBe(12);
  });

  it('falls back to subtitle then keywords when title misses', () => {
    const items: PaletteItem[] = [
      { kind: 'run', id: '1', title: 'nameless', subtitle: 'zzz', keywords: 'abc123', to: '/runs/1' },
    ];
    // 'abc' only appears in keywords
    const groups = rankPalette('abc', items, []);
    expect(groups.flatMap((g) => g.items).map((i) => i.id)).toEqual(['1']);
  });
});

// ---------------------------------------------------------------------------
// Mappers
// ---------------------------------------------------------------------------

describe('mappers', () => {
  it('runItems → /runs/:id', () => {
    const rows = [
      { id: 'ABC123XYZ', workflow_name: 'deploy', status: 'completed' } as RunListRow,
    ];
    const [it0] = runItems(rows);
    expect(it0.to).toBe('/runs/ABC123XYZ');
    expect(it0.title).toBe('deploy');
    expect(it0.subtitle).toBe('ABC123XY · completed');
  });

  it('agentItems → /agents/:name with provider/model subtitle', () => {
    const rows = [{ name: 'reviewer', provider: 'anthropic', model: 'opus' } as AgentSummary];
    const [a] = agentItems(rows);
    expect(a.to).toBe('/agents/reviewer');
    expect(a.subtitle).toBe('anthropic/opus');
  });

  it('autoflowItems → /workflows/:slug (reuses workflow route)', () => {
    const rows = [
      { name: 'Nightly', slug: 'nightly-flow', trigger: 'cron', scope: 'global' } as AutoflowDefRow,
    ];
    const [a] = autoflowItems(rows);
    expect(a.to).toBe('/workflows/nightly-flow');
    expect(a.subtitle).toBe('autoflow · cron');
  });

  it('findingItems → /findings (no detail route)', () => {
    const rows = [
      { id: 'f1', summary: 'sqli', severity: 'high', file_path: 'src/db.ts' } as FindingOut,
    ];
    const [f] = findingItems(rows);
    expect(f.to).toBe('/findings');
    expect(f.subtitle).toBe('high · src/db.ts');
  });

  it('issueItems → run when last_run_id present, else autoflows list', () => {
    const rows = [
      { issue_ref: 'r#1', issue_title: 'Bug', issue_display_ref: 'acme#1', status: 'running', last_run_id: 'RUN9' } as AutoflowClaim,
      { issue_ref: 'r#2', issue_display_ref: 'acme#2', status: 'blocked' } as AutoflowClaim,
    ];
    const [a, b] = issueItems(rows);
    expect(a.to).toBe('/runs/RUN9');
    expect(a.title).toBe('Bug');
    expect(b.to).toBe('/runs/autoflows');
    expect(b.title).toBe('acme#2');
  });

  it('workerItems → /workers (no detail route)', () => {
    const rows = [
      { worker_id: 'w1', name: 'runner-1', kind: 'autoflow', host: 'mac-1' } as WorkerRecord,
    ];
    const [w] = workerItems(rows);
    expect(w.to).toBe('/workers');
    expect(w.subtitle).toBe('autoflow · mac-1');
  });
});
