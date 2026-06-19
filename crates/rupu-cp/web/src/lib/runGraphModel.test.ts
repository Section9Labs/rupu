import { describe, it, expect } from 'vitest';
import { buildRunGraphModel } from './runGraphModel';
import type { RunGraphResponse, StepNodeDto, UnitCheckpoint, StepResultRecord, RunEvent } from './api';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const STEP_A: StepNodeDto = { id: 'a', kind: 'step', agent: 'agent-a' };
const STEP_B: StepNodeDto = {
  id: 'b',
  kind: 'for_each',
  agent: 'agent-b',
  for_each: 'items',
};
const STEP_C: StepNodeDto = { id: 'c', kind: 'step', agent: 'agent-c' };

function makeGraph(
  overrides: {
    steps?: StepNodeDto[];
    step_results?: StepResultRecord[];
    units?: UnitCheckpoint[];
  } = {},
): RunGraphResponse {
  return {
    run: {
      id: 'run-1',
      workflow_name: 'test-wf',
      status: 'running',
      started_at: '2026-06-18T00:00:00Z',
    },
    workflow: { steps: overrides.steps ?? [STEP_A, STEP_B, STEP_C] },
    step_results: overrides.step_results ?? [],
    units: overrides.units ?? [],
  };
}

function runId(): string { return 'run-1'; }

// ---------------------------------------------------------------------------
// 1. Skeleton only — all pending, edges a→b→c
// ---------------------------------------------------------------------------

describe('skeleton only', () => {
  it('all nodes are pending with no events or results', () => {
    const model = buildRunGraphModel(makeGraph(), []);
    expect(model.nodes).toHaveLength(3);
    for (const node of model.nodes) {
      expect(node.state).toBe('pending');
    }
  });

  it('edges chain a→b→c', () => {
    const model = buildRunGraphModel(makeGraph(), []);
    expect(model.edges).toEqual([
      { from: 'a', to: 'b' },
      { from: 'b', to: 'c' },
    ]);
  });

  it('nodeById returns the right node', () => {
    const model = buildRunGraphModel(makeGraph(), []);
    expect(model.nodeById('b')?.id).toBe('b');
    expect(model.nodeById('z')).toBeUndefined();
  });

  it('carries kind and agent from the DTO', () => {
    const model = buildRunGraphModel(makeGraph(), []);
    const a = model.nodeById('a')!;
    expect(a.kind).toBe('step');
    expect(a.agent).toBe('agent-a');
    const b = model.nodeById('b')!;
    expect(b.kind).toBe('for_each');
  });
});

// ---------------------------------------------------------------------------
// 2. step_results overlay
// ---------------------------------------------------------------------------

describe('step_results overlay', () => {
  it('success:true → done', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: true }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('a')!.state).toBe('done');
    expect(model.nodeById('c')!.state).toBe('pending');
  });

  it('success:false → failed', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: false }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('a')!.state).toBe('failed');
  });

  it('skipped:true → skipped (regardless of success)', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'c', success: false, skipped: true }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('c')!.state).toBe('skipped');
  });

  it('step_result.transcript_path → node.transcriptPath', () => {
    const g = makeGraph({
      step_results: [
        { run_id: runId(), step_id: 'a', success: true, transcript_path: '/tmp/step-a.jsonl' },
      ],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('a')!.transcriptPath).toBe('/tmp/step-a.jsonl');
    // A step with no result has no transcript path.
    expect(model.nodeById('c')!.transcriptPath).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// 3. Live events WIN over step_results
// ---------------------------------------------------------------------------

describe('live events win over step_results', () => {
  it('step_started overrides a done result → running', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: true }],
    });
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('running');
  });

  it('step_working also sets running', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: true }],
    });
    const events: RunEvent[] = [
      { type: 'step_working', run_id: runId(), step_id: 'a' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('running');
  });

  it('step_awaiting_approval → awaiting_approval', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'c', success: false }],
    });
    const events: RunEvent[] = [
      { type: 'step_awaiting_approval', run_id: runId(), step_id: 'c', reason: 'needs approval' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('c')!.state).toBe('awaiting_approval');
  });

  it('step_completed success:true → done (even if result said failed)', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: false }],
    });
    const events: RunEvent[] = [
      { type: 'step_completed', run_id: runId(), step_id: 'a', success: true, duration_ms: 100 },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('done');
  });

  it('step_completed success:false → failed', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'step_completed', run_id: runId(), step_id: 'a', success: false, duration_ms: 50 },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('failed');
  });

  it('step_failed → failed', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'step_failed', run_id: runId(), step_id: 'b', error: 'boom' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('b')!.state).toBe('failed');
  });

  it('step_skipped → skipped', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'step_skipped', run_id: runId(), step_id: 'c', reason: 'cond false' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('c')!.state).toBe('skipped');
  });

  it('later events in array win (last-event wins per step)', () => {
    // step_started then step_completed in order → completed wins
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step' },
      { type: 'step_completed', run_id: runId(), step_id: 'a', success: true, duration_ms: 200 },
    ];
    const model = buildRunGraphModel(makeGraph(), events);
    expect(model.nodeById('a')!.state).toBe('done');
  });
});

