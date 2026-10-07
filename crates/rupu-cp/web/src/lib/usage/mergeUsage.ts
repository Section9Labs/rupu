// Client-side merge of per-host `/api/usage` answers (spec 2026-10-01 §6.5).
// Ports of `rollup` (rupu-cp/src/usage.rs) and `merge_unpriced`
// (rupu-cp/src/api/usage.rs) — keep the two in step.

import type { UsageResponse } from '../api';
import type { UnpricedGap, UsageSummary } from '../usage';

export function rollupSummaries(list: readonly UsageSummary[]): UsageSummary {
  let input = 0;
  let output = 0;
  let cached = 0;
  let cacheWrite = 0;
  let runs = 0;
  let anyCost = false;
  let cost = 0;
  let priced = true;
  let partial = false;
  let pricingError: string | undefined;
  for (const s of list) {
    input += s.input_tokens;
    output += s.output_tokens;
    cached += s.cached_tokens;
    cacheWrite += s.cache_write_tokens ?? 0;
    runs += s.runs;
    if (s.cost_usd != null) {
      anyCost = true;
      cost += s.cost_usd;
    }
    if (!s.priced) priced = false;
    if (s.partial) partial = true;
    pricingError ??= s.pricing_error;
  }
  return {
    input_tokens: input,
    output_tokens: output,
    cached_tokens: cached,
    cache_write_tokens: cacheWrite,
    total_tokens: input + output,
    cost_usd: anyCost ? cost : null,
    priced,
    runs,
    partial,
    ...(pricingError ? { pricing_error: pricingError } : {}),
  };
}

export function mergeUnpriced(gaps: readonly UnpricedGap[]): UnpricedGap {
  const models = new Set<string>();
  let rows = 0;
  for (const g of gaps) {
    for (const m of g.models) models.add(m);
    rows += g.rows;
  }
  return { models: [...models].sort(), rows };
}

export interface MergedUsage {
  summary: UsageSummary;
  unpriced: UnpricedGap;
}

export function mergeUsage(responses: readonly Pick<UsageResponse, 'summary' | 'unpriced'>[]): MergedUsage {
  return {
    summary: rollupSummaries(responses.map((r) => r.summary)),
    unpriced: mergeUnpriced(responses.map((r) => r.unpriced)),
  };
}
