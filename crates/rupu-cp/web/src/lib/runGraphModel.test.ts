import { describe, it, expect } from 'vitest';
import { buildRunGraphModel, collectWarnings } from './runGraphModel';
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
    edges?: { from: string; to: string }[];
  } = {},
): RunGraphResponse {
  return {
    run: {
      id: 'run-1',
      workflow_name: 'test-wf',
      status: 'running',
      started_at: '2026-06-18T00:00:00Z',
    },
    workflow: {
      steps: overrides.steps ?? [STEP_A, STEP_B, STEP_C],
      // Only attach `edges` when the test supplies them, so tests that omit
      // it exercise the older-backend / linear-chain fallback path.
      ...(overrides.edges !== undefined ? { edges: overrides.edges } : {}),
    },
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
});

// ---------------------------------------------------------------------------
// 1b. Real DAG edges — the run graph forks where the workflow forks.
// ---------------------------------------------------------------------------

describe('real DAG edges', () => {
  const forkSteps: StepNodeDto[] = [
    { id: 'start', kind: 'step', agent: 'a' },
    { id: 'fork', kind: 'split' },
    { id: 'left', kind: 'step', agent: 'l' },
    { id: 'right', kind: 'step', agent: 'r' },
    { id: 'join', kind: 'join' },
  ];
  const forkEdges = [
    { from: 'start', to: 'fork' },
    { from: 'fork', to: 'left' },
    { from: 'fork', to: 'right' },
    { from: 'left', to: 'join' },
    { from: 'right', to: 'join' },
  ];

  it('uses workflow.edges when present, forking on a split', () => {
    const model = buildRunGraphModel(makeGraph({ steps: forkSteps, edges: forkEdges }), []);
    expect(model.edges).toEqual(forkEdges);
    // The bifurcation: the split has two outgoing edges, the join two incoming.
    expect(model.edges.filter((e) => e.from === 'fork')).toHaveLength(2);
    expect(model.edges.filter((e) => e.to === 'join')).toHaveLength(2);
    expect(model.nodeById('fork')!.kind).toBe('split');
    expect(model.nodeById('join')!.kind).toBe('join');
  });

  it('drops an edge that names an unknown step', () => {
    const model = buildRunGraphModel(
      makeGraph({
        steps: [
          { id: 'a', kind: 'step' },
          { id: 'b', kind: 'step' },
        ],
        edges: [
          { from: 'a', to: 'b' },
          { from: 'a', to: 'ghost' },
        ],
      }),
      [],
    );
    expect(model.edges).toEqual([{ from: 'a', to: 'b' }]);
  });

  it('respects an explicit empty edges array (single-node run — no chain)', () => {
    const model = buildRunGraphModel(
      makeGraph({ steps: [{ id: 'only', kind: 'step' }], edges: [] }),
      [],
    );
    expect(model.edges).toEqual([]);
  });

  it('falls back to a linear chain when the edges field is absent (older backend)', () => {
    // makeGraph() omits `edges` → the model rebuilds the consecutive-pair chain.
    const model = buildRunGraphModel(makeGraph(), []);
    expect(model.edges).toEqual([
      { from: 'a', to: 'b' },
      { from: 'b', to: 'c' },
    ]);
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

  it('step_working carries transcript_path for a running single step (no result yet)', () => {
    // A running linear step has no persisted step_result, so its transcript
    // path arrives only via the live step_working event. Without it the panel
    // has nothing to select/tail.
    const g = makeGraph();
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step' },
      {
        type: 'step_working',
        run_id: runId(),
        step_id: 'a',
        transcript_path: '/tmp/transcripts/run_X.jsonl',
      },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('running');
    expect(model.nodeById('a')!.transcriptPath).toBe('/tmp/transcripts/run_X.jsonl');
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
// 4c. Phase 5b: completed run reconciles lingering in-flight units to done
// ---------------------------------------------------------------------------

describe('completed run reconciliation (Phase 5b)', () => {
  function makeNullUnit(): UnitCheckpoint {
    return {
      step_id: 'b', index: 0, item: 'file-a.ts',
      run_id: runId(), transcript_path: '/tmp/t0.jsonl',
      output: '', success: null, finished_at: '2026-06-18T00:01:00Z',
    };
  }

  it('completed run: success:null unit and parent node both promote to done', () => {
    const g = makeGraph({ steps: [STEP_B], units: [makeNullUnit()] });
    g.run.status = 'completed';
    const model = buildRunGraphModel(g, []);
    const b = model.nodeById('b')!;
    expect(b.state).toBe('done');
    expect(b.fanout!.units[0].state).toBe('done');
    // byState badges recomputed to match the promoted units.
    expect(b.fanout!.byState.done).toBe(1);
    expect(b.fanout!.byState.running).toBe(0);
  });

  it('failed run: success:null unit stays running (NOT silently marked done)', () => {
    const g = makeGraph({ steps: [STEP_B], units: [makeNullUnit()] });
    g.run.status = 'failed';
    const model = buildRunGraphModel(g, []);
    const b = model.nodeById('b')!;
    expect(b.fanout!.units[0].state).toBe('running');
    expect(b.fanout!.byState.running).toBe(1);
    expect(b.fanout!.byState.done).toBe(0);
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

// ---------------------------------------------------------------------------
// 10. Pause / resume events (Task 8) — a step_paused/step_resumed pair flips
// the paused step's node state; the run-level run_paused/run_resumed events
// carry no step_id and are a no-op at the per-step level (mirroring
// run_started/run_completed/run_failed).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 10. gate / action kinds pass through (data-only; Task 3 wires rendering)
// ---------------------------------------------------------------------------

describe('gate and action kinds', () => {
  it('a kind:"gate" DTO produces a GraphNode with kind:"gate"', () => {
    const GATE_STEP: StepNodeDto = {
      id: 'approve',
      kind: 'gate',
      approval_gate: { auto_approve: false, has_on_reject: true, timeout_seconds: 3600 },
    };
    const g = makeGraph({ steps: [GATE_STEP] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('approve')!.kind).toBe('gate');
  });

  it('step_awaiting_approval still folds to awaiting_approval state on a gate node', () => {
    const GATE_STEP: StepNodeDto = { id: 'approve', kind: 'gate' };
    const g = makeGraph({ steps: [GATE_STEP] });
    const events: RunEvent[] = [
      { type: 'step_awaiting_approval', run_id: runId(), step_id: 'approve', reason: 'gate' },
    ];
    const model = buildRunGraphModel(g, events);
    const node = model.nodeById('approve')!;
    expect(node.kind).toBe('gate');
    expect(node.state).toBe('awaiting_approval');
  });

  it('a kind:"action" DTO produces a GraphNode with kind:"action"', () => {
    const ACTION_STEP: StepNodeDto = { id: 'create_pr', kind: 'action', action: 'scm.prs.create' };
    const g = makeGraph({ steps: [ACTION_STEP] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('create_pr')!.kind).toBe('action');
  });

  it('threads dto.action onto the GraphNode', () => {
    const ACTION_STEP: StepNodeDto = { id: 'create_pr', kind: 'action', action: 'scm.prs.create' };
    const g = makeGraph({ steps: [ACTION_STEP] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('create_pr')!.action).toBe('scm.prs.create');
  });

  it('threads dto.approval_gate onto the GraphNode', () => {
    const GATE_STEP: StepNodeDto = {
      id: 'approve',
      kind: 'gate',
      approval_gate: { auto_approve: true, has_on_reject: true, timeout_seconds: 3600 },
    };
    const g = makeGraph({ steps: [GATE_STEP] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('approve')!.approval_gate).toEqual({
      auto_approve: true,
      has_on_reject: true,
      timeout_seconds: 3600,
    });
  });

  it('leaves action/approval_gate undefined when the DTO has none', () => {
    const g = makeGraph({ steps: [STEP_A] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('a')!.action).toBeUndefined();
    expect(model.nodeById('a')!.approval_gate).toBeUndefined();
  });
});

describe('pause / resume events', () => {
  it('step_paused sets the node state to paused', () => {
    const g = makeGraph({ steps: [STEP_A, STEP_B, STEP_C] });
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step', agent: 'agent-a' },
      { type: 'run_paused', run_id: runId() },
      { type: 'step_paused', run_id: runId(), step_id: 'a' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('paused');
    // Untouched steps stay pending.
    expect(model.nodeById('b')!.state).toBe('pending');
  });

  it('step_resumed reverts a paused node back to running', () => {
    const g = makeGraph({ steps: [STEP_A] });
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step', agent: 'agent-a' },
      { type: 'step_paused', run_id: runId(), step_id: 'a' },
      { type: 'run_resumed', run_id: runId() },
      { type: 'step_resumed', run_id: runId(), step_id: 'a' },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.state).toBe('running');
  });

  it('step_paused for an unknown step_id is a no-op (does not throw)', () => {
    const g = makeGraph({ steps: [STEP_A] });
    const events: RunEvent[] = [
      { type: 'step_paused', run_id: runId(), step_id: 'nonexistent' },
    ];
    expect(() => buildRunGraphModel(g, events)).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// Codenames + provider/model (agent codenames Plan 2, Task 5)
// ---------------------------------------------------------------------------

describe('codenames and provider/model', () => {
  const cp = (index: number, codename?: string): UnitCheckpoint => ({
    step_id: 'b', index, item: `f${index}.ts`,
    run_id: runId(), transcript_path: `/tmp/t${index}.jsonl`,
    output: 'ok', success: true, finished_at: '2026-06-18T00:01:00Z',
    ...(codename ? { codename } : {}),
  });

  it('checkpoint codename flows to UnitView.codename', () => {
    const g = makeGraph({ units: [cp(0, 'otter-3/scout#0')] });
    const model = buildRunGraphModel(g, []);
    expect(model.nodeById('b')!.fanout!.units[0].codename).toBe('otter-3/scout#0');
  });

  it('agent_started with unit_index sets provider/model on that unit only', () => {
    const g = makeGraph({ units: [cp(0), cp(1), cp(2), cp(3)] });
    const events: RunEvent[] = [
      {
        type: 'agent_started', run_id: runId(), step_id: 'b', unit_index: 3,
        codename: 'otter-3/scout#3', agent: 'agent-b', provider: 'anthropic',
        model: 'claude-sonnet-4-6', agent_run_id: 'ar-3', transcript_path: '/tmp/t3.jsonl',
      },
    ];
    const model = buildRunGraphModel(g, events);
    const b = model.nodeById('b')!;
    const u3 = b.fanout!.units.find((u) => u.index === 3)!;
    expect(u3.provider).toBe('anthropic');
    expect(u3.model).toBe('claude-sonnet-4-6');
    expect(u3.codename).toBe('otter-3/scout#3');
    const u2 = b.fanout!.units.find((u) => u.index === 2)!;
    expect(u2.provider).toBeUndefined();
    expect(u2.model).toBeUndefined();
    // Unit-scoped agent_started never lands on the step node itself.
    expect(b.provider).toBeUndefined();
    expect(b.codename).toBeUndefined();
  });

  it('agent_started arriving before unit_started still lands on the unit', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      {
        type: 'agent_started', run_id: runId(), step_id: 'b', unit_index: 1,
        agent: 'agent-b', provider: 'openai', model: 'gpt-5',
        agent_run_id: 'ar-1', transcript_path: '/tmp/t1.jsonl',
      },
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 1, unit_key: 'x', transcript_path: '/tmp/t1.jsonl', codename: 'otter-3/scout#1' },
    ];
    const u = buildRunGraphModel(g, events).nodeById('b')!.fanout!.units[0];
    expect(u.codename).toBe('otter-3/scout#1');
    expect(u.provider).toBe('openai');
    expect(u.model).toBe('gpt-5');
  });

  it('unit_started codename populates a live unit', () => {
    const g = makeGraph({ units: [cp(0)] });
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 0, unit_key: 'f0.ts', transcript_path: '/tmp/t0.jsonl', codename: 'otter-3/scout#0' },
    ];
    expect(buildRunGraphModel(g, events).nodeById('b')!.fanout!.units[0].codename).toBe('otter-3/scout#0');
  });

  it('step_results codename lands on the node; step_started overrides; agent_started sets provider/model', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: true, codename: 'otter-3/lead' }],
      steps: [STEP_A, STEP_C],
    });
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'c', kind: 'step', agent: 'agent-c', codename: 'otter-3/fixer' },
      {
        type: 'agent_started', run_id: runId(), step_id: 'c', agent: 'agent-c',
        provider: 'anthropic', model: 'claude-opus-4-7', agent_run_id: 'ar-c', transcript_path: '/tmp/c.jsonl',
      },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.codename).toBe('otter-3/lead');
    const c = model.nodeById('c')!;
    expect(c.codename).toBe('otter-3/fixer');
    expect(c.provider).toBe('anthropic');
    expect(c.model).toBe('claude-opus-4-7');
  });

  it('placed-unit agent_started without provider/model leaves them absent', () => {
    const g = makeGraph({ units: [cp(0)] });
    const events: RunEvent[] = [
      {
        type: 'agent_started', run_id: runId(), step_id: 'b', unit_index: 0,
        codename: 'otter-3/scout#0', agent: 'agent-b', agent_run_id: 'ar-0', transcript_path: '/tmp/t0.jsonl',
      },
    ];
    const u = buildRunGraphModel(g, events).nodeById('b')!.fanout!.units[0];
    expect(u.codename).toBe('otter-3/scout#0');
    expect(u.provider).toBeUndefined();
    expect(u.model).toBeUndefined();
  });
});

describe('parallel sub-step identities + unit agents', () => {
  const PAR: StepNodeDto = {
    id: 'p', kind: 'parallel',
    parallel: [{ id: 'lint', agent: 'linter' }, { id: 'test', agent: 'tester' }],
  };

  it('sub-steps carry the DAG agent, step_result item codename, and agent_started provider/model by declared position', () => {
    const g = makeGraph({
      steps: [PAR],
      step_results: [{
        run_id: runId(), step_id: 'p', success: true,
        items: [{ index: 0, sub_id: 'lint', codename: 'jade-reef/heron.a' }],
      }],
    });
    const events: RunEvent[] = [
      {
        type: 'agent_started', run_id: runId(), step_id: 'p', unit_index: 1,
        codename: 'jade-reef/lynx.b', agent: 'tester', provider: 'openai', model: 'gpt-5',
        agent_run_id: 'ar', transcript_path: '/t/b.jsonl',
      },
    ];
    const subs = buildRunGraphModel(g, events).nodeById('p')!.parallel!;
    expect(subs[0]).toMatchObject({ id: 'lint', agent: 'linter', codename: 'jade-reef/heron.a' });
    expect(subs[0].provider).toBeUndefined();
    expect(subs[1]).toMatchObject({
      id: 'test', agent: 'tester', codename: 'jade-reef/lynx.b', provider: 'openai', model: 'gpt-5',
    });
  });

  it('unit_started agent lands on UnitView.agent (panel units run their own agents)', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      { type: 'unit_started', run_id: runId(), step_id: 'b', index: 0, unit_key: 'alice', agent: 'sec-reviewer', transcript_path: '/t/0.jsonl' },
    ];
    expect(buildRunGraphModel(g, events).nodeById('b')!.fanout!.units[0].agent).toBe('sec-reviewer');
  });
});

// ---------------------------------------------------------------------------
// Server-folded identities (graph response) — survive the capped live window
// ---------------------------------------------------------------------------

describe('seeded identities from the graph response', () => {
  const PAR: StepNodeDto = {
    id: 'p', kind: 'parallel', parallel: [{ id: 'x', agent: 'ax' }, { id: 'y', agent: 'ay' }],
  };

  it('names step, unit and parallel sub-step with NO live events', () => {
    const g: RunGraphResponse = {
      ...makeGraph({
        steps: [STEP_A, STEP_B, PAR],
        units: [{
          step_id: 'b', index: 2, item: 'crates/db', run_id: 'u', transcript_path: 't', output: '',
          success: true, finished_at: 'x', codename: 'jade-reef/lynx#3',
          agent: 'agent-b', provider: 'anthropic', model: 'claude-sonnet-4-6',
        }],
      }),
      step_identities: { a: { codename: 'jade-reef/heron', agent: 'agent-a', provider: 'openai', model: 'gpt-5' } },
      unit_identities: { p: { '1': { codename: 'jade-reef/owl.b', agent: 'ay', provider: 'anthropic', model: 'opus' } } },
    };
    const model = buildRunGraphModel(g, []);
    const a = model.nodes.find((n) => n.id === 'a')!;
    expect(a).toMatchObject({ codename: 'jade-reef/heron', provider: 'openai', model: 'gpt-5' });
    const unit = model.nodes.find((n) => n.id === 'b')!.fanout!.units[0];
    expect(unit).toMatchObject({ codename: 'jade-reef/lynx#3', agent: 'agent-b', provider: 'anthropic', model: 'claude-sonnet-4-6' });
    const sub = model.nodes.find((n) => n.id === 'p')!.parallel![1];
    expect(sub).toMatchObject({ codename: 'jade-reef/owl.b', provider: 'anthropic', model: 'opus' });
  });

  it('live agent_started layers over the seed field by field (live wins)', () => {
    const g: RunGraphResponse = {
      ...makeGraph(),
      step_identities: { a: { codename: 'jade-reef/heron', agent: 'agent-a', provider: 'openai', model: 'gpt-5' } },
    };
    const events: RunEvent[] = [{
      type: 'agent_started', run_id: runId(), step_id: 'a', agent: 'agent-a',
      model: 'gpt-5.1', agent_run_id: 'ar', transcript_path: 't',
    } as RunEvent];
    const a = buildRunGraphModel(g, events).nodes.find((n) => n.id === 'a')!;
    expect(a.model).toBe('gpt-5.1');
    expect(a.provider).toBe('openai'); // absent on the live event → seed kept
    expect(a.codename).toBe('jade-reef/heron');
  });
});

// ---------------------------------------------------------------------------
// Derived (legacy, derive-on-read) codenames carry a muted flag
// ---------------------------------------------------------------------------

describe('derived codenames', () => {
  const PAR: StepNodeDto = {
    id: 'p', kind: 'parallel',
    parallel: [{ id: 'lint', agent: 'linter' }],
  };

  it('flags derived step, unit, parallel-sub and event names; stored names stay unflagged', () => {
    const g = makeGraph({
      steps: [STEP_A, STEP_B, PAR],
      step_results: [
        { run_id: runId(), step_id: 'a', success: true, codename: 'jade-reef/hedgehog', codename_derived: true },
        {
          run_id: runId(), step_id: 'p', success: true,
          items: [{ index: 0, sub_id: 'lint', codename: 'jade-reef/heron', codename_derived: true }],
        },
      ],
      units: [
        {
          step_id: 'b', index: 0, item: 'f0.ts', run_id: runId(), transcript_path: '/t/0.jsonl',
          output: '', success: true, finished_at: '2026-06-18T00:01:00Z',
          codename: 'jade-reef/numbat#1', codename_derived: true,
        },
        {
          step_id: 'b', index: 1, item: 'f1.ts', run_id: runId(), transcript_path: '/t/1.jsonl',
          output: '', success: true, finished_at: '2026-06-18T00:01:00Z',
          codename: 'jade-reef/numbat#2',
        },
      ],
    });
    const events: RunEvent[] = [
      {
        type: 'unit_started', run_id: runId(), step_id: 'b', index: 2, unit_key: 'f2.ts',
        transcript_path: '/t/2.jsonl', codename: 'jade-reef/numbat#3', codename_derived: true,
      },
    ];
    const model = buildRunGraphModel(g, events);
    expect(model.nodeById('a')!.codenameDerived).toBe(true);
    expect(model.nodeById('p')!.parallel![0].codenameDerived).toBe(true);
    const units = model.nodeById('b')!.fanout!.units;
    expect(units.find((u) => u.index === 0)!.codenameDerived).toBe(true);
    expect(units.find((u) => u.index === 1)!.codenameDerived).toBeUndefined();
    expect(units.find((u) => u.index === 2)!.codenameDerived).toBe(true);
  });

  it('a stored name arriving later clears the derived flag', () => {
    const g = makeGraph({
      step_results: [{ run_id: runId(), step_id: 'a', success: true, codename: 'jade-reef/hedgehog', codename_derived: true }],
      steps: [STEP_A],
    });
    const events: RunEvent[] = [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step', codename: 'jade-reef/hedgehog' },
    ];
    expect(buildRunGraphModel(g, events).nodeById('a')!.codenameDerived).toBeUndefined();
  });

  it('seeded step identity carries the derived flag', () => {
    const g = makeGraph({ steps: [STEP_A] });
    g.step_identities = { a: { codename: 'jade-reef/hedgehog', codename_derived: true, agent: 'agent-a' } };
    expect(buildRunGraphModel(g, []).nodeById('a')!.codenameDerived).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// In-flight work left behind by an attempt that ended (resume / pause / fail)
// ---------------------------------------------------------------------------

describe('superseded attempts', () => {
  const doneCheckpoint: UnitCheckpoint = {
    step_id: 'b', index: 0, item: 'u0',
    run_id: runId(), transcript_path: '/tmp/u0.jsonl',
    output: 'ok', success: true, finished_at: '2026-06-18T00:01:00Z',
  };
  const runStarted: RunEvent = {
    type: 'run_started', run_id: runId(), workflow_path: '/w', started_at: '2026-06-18T00:00:00Z',
  } as RunEvent;
  const started = (index: number): RunEvent => ({
    type: 'unit_started', run_id: runId(), step_id: 'b', index, unit_key: `u${index}`, transcript_path: `/tmp/u${index}.jsonl`,
  });

  it('a resume settles units the dead attempt left running back to their checkpoint (or pending)', () => {
    const g = makeGraph({ units: [doneCheckpoint] });
    const events: RunEvent[] = [
      runStarted,
      started(0), // a re-run of an already-done unit, killed mid-way
      started(1), // never finished
      runStarted, // the resume
      started(2),
    ];
    const b = buildRunGraphModel(g, events).nodeById('b')!;
    const state = (i: number) => b.fanout!.units.find((u) => u.index === i)!.state;
    expect(state(0)).toBe('done');
    expect(state(1)).toBe('pending');
    expect(state(2)).toBe('running');
    expect(b.fanout!.byState.running).toBe(1);
    expect(b.fanout!.byState.done).toBe(1);
  });

  it('a pause settles the units it interrupted', () => {
    const g = makeGraph({});
    const events: RunEvent[] = [
      runStarted,
      started(0),
      started(1),
      { type: 'unit_completed', run_id: runId(), step_id: 'b', index: 0, unit_key: 'u0', success: true, tokens_in: 1, tokens_out: 1 },
      { type: 'run_paused', run_id: runId() } as RunEvent,
    ];
    const b = buildRunGraphModel(g, events).nodeById('b')!;
    expect(b.fanout!.units.find((u) => u.index === 0)!.state).toBe('done');
    expect(b.fanout!.units.find((u) => u.index === 1)!.state).toBe('pending');
    expect(b.fanout!.byState.running).toBe(0);
  });

  it("the live attempt's in-flight units stay running", () => {
    const events: RunEvent[] = [runStarted, started(0)];
    const b = buildRunGraphModel(makeGraph({}), events).nodeById('b')!;
    expect(b.fanout!.units[0].state).toBe('running');
    expect(b.state).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Step warnings — recorded per step / unit, NEVER a status change.
// ---------------------------------------------------------------------------

describe('step_warning', () => {
  const warn = (step_id: string, message: string, index?: number): RunEvent =>
    ({
      type: 'step_warning',
      run_id: runId(),
      step_id,
      message,
      ...(index === undefined ? {} : { index }),
    }) as RunEvent;
  const unitStarted = (index: number): RunEvent => ({
    type: 'unit_started',
    run_id: runId(),
    step_id: 'b',
    index,
    unit_key: `u${index}`,
    transcript_path: `/t/${index}.jsonl`,
  });
  const unitDone = (index: number): RunEvent => ({
    type: 'unit_completed',
    run_id: runId(),
    step_id: 'b',
    index,
    unit_key: `u${index}`,
    success: true,
    tokens_in: 1,
    tokens_out: 1,
  });

  it('records a step-level warning on that step only', () => {
    const model = buildRunGraphModel(makeGraph(), [warn('a', 'host gpu-9 sent no coverage')]);
    expect(model.nodeById('a')!.warnings).toEqual([{ message: 'host gpu-9 sent no coverage' }]);
    expect(model.nodeById('b')!.warnings).toBeUndefined();
    expect(model.nodeById('c')!.warnings).toBeUndefined();
  });

  it('never changes a step status — pending stays pending, done stays done, failed stays failed', () => {
    const events: RunEvent[] = [
      { type: 'step_completed', run_id: runId(), step_id: 'a', success: true, duration_ms: 5 },
      { type: 'step_failed', run_id: runId(), step_id: 'c', error: 'boom' },
      warn('a', 'late coverage note'),
      warn('b', 'never started'),
      warn('c', 'partial coverage'),
    ];
    const model = buildRunGraphModel(makeGraph(), events);
    expect(model.nodeById('a')!.state).toBe('done');
    expect(model.nodeById('b')!.state).toBe('pending');
    expect(model.nodeById('c')!.state).toBe('failed');
    for (const id of ['a', 'b', 'c']) expect(model.nodeById(id)!.warnings).toHaveLength(1);
  });

  it('a warning on a running step leaves it running', () => {
    const model = buildRunGraphModel(makeGraph(), [
      { type: 'step_started', run_id: runId(), step_id: 'a', kind: 'step' },
      warn('a', 'still going'),
    ]);
    expect(model.nodeById('a')!.state).toBe('running');
  });

  it('attaches a unit warning to that unit and to its step, leaving unit states alone', () => {
    const model = buildRunGraphModel(makeGraph(), [
      unitStarted(0),
      unitStarted(1),
      unitDone(0),
      warn('b', 'host gpu-9 did not stream coverage', 1),
    ]);
    const b = model.nodeById('b')!;
    const u0 = b.fanout!.units.find((u) => u.index === 0)!;
    const u1 = b.fanout!.units.find((u) => u.index === 1)!;
    expect(u1.warnings).toEqual(['host gpu-9 did not stream coverage']);
    expect(u0.warnings).toBeUndefined();
    // the step aggregates every warning, tagged with its unit index
    expect(b.warnings).toEqual([{ index: 1, message: 'host gpu-9 did not stream coverage' }]);
    // statuses are exactly what the lifecycle events say
    expect(u0.state).toBe('done');
    expect(u1.state).toBe('running');
    expect(b.fanout!.byState.failed).toBe(0);
  });

  it('lands a unit warning even when it precedes that unit_started', () => {
    const model = buildRunGraphModel(makeGraph(), [warn('b', 'early', 3), unitStarted(3)]);
    const unit = model.nodeById('b')!.fanout!.units.find((u) => u.index === 3)!;
    expect(unit.warnings).toEqual(['early']);
  });

  it('keeps a unit warning on the step when the unit has no view yet', () => {
    const model = buildRunGraphModel(makeGraph(), [warn('b', 'orphan unit note', 7)]);
    expect(model.nodeById('b')!.warnings).toEqual([{ index: 7, message: 'orphan unit note' }]);
  });

  it('keeps several warnings in arrival order and drops exact duplicates', () => {
    const model = buildRunGraphModel(makeGraph(), [
      warn('a', 'first'),
      warn('a', 'second'),
      warn('a', 'first'),
    ]);
    expect(model.nodeById('a')!.warnings!.map((w) => w.message)).toEqual(['first', 'second']);
  });

  it('ignores a warning for a step that is not in the workflow', () => {
    const model = buildRunGraphModel(makeGraph(), [warn('ghost', 'who?')]);
    expect(model.nodes.every((n) => n.warnings === undefined)).toBe(true);
  });

  it('a successfully completed run keeps its warnings while promoting in-flight state', () => {
    const g = makeGraph();
    g.run.status = 'completed';
    const model = buildRunGraphModel(g, [warn('a', 'kept')]);
    expect(model.nodeById('a')!.state).toBe('done');
    expect(model.nodeById('a')!.warnings).toEqual([{ message: 'kept' }]);
  });
});

describe('collectWarnings', () => {
  it('lists every warning across steps in graph order, labelled by step and unit', () => {
    const model = buildRunGraphModel(makeGraph(), [
      { type: 'step_warning', run_id: runId(), step_id: 'c', message: 'c note' } as RunEvent,
      { type: 'step_warning', run_id: runId(), step_id: 'b', index: 2, message: 'b unit note' } as RunEvent,
      { type: 'step_warning', run_id: runId(), step_id: 'a', message: 'a note' } as RunEvent,
    ]);
    expect(collectWarnings(model)).toEqual([
      { stepId: 'a', message: 'a note' },
      { stepId: 'b', index: 2, message: 'b unit note' },
      { stepId: 'c', message: 'c note' },
    ]);
  });

  it('is empty when nothing warned', () => {
    expect(collectWarnings(buildRunGraphModel(makeGraph(), []))).toEqual([]);
  });
});