// ---------------------------------------------------------------------------
// 4. Units: checkpoints + unit events → fanout, parent state
// ---------------------------------------------------------------------------

describe('units from checkpoints', () => {
  const unitDone: UnitCheckpoint = {
    step_id: 'b', index: 0, item: 'file-a.ts',
    run_id: runId(), transcript_path: '/tmp/t0.jsonl',
    output: 'ok', success: true, finished_at: '2026-06-18T00:01:00Z',
  };
  const unitFailed: UnitCheckpoint = {
    step_id: 'b', index: 1, item: 'file-b.ts',
    run_id: runId(), transcript_path: '/tmp/t1.jsonl',
    output: 'fail', success: false, finished_at: '2026-06-18T00:02:00Z',
  };

  it('checkpoints build fanout.units with correct state', () => {
    const g = makeGraph({ units: [unitDone, unitFailed] });
    const model = buildRunGraphModel(g, []);
    const b = model.nodeById('b')!;
    expect(b.fanout).toBeDefined();
    expect(b.fanout!.units).toHaveLength(2);
    expect(b.fanout!.units[0].state).toBe('done');
    expect(b.fanout!.units[1].state).toBe('failed');
  });

  it('fanout.byState counts are correct', () => {
    const g = makeGraph({ units: [unitDone, unitFailed] });
    const model = buildRunGraphModel(g, []);
    const { byState } = model.nodeById('b')!.fanout!;
    expect(byState.done).toBe(1);
    expect(byState.failed).toBe(1);
    expect(byState.running ?? 0).toBe(0);
    expect(byState.pending ?? 0).toBe(0);
  });

  it('fanout.total is the count of all units', () => {
    const g = makeGraph({ units: [unitDone, unitFailed] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.total).toBe(2);
  });

  it('units sorted by index', () => {
    // Insert in reverse order — must come out sorted
    const g = makeGraph({ units: [unitFailed, unitDone] });
    const model = buildRunGraphModel(g, []);
    const units = model.nodeById('b')!.fanout!.units;
    expect(units[0].index).toBe(0);
    expect(units[1].index).toBe(1);
  });

  it('checkpoint transcriptPath is carried through', () => {
    const g = makeGraph({ units: [unitDone] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].transcriptPath).toBe('/tmp/t0.jsonl');
  });

  it('parent step with terminal-only units keeps original state (not flipped to running)', () => {
    const g = makeGraph({ units: [unitDone] });
    const model = buildRunGraphModel(g, []);
    // step b has a done unit but no in-flight units — keep its own result state
    // (no result here, so it stays pending)
    expect(model.nodeById('b')!.state).toBe('pending');
  });

  it('parent step with in-flight unit_started event flips to running', () => {
    const g = makeGraph({ units: [unitDone] });
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 2, unit_key: 'file-c.ts', transcript_path: '/tmp/t2.jsonl' },
    ];
    const model = buildRunGraphModel(g, events);
    const b = model.nodeById('b')!;
    // A running unit exists → step is running
    expect(b.state).toBe('running');
    // The new unit is in the fanout
    const runningUnit = b.fanout!.units.find(u => u.index === 2);
    expect(runningUnit).toBeDefined();
    expect(runningUnit!.state).toBe('running');
    expect(runningUnit!.key).toBe('file-c.ts');
  });

  it('unit_started + unit_completed → unit is done, parent not forced running', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 0, unit_key: 'x.ts', transcript_path: '/tmp/tx.jsonl' },
      { type: 'unit_completed', run_id: runId(), step_id: 'b', index: 0, unit_key: 'x.ts', success: true, tokens_in: 10, tokens_out: 20 },
    ];
    const model = buildRunGraphModel(g, events);
    const b = model.nodeById('b')!;
    const unit = b.fanout!.units.find(u => u.index === 0)!;
    expect(unit.state).toBe('done');
    // No running units → step not forced to running
    expect(b.state).toBe('pending');
  });

  it('unit_completed success:false → unit failed', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 0, unit_key: 'y.ts', transcript_path: '/tmp/ty.jsonl' },
      { type: 'unit_completed', run_id: runId(), step_id: 'b', index: 0, unit_key: 'y.ts', success: false, tokens_in: 5, tokens_out: 10 },
    ];
    const model = buildRunGraphModel(g, events);
    const unit = model.nodeById('b')!.fanout!.units.find(u => u.index === 0)!;
    expect(unit.state).toBe('failed');
  });

  it('fanout.byState tracks running count when a unit_started event has no completion', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 0, unit_key: 'z.ts', transcript_path: '/tmp/tz.jsonl' },
    ];
    const model = buildRunGraphModel(g, events);
    const { byState } = model.nodeById('b')!.fanout!;
    expect(byState.running).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// 4b. Unit checkpoint success: null → running (started, not completed)
