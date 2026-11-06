// UsageSummary — the spend + token footprint of the list it sits above, as a
// key/value figure strip (spec §"Summary": tabular numbers, each figure
// labelled, not a run-on sentence and not a chart).
//
// This replaces the old per-run stacked-token BarChart. One stacked bar per
// run — of which a list can hold dozens — gave no scale, no legend and no
// ordering, so it read as a wall of random bars and only duplicated the
// table's own In / Out / Cached / Cost columns. The totals, by contrast,
// answer a question the table makes you add up by hand: what has all of this
// cost, and how do the tokens split. (The project tabs already made the same
// move away from per-run bars, to a spend-over-time timeline.)
//
// Kept at its original module path and `bars` prop so the ~8 list screens that
// render it need no change; only the rendering is new. Pure presentational,
// token classes only — it sits inside each caller's existing panel.

import { formatTokens, formatCost } from '../../lib/usage';

export interface UsageBar {
  id: string;
  label: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  cost_usd: number | null;
  /** Kept for call-site compatibility; the summary has no per-item link. */
  to?: string;
}

interface Figure {
  label: string;
  value: string;
}

export default function UsageBarChart({ bars }: { bars: UsageBar[] }) {
  const totalIn = bars.reduce((a, b) => a + b.input_tokens, 0);
  const totalOut = bars.reduce((a, b) => a + b.output_tokens, 0);
  const totalCached = bars.reduce((a, b) => a + b.cached_tokens, 0);
  // Cache reads are a subset of input (rupu_providers::Usage), so the total
  // is input + output, the server's own `total_tokens` rule.
  const totalTokens = totalIn + totalOut;

  if (bars.length === 0 || totalTokens === 0) {
    return <div className="py-2 text-center text-note text-ink-mute">No token usage in this list yet</div>;
  }

  // A real $0 total must read as "$0", not as "not priced" — so cost is "—"
  // only when NOTHING in the list priced (mirrors the usage summary's rule).
  const priced = bars.filter((b) => b.cost_usd != null);
  const totalCost = priced.reduce((a, b) => a + (b.cost_usd ?? 0), 0);

  const figures: Figure[] = [
    { label: 'Spend', value: priced.length > 0 ? formatCost(totalCost) : '—' },
    { label: 'Tokens', value: formatTokens(totalTokens) },
    { label: 'In', value: formatTokens(totalIn) },
    { label: 'Out', value: formatTokens(totalOut) },
    { label: 'Cached', value: formatTokens(totalCached) },
  ];

  return (
    <dl className="flex flex-wrap gap-x-10 gap-y-3">
      {figures.map((f) => (
        <div key={f.label} className="min-w-0">
          <dt className="text-note text-ink-dim">{f.label}</dt>
          <dd className="mt-0.5 text-lead font-semibold tabular-nums text-ink">{f.value}</dd>
        </div>
      ))}
    </dl>
  );
}
