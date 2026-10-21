// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { renderHook } from '@testing-library/react';
import { cueFor, edgeTone, entranceSlots, MAX_ENTRANCE_SLOT, useStateCue } from './runMotion';
import type { StepState } from '../runGraphModel';

const pos = (x: number, y = 0) => ({ x, y, width: 170, height: 72 });

describe('entranceSlots', () => {
  it('numbers columns left to right; nodes in one column share a slot', () => {
    const slots = entranceSlots(new Map([['c', pos(500)], ['a', pos(0)], ['b1', pos(250, 0)], ['b2', pos(250, 120)]]));
    expect(Object.fromEntries(slots)).toEqual({ a: 0, b1: 1, b2: 1, c: 2 });
  });

  it('caps the slot so a wide graph never waits long for its last column', () => {
    const positions = new Map(Array.from({ length: 20 }, (_, i) => [`n${i}`, pos(i * 200)] as const));
    expect(entranceSlots(positions).get('n19')).toBe(MAX_ENTRANCE_SLOT);
  });
});

describe('cueFor', () => {
  it('plays done or failed only for a change seen live', () => {
    expect(cueFor('running', 'done')).toBe('done');
    expect(cueFor('awaiting_approval', 'failed')).toBe('failed');
    expect(cueFor(undefined, 'done')).toBeNull();
    expect(cueFor('done', 'done')).toBeNull();
    expect(cueFor('pending', 'running')).toBeNull();
  });
});

describe('useStateCue', () => {
  it('is quiet on first render and keys a new cue per landing', () => {
    const { result, rerender } = renderHook(({ s }: { s: StepState }) => useStateCue(s), {
      initialProps: { s: 'done' as StepState },
    });
    expect(result.current).toEqual({ cue: null, key: 0 });
    rerender({ s: 'running' });
    expect(result.current.cue).toBeNull();
    rerender({ s: 'done' });
    expect(result.current).toEqual({ cue: 'done', key: 1 });
    rerender({ s: 'done' });
    expect(result.current).toEqual({ cue: 'done', key: 1 });
    rerender({ s: 'running' });
    rerender({ s: 'failed' });
    expect(result.current).toEqual({ cue: 'failed', key: 2 });
  });
});

describe('edgeTone', () => {
  it('reads the edge by the state of the step it leads into', () => {
    expect(edgeTone('done', 'running')).toBe('live');
    expect(edgeTone('done', 'awaiting_approval')).toBe('await');
    expect(edgeTone('done', 'done')).toBe('lit');
    expect(edgeTone('done', 'failed')).toBe('lit');
    expect(edgeTone('done', 'pending')).toBe('idle');
    expect(edgeTone('done', 'skipped')).toBe('idle');
    expect(edgeTone(undefined, undefined)).toBe('idle');
  });

  it('an edge out of a failed step is a dead end, unless the run went on anyway', () => {
    expect(edgeTone('failed', 'pending')).toBe('dead');
    expect(edgeTone('failed', 'awaiting_approval')).toBe('dead');
    expect(edgeTone('failed', 'running')).toBe('live');
    expect(edgeTone('failed', 'done')).toBe('lit');
  });
});
