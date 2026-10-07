import { describe, expect, it } from 'vitest';
import type { UsageSummary } from '../usage';
import { mergeUnpriced, mergeUsage, rollupSummaries } from './mergeUsage';

const sum = (over: Partial<UsageSummary> = {}): UsageSummary => ({
  input_tokens: 10, output_tokens: 5, cached_tokens: 2, cache_write_tokens: 1, total_tokens: 15,
  cost_usd: 1, priced: true, runs: 1, partial: false, ...over,
});

describe('rollupSummaries (port of usage::rollup)', () => {
  it('sums tokens and runs; total = input + output', () => {
    const r = rollupSummaries([sum(), sum({ input_tokens: 20, output_tokens: 10, runs: 3 })]);
    expect(r).toMatchObject({ input_tokens: 30, output_tokens: 15, cached_tokens: 4, cache_write_tokens: 2, total_tokens: 45, runs: 4 });
  });
  it('cost is null unless some host priced; priced ANDs; partial ORs', () => {
    expect(rollupSummaries([sum({ cost_usd: null }), sum({ cost_usd: null })]).cost_usd).toBeNull();
    expect(rollupSummaries([sum({ cost_usd: null }), sum({ cost_usd: 2.5 })]).cost_usd).toBe(2.5);
    expect(rollupSummaries([sum(), sum({ priced: false })]).priced).toBe(false);
    expect(rollupSummaries([sum(), sum({ partial: true })]).partial).toBe(true);
  });
  it('carries a pricing_error through, and adds none when no host has one', () => {
    expect(rollupSummaries([sum(), sum({ pricing_error: 'acme layer broken' })]).pricing_error).toBe('acme layer broken');
    expect(rollupSummaries([sum(), sum()])).not.toHaveProperty('pricing_error');
  });
  it('an empty rollup is priced, unpartial and costless (Rust parity)', () => {
    expect(rollupSummaries([])).toMatchObject({ priced: true, partial: false, cost_usd: null, runs: 0 });
  });
});

describe('mergeUnpriced (port of merge_unpriced)', () => {
  it('unions models (sorted, distinct) and sums rows', () => {
    expect(mergeUnpriced([{ models: ['b', 'a'], rows: 2 }, { models: ['a', 'c'], rows: 3 }])).toEqual({
      models: ['a', 'b', 'c'],
      rows: 5,
    });
  });
});

describe('mergeUsage', () => {
  it('merges summaries and gaps across hosts', () => {
    const m = mergeUsage([
      { summary: sum(), unpriced: { models: ['x'], rows: 1 } },
      { summary: sum(), unpriced: { models: [], rows: 0 } },
    ]);
    expect(m.summary.runs).toBe(2);
    expect(m.unpriced).toEqual({ models: ['x'], rows: 1 });
  });
});
