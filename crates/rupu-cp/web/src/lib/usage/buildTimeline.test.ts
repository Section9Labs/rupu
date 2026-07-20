import { describe, it, expect } from 'vitest';
import { buildTimeline } from './buildTimeline';
import type { TimelineFilter } from './buildTimeline';
import type { UsageRunRow } from '../usage';

function row(overrides: Partial<UsageRunRow>): UsageRunRow {
  return {
    run_id: 'run_1',
    started_at: '2026-07-01T10:00:00Z',
    workflow_name: 'wf',
    agent: 'agent_1',
    provider: 'anthropic',
    model: 'claude',
    workspace_id: 'ws_1',
    host_id: 'local',
    input_tokens: 100,
    output_tokens: 50,
    cached_tokens: 0,
    total_tokens: 150,
    cost_usd: 1,
    priced: true,
    ...overrides,
  };
}

function noFilter(): TimelineFilter {
  return { excludedRunIds: new Set(), excludedKeys: new Set() };
}

describe('buildTimeline', () => {
  it('two runs same day different models -> one bucket, two model series summed', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'claude', input_tokens: 10, output_tokens: 5, total_tokens: 15, cost_usd: 1 }),
      row({
        run_id: 'run_b',
        started_at: '2026-07-01T18:00:00Z',
        model: 'gpt',
        input_tokens: 20,
        output_tokens: 8,
        total_tokens: 28,
        cost_usd: 2,
      }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'day');

    expect(buckets).toHaveLength(1);
    expect(buckets[0].bucket).toBe('2026-07-01');
    expect(buckets[0].rows).toHaveLength(2);

    const claude = buckets[0].rows.find((r) => r.model === 'claude');
    const gpt = buckets[0].rows.find((r) => r.model === 'gpt');
    expect(claude).toBeDefined();
    expect(gpt).toBeDefined();
    expect(claude!.total_tokens).toBe(15);
    expect(claude!.cost_usd).toBe(1);
    expect(gpt!.total_tokens).toBe(28);
    expect(gpt!.cost_usd).toBe(2);
  });

  it('summing two rows for the same model in the same bucket', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'claude', input_tokens: 10, output_tokens: 5, total_tokens: 15, cost_usd: 1 }),
      row({ run_id: 'run_c', model: 'claude', input_tokens: 4, output_tokens: 1, total_tokens: 5, cost_usd: 0.5 }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'day');

    expect(buckets).toHaveLength(1);
    expect(buckets[0].rows).toHaveLength(1);
    expect(buckets[0].rows[0].total_tokens).toBe(20);
    expect(buckets[0].rows[0].cost_usd).toBe(1.5);
    expect(buckets[0].rows[0].priced).toBe(true);
  });

  it('excluding a run_id drops its contribution (a model with only that run disappears)', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'claude', cost_usd: 1 }),
      row({ run_id: 'run_b', model: 'gpt', cost_usd: 2 }),
    ];

    const filter: TimelineFilter = { excludedRunIds: new Set(['run_b']), excludedKeys: new Set() };
    const buckets = buildTimeline(rows, 'model', filter, 'day');

    expect(buckets).toHaveLength(1);
    expect(buckets[0].rows).toHaveLength(1);
    expect(buckets[0].rows[0].model).toBe('claude');
  });

  it('excluding a pivot key drops all its rows, even across multiple runs', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'gpt', cost_usd: 1 }),
      row({ run_id: 'run_b', model: 'gpt', cost_usd: 2 }),
      row({ run_id: 'run_c', model: 'claude', cost_usd: 3 }),
    ];

    const filter: TimelineFilter = { excludedRunIds: new Set(), excludedKeys: new Set(['gpt']) };
    const buckets = buildTimeline(rows, 'model', filter, 'day');

    expect(buckets).toHaveLength(1);
    expect(buckets[0].rows).toHaveLength(1);
    expect(buckets[0].rows[0].model).toBe('claude');
  });

  it("pivot='agent' stacks by agent", () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', agent: 'reviewer', model: 'claude', total_tokens: 10, cost_usd: 1 }),
      row({ run_id: 'run_b', agent: 'planner', model: 'claude', total_tokens: 20, cost_usd: 2 }),
    ];

    const buckets = buildTimeline(rows, 'agent', noFilter(), 'day');

    expect(buckets[0].rows).toHaveLength(2);
    const reviewer = buckets[0].rows.find((r) => r.agent === 'reviewer');
    const planner = buckets[0].rows.find((r) => r.agent === 'planner');
    expect(reviewer!.total_tokens).toBe(10);
    expect(planner!.total_tokens).toBe(20);
    // model field is left blank for the agent pivot per the "only the
    // matching identity field is populated" convention.
    expect(reviewer!.model).toBe('');
  });

  it("pivot='workflow' stacks by the real `workflow` field only — no mirroring into `model`", () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', workflow_name: 'nightly-scan', total_tokens: 10, cost_usd: 1 }),
      row({ run_id: 'run_b', workflow_name: 'pr-review', total_tokens: 20, cost_usd: 2 }),
    ];

    const buckets = buildTimeline(rows, 'workflow', noFilter(), 'day');

    expect(buckets[0].rows).toHaveLength(2);
    const nightly = buckets[0].rows.find((r) => r.workflow === 'nightly-scan');
    expect(nightly).toBeDefined();
    // The old U2 workaround mirrored the pivot key into `model` so the chart
    // component (which only knew model/provider/agent) could still resolve a
    // series. `UsageTimelineStacked` now stacks by the real pivot field
    // directly, so `model` stays blank per the "only the matching identity
    // field is populated" convention every other pivot already follows.
    expect(nightly!.model).toBe('');
  });

  it("pivot='host' populates only `host_id`, not `model`", () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', host_id: 'local', total_tokens: 10, cost_usd: 1 }),
      row({ run_id: 'run_b', host_id: 'host_remote', total_tokens: 20, cost_usd: 2 }),
    ];

    const buckets = buildTimeline(rows, 'host', noFilter(), 'day');

    expect(buckets[0].rows).toHaveLength(2);
    const local = buckets[0].rows.find((r) => r.host_id === 'local');
    expect(local).toBeDefined();
    expect(local!.model).toBe('');
  });

  it("pivot='project' populates only `workspace_id`, not `model`", () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', workspace_id: 'ws_a', total_tokens: 10, cost_usd: 1 }),
      row({ run_id: 'run_b', workspace_id: 'ws_b', total_tokens: 20, cost_usd: 2 }),
    ];

    const buckets = buildTimeline(rows, 'project', noFilter(), 'day');

    expect(buckets[0].rows).toHaveLength(2);
    const wsA = buckets[0].rows.find((r) => r.workspace_id === 'ws_a');
    expect(wsA).toBeDefined();
    expect(wsA!.model).toBe('');
  });

  it('a null-cost row adds 0 cost but its tokens still count; priced becomes false', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'claude', total_tokens: 10, cost_usd: 1, priced: true }),
      row({ run_id: 'run_b', model: 'claude', total_tokens: 40, cost_usd: null, priced: false }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'day');

    expect(buckets[0].rows).toHaveLength(1);
    const claude = buckets[0].rows[0];
    expect(claude.total_tokens).toBe(50);
    expect(claude.cost_usd).toBe(1);
    expect(claude.priced).toBe(false);
  });

  it('cost_usd is null (not 0) when every contributing row is unpriced', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', model: 'mystery', total_tokens: 10, cost_usd: null, priced: false }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'day');

    expect(buckets[0].rows[0].cost_usd).toBeNull();
    expect(buckets[0].rows[0].priced).toBe(false);
    expect(buckets[0].rows[0].total_tokens).toBe(10);
  });

  it('week bucketing groups by ISO-Monday', () => {
    // 2026-06-24 is a Wednesday; its ISO week starts Monday 2026-06-22.
    // 2026-06-28 is the following Sunday, same ISO week.
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', started_at: '2026-06-24T13:45:00Z', model: 'claude', total_tokens: 10 }),
      row({ run_id: 'run_b', started_at: '2026-06-28T23:59:00Z', model: 'claude', total_tokens: 5 }),
      row({ run_id: 'run_c', started_at: '2026-06-22T00:00:00Z', model: 'claude', total_tokens: 1 }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'week');

    expect(buckets).toHaveLength(1);
    expect(buckets[0].bucket).toBe('2026-06-22');
    expect(buckets[0].rows[0].total_tokens).toBe(16);
  });

  it('week bucketing puts a following-week run in a separate bucket', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', started_at: '2026-06-24T13:45:00Z', model: 'claude', total_tokens: 10 }),
      row({ run_id: 'run_b', started_at: '2026-06-29T00:00:00Z', model: 'claude', total_tokens: 5 }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'week');

    expect(buckets.map((b) => b.bucket)).toEqual(['2026-06-22', '2026-06-29']);
  });

  it('buckets are sparse (no zero-fill) and sorted chronologically', () => {
    const rows: UsageRunRow[] = [
      row({ run_id: 'run_a', started_at: '2026-07-05T00:00:00Z', model: 'claude' }),
      row({ run_id: 'run_b', started_at: '2026-07-01T00:00:00Z', model: 'claude' }),
    ];

    const buckets = buildTimeline(rows, 'model', noFilter(), 'day');

    expect(buckets.map((b) => b.bucket)).toEqual(['2026-07-01', '2026-07-05']);
  });

  it('returns no buckets for an empty input', () => {
    expect(buildTimeline([], 'model', noFilter(), 'day')).toEqual([]);
  });
});