// ---------------------------------------------------------------------------

describe('unit checkpoint with success: null', () => {
  function makeNullUnit(success: boolean | null): UnitCheckpoint {
    return {
      step_id: 'b', index: 0, item: 'file-a.ts',
      run_id: runId(), transcript_path: '/tmp/t0.jsonl',
      output: '', success, finished_at: '2026-06-18T00:01:00Z',
    };
  }

  it('success: null → unit state is running', () => {
    const g = makeGraph({ units: [makeNullUnit(null)] });
    const model = buildRunGraphModel(g, []);
    const unit = model.nodeById('b')!.fanout!.units[0];
    expect(unit.state).toBe('running');
  });

  it('success: true → unit state is done', () => {
    const g = makeGraph({ units: [makeNullUnit(true)] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].state).toBe('done');
  });

  it('success: false → unit state is failed', () => {
    const g = makeGraph({ units: [makeNullUnit(false)] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].state).toBe('failed');
  });

  it('success: null unit counts as running in fanout.byState', () => {
    const g = makeGraph({ units: [makeNullUnit(null)] });
    const model = buildRunGraphModel(g, []);
    const { byState } = model.nodeById('b')!.fanout!;
    expect(byState.running).toBe(1);
    expect(byState.failed).toBe(0);
  });

  it('success: null unit flips parent step from pending to running', () => {
    const g = makeGraph({ units: [makeNullUnit(null)] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.state).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// 5. coerceItem: object item → JSON string key
// ---------------------------------------------------------------------------

describe('coerceItem', () => {
  it('string item stays as-is', () => {
    const g = makeGraph({
      units: [{ step_id: 'b', index: 0, item: 'hello', run_id: runId(), transcript_path: '/t', output: '', success: true, finished_at: '' }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].key).toBe('hello');
  });

  it('object item is JSON.stringified', () => {
    const g = makeGraph({
      units: [{ step_id: 'b', index: 0, item: { file: 'main.rs', line: 42 }, run_id: runId(), transcript_path: '/t', output: '', success: true, finished_at: '' }],
    });
    const model = buildRunGraphModel(g, []);
    const key = model.nodeById('b')!.fanout!.units[0].key;
    expect(typeof key).toBe('string');
    expect(key).toBe('{"file":"main.rs","line":42}');
  });

  it('number item is stringified', () => {
    const g = makeGraph({
      units: [{ step_id: 'b', index: 0, item: 7, run_id: runId(), transcript_path: '/t', output: '', success: true, finished_at: '' }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].key).toBe('7');
  });

  it('null item stringifies to "null"', () => {
    const g = makeGraph({
      units: [{ step_id: 'b', index: 0, item: null, run_id: runId(), transcript_path: '/t', output: '', success: true, finished_at: '' }],
    });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].key).toBe('null');
  });
});

// ---------------------------------------------------------------------------
// 6. Parallel sub-steps
// ---------------------------------------------------------------------------

describe('parallel sub-steps', () => {
  const STEP_PAR: StepNodeDto = {
    id: 'par', kind: 'parallel',
    parallel: [
      { id: 'par-x', agent: 'agent-x' },
      { id: 'par-y', agent: 'agent-y' },
    ],
  };

  it('parallel sub-steps default to pending', () => {
    const g = makeGraph({ steps: [STEP_PAR] });
    const model = buildRunGraphModel(g, []);
    const par = model.nodeById('par')!;
    expect(par.parallel).toBeDefined();
    expect(par.parallel).toHaveLength(2);
    for (const sub of par.parallel!) {
      expect(sub.state).toBe('pending');
    }
  });
});

// ---------------------------------------------------------------------------
// 7. gate is carried through
// ---------------------------------------------------------------------------

describe('gate field', () => {
  it('gate is present on the node when defined on the DTO', () => {
    const STEP_GATE: StepNodeDto = {
      id: 'g', kind: 'panel',
      gate: { max_iterations: 3, until_severity: 'high', fix_with: 'fixer-agent' },
    };
    const g = makeGraph({ steps: [STEP_GATE] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('g')!.gate).toEqual({
      max_iterations: 3, until_severity: 'high', fix_with: 'fixer-agent',
    });
  });
});

// ---------------------------------------------------------------------------
// 8. panel_round events set node.round
// ---------------------------------------------------------------------------

describe('panel_round events', () => {
  const PANEL_STEP: StepNodeDto = {
    id: 'panel', kind: 'panel',
    gate: { max_iterations: 5, until_severity: 'high', fix_with: 'fixer' },
  };

  it('panel_round sets round.current and round.max on the node', () => {
    const g = makeGraph({ steps: [PANEL_STEP] });
    const events: RunEvent[] = [
      { type: 'panel_round', run_id: runId(), step_id: 'panel', round: 2, max_iterations: 5 },
    ];
    const model = buildRunGraphModel(g, events);
    const node = model.nodeById('panel')!;
    expect(node.round).toEqual({ current: 2, max: 5 });
  });

  it('later panel_round wins — last event overwrites earlier', () => {
    const g = makeGraph({ steps: [PANEL_STEP] });
    const events: RunEvent[] = [
      { type: 'panel_round', run_id: runId(), step_id: 'panel', round: 1, max_iterations: 5 },
      { type: 'panel_round', run_id: runId(), step_id: 'panel', round: 2, max_iterations: 5 },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('panel')!.round).toEqual({ current: 2, max: 5 });
  });

  it('panel_round for unknown step_id is a no-op (does not throw)', () => {
    const g = makeGraph({ steps: [PANEL_STEP] });
    const events: RunEvent[] = [
      { type: 'panel_round', run_id: runId(), step_id: 'nonexistent', round: 1, max_iterations: 3 },
    ];
    // Should not throw; model is unmodified.
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('panel')!.round).toBeUndefined();
  });

  it('node has no round property before any panel_round event', () => {
    const g = makeGraph({ steps: [PANEL_STEP] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('panel')!.round).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// 9. panel units fold onto a panel node's fanout (by step_id, regardless of kind)
// ---------------------------------------------------------------------------

describe('panel units fold onto fanout', () => {
  const PANEL_STEP: StepNodeDto = {
    id: 'panel', kind: 'panel',
    gate: { max_iterations: 3, until_severity: 'high', fix_with: 'fixer' },
  };

  // Backend now merges panel panelist/fixer runs (from events.jsonl) into
  // g.units with the same UnitCheckpoint field shape. The model must fold
  // them onto the panel node's fanout.units even though kind === 'panel'.
  const panelistA: UnitCheckpoint = {
    step_id: 'panel', index: 0, item: 'reviewer-a',
    run_id: runId(), transcript_path: '/tmp/panel_a.jsonl',
    output: '', success: true, finished_at: '2026-06-18T00:01:00Z',
  };
  const panelistB: UnitCheckpoint = {
    step_id: 'panel', index: 1, item: 'reviewer-b',
    run_id: runId(), transcript_path: '/tmp/panel_b.jsonl',
    output: '', success: false, finished_at: '2026-06-18T00:02:00Z',
  };

  it('panel node gets fanout.units with their transcriptPath', () => {
    const g = makeGraph({ steps: [PANEL_STEP], units: [panelistA, panelistB] });
    const model = buildRunGraphModel(g, []);
    const node = model.nodeById('panel')!;
    expect(node.kind).toBe('panel');
    expect(node.fanout).toBeDefined();
    expect(node.fanout!.units).toHaveLength(2);
    expect(node.fanout!.units[0].key).toBe('reviewer-a');
    expect(node.fanout!.units[0].transcriptPath).toBe('/tmp/panel_a.jsonl');
    expect(node.fanout!.units[0].state).toBe('done');
    expect(node.fanout!.units[1].key).toBe('reviewer-b');
    expect(node.fanout!.units[1].transcriptPath).toBe('/tmp/panel_b.jsonl');
    expect(node.fanout!.units[1].state).toBe('failed');
  });
});
