import { describe, expect, it } from 'vitest';
import type { StreamCard } from '../situationRoom/cards';
import {
  ctxOf,
  deckBrief,
  deckOf,
  gateOutcomes,
  groupByBucket,
  isRaised,
  markOf,
  needsYou,
  relAge,
  rowText,
  BUCKET_MS,
  NEEDS_YOU_WINDOW_MS,
} from './rail';

const NOW = 1_800_000_000_000;
let n = 0;
function card(p: Partial<StreamCard>): StreamCard {
  return { key: `k${n++}`, ts: NOW, form: 'activity', group: 'activity', accent: 'brand', badge: 'Working', title: 't', ...p };
}

describe('row anatomy', () => {
  it('raises findings, gates and failures; the rest are lines', () => {
    expect(isRaised(card({ form: 'finding' }))).toBe(true);
    expect(isRaised(card({ form: 'await' }))).toBe(true);
    expect(isRaised(card({ form: 'error' }))).toBe(true);
    expect(isRaised(card({ form: 'warning' }))).toBe(false);
    expect(isRaised(card({ form: 'complete' }))).toBe(false);
  });

  it('marks a finding by severity and a failed completion as failed', () => {
    expect(markOf(card({ form: 'finding', severity: 'critical' }))).toBe('critical');
    expect(markOf(card({ form: 'complete', accent: 'error' }))).toBe('failed');
    expect(markOf(card({ form: 'complete' }))).toBe('done');
    expect(markOf(card({ form: 'warning' }))).toBe('warning');
  });

  it('reads each kind as a kind word, a lowercase headline and a meta', () => {
    expect(rowText(card({ badge: 'Working', detail: 'Reading src/a.rs' }))).toMatchObject({ kw: 'working', headline: 'Reading src/a.rs' });
    expect(rowText(card({ badge: 'Agent' }))).toMatchObject({ kw: 'agent', headline: 'agent started' });
    expect(rowText(card({ badge: 'Scanning', title: 'scan' }))).toMatchObject({ kw: 'step', headline: 'scan started' });
    expect(rowText(card({ form: 'complete', badge: 'Step done', title: 'plan', durationMs: 62_000 }))).toEqual({
      kw: 'done',
      headline: 'plan finished',
      meta: '1m 02s',
    });
    expect(rowText(card({ form: 'complete', badge: 'Unit done', tokensIn: 12_100, tokensOut: 2_300 }))).toMatchObject({
      headline: 'unit finished',
      meta: '12.1k → 2.3k tok',
    });
    expect(rowText(card({ form: 'complete', badge: 'Run failed', title: 'Run failed', accent: 'error' }))).toMatchObject({
      kw: 'failed',
      headline: 'run failed',
    });
    expect(rowText(card({ form: 'lifecycle', badge: 'Run started', detail: '/x/.rupu/workflows/nightly.yaml' }))).toMatchObject({
      kw: 'run',
      headline: 'run started',
      note: 'nightly.yaml',
    });
    expect(rowText(card({ form: 'panel', badge: 'Panel round', round: { n: 2, max: 5 } }))).toMatchObject({ kw: 'panel', headline: 'round 2/5' });
    expect(rowText(card({ form: 'warning', badge: 'Warning', detail: 'host gpu-9 sent no coverage stream' }))).toMatchObject({
      kw: 'warning',
      headline: 'host gpu-9 sent no coverage stream',
    });
  });
});

describe('context', () => {
  const c = card({
    runId: 'run_1',
    stepId: 'scan',
    stepKind: 'for_each',
    unitKey: '3/8',
    agent: 'recon',
    codename: 'cobalt-harbor/heron#4',
    projectName: 'oracle-api',
    detail: 'Reading x',
  });

  it('names project, workflow, step and unit, agent, and links the run on Live Events', () => {
    const parts = ctxOf(c, 'live', { workflow: 'nightly' });
    expect(parts.map((p) => p.text)).toEqual(['oracle-api', 'nightly', 'for_each scan [3/8]', 'recon', 'cobalt-harbor/heron#4']);
    expect(parts[4]).toMatchObject({ codename: { crew: 'cobalt-harbor', leaf: 'heron#4' }, href: '/runs/run_1' });
  });

  it('drops the run on its own tab, keeping the leaf', () => {
    const parts = ctxOf(c, 'run');
    expect(parts.map((p) => p.text)).toEqual(['for_each scan [3/8]', 'recon', 'cobalt-harbor/heron#4']);
    expect(parts[2].codename).toEqual({ crew: undefined, leaf: 'heron#4', derived: undefined });
    expect(parts[2].href).toBeUndefined();
  });

  it('does not repeat a step the headline already names', () => {
    const started = card({ badge: 'Scanning', title: 'scan', stepId: 'scan', runId: 'r' });
    expect(ctxOf(started, 'run').map((p) => p.text)).toEqual([]);
  });
});

