// Token + cost types (mirror of the rupu-cp `usage` DTOs) and dependency-free
// formatters. No price logic lives here — the backend computes all cost; this
// only formats numbers for display.

export interface UsageSummary {
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  /** null when no contributing model was priced (a partial total when `priced` is false). */
  cost_usd: number | null;
  /** false when at least one contributing model lacked a price. */
  priced: boolean;
  runs: number;
}

export interface UsageBreakdownRow {
  provider: string;
  model: string;
  agent: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  cost_usd: number | null;
  priced: boolean;
  runs: number;
}

export interface UsageOverview {
  summary: UsageSummary;
  breakdown: UsageBreakdownRow[];
}

/** One time bucket of the usage timeline — a `YYYY-MM-DD` key (the day, or the
 *  ISO-Monday for week buckets) plus the per-model breakdown of every run whose
 *  `started_at` fell in that bucket. Mirrors rupu-cp's `UsageTimelineBucket`. */
export interface UsageTimelineBucket {
  bucket: string;
  rows: UsageBreakdownRow[];
}

/** Compact a token count consistently from ≥10k so columns don't mix
 *  `950,000` and `1.2M`: `4,210` / `50k` / `1.2M` / `3.4B`. Below 10k the raw
 *  grouped number stays legible; at/above 10k we switch to a compact suffix. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1_000)}k`;
  return n.toLocaleString('en-US');
}

/** Format a USD cost. `null` → em-dash. Sub-dollar amounts get 4 decimals
 *  (small per-run costs stay legible); larger amounts get 2. */
export function formatCost(cost: number | null): string {
  if (cost === null || cost === undefined) return '—';
  return `$${cost.toFixed(cost < 1 ? 4 : 2)}`;
}