describe('gates', () => {
  it('a gate is open until its run says something newer about its step or the run', () => {
    const gate = card({ form: 'await', runId: 'r', stepId: 'ship' });
    const sibling = card({ runId: 'r', stepId: 'other', ts: NOW + 1 });
    expect(gateOutcomes([sibling, gate]).get(gate.key)).toBe('open');
    const moved = card({ form: 'complete', badge: 'Step done', runId: 'r', stepId: 'ship', ts: NOW + 2 });
    expect(gateOutcomes([moved, sibling, gate]).get(gate.key)).toBe('approved');
    const rejected = card({ form: 'complete', badge: 'Run rejected', title: 'Run rejected', runId: 'r', ts: NOW + 3 });
    expect(gateOutcomes([rejected, gate]).get(gate.key)).toBe('rejected');
  });

  it('a run the page knows ended closes its gates', () => {
    const gate = card({ form: 'await', runId: 'r', stepId: 'ship' });
    expect(gateOutcomes([gate], () => 'rejected').get(gate.key)).toBe('rejected');
    expect(gateOutcomes([gate], () => 'completed').get(gate.key)).toBe('approved');
  });
});

describe('the Needs-you deck', () => {
  const gate = card({ form: 'await', runId: 'r', stepId: 'ship', ts: NOW - 10 });
  const fail = card({ form: 'error', ts: NOW - 5 });
  const crit = card({ form: 'finding', severity: 'critical', ts: NOW - 1 });
  const high = card({ form: 'finding', severity: 'high', ts: NOW });
  const med = card({ form: 'finding', severity: 'medium', ts: NOW });
  const list = [high, med, crit, fail, gate];
  const gates = gateOutcomes(list);
  const base = { now: NOW, isSeen: () => false, gates };

  it('pins gates, failures, critical and high, most urgent first', () => {
    expect(deckOf(list, base).map((c) => c.key)).toEqual([gate.key, fail.key, crit.key, high.key]);
    expect(deckBrief(deckOf(list, base))).toBe('1 gate · 1 failure · 2 findings');
  });

  it('lets go of what was seen, answered here, or is older than a day', () => {
    expect(needsYou(fail, { ...base, isSeen: (k) => k === fail.key })).toBe(false);
    expect(needsYou(gate, { ...base, answered: () => true })).toBe(false);
    expect(needsYou(fail, { ...base, now: NOW + NEEDS_YOU_WINDOW_MS + 10 })).toBe(false);
    // a run's own events carry no time: they stay pinned until seen
    expect(needsYou(card({ form: 'error', ts: 0 }), base)).toBe(true);
  });
});

describe('time', () => {
  it('groups consecutive cards by five-minute bucket, untimed ones with their neighbours', () => {
    const b = Math.floor(NOW / BUCKET_MS) * BUCKET_MS;
    const a1 = card({ ts: b + 60_000 });
    const a2 = card({ ts: 0 });
    const a3 = card({ ts: b + 1 });
    const older = card({ ts: b - 1 });
    const groups = groupByBucket([a1, a2, a3, older]);
    expect(groups.map((g) => g.cards.length)).toEqual([3, 1]);
    expect(groups[0]).toMatchObject({ bucket: b, newest: b + 60_000 });
    expect(groups[1].bucket).toBe(b - BUCKET_MS);
  });

  it('says how long ago', () => {
    expect(relAge(10_000)).toBe('just now');
    expect(relAge(4 * 60_000)).toBe('4 min ago');
    expect(relAge(2 * 3_600_000)).toBe('2 h ago');
    expect(relAge(3 * 86_400_000)).toBe('3 d ago');
  });
});
