// All-models spend breakdown table. Top 6 priced models by cost, an `others (N)`
// rollup for the rest, then unpriced models pinned below a divider (cost `—`,
// never $0). Colors match the usage-timeline legend (shared `assignModelColors`).
//
// Pure presentational component (no recharts, no data fetching). The `toRows`
// transform is exported for unit testing.

import { formatCost, formatTokens, type UsageBreakdownRow } from '../../lib/usage';
import { assignModelColors, modelLabel, OTHER_COLOR } from './modelColors';

const TOP_N = 6;

/** A rendered table row — either a single model, the `others` rollup, or an
 *  unpriced model. `cost` is null for unpriced rows (rendered as `—`). */
export interface BreakdownRow {
  key: string;
  label: string;
  provider: string;
  tokens: number;
  cost: number | null;
  /** Cost share of total priced spend, 0..1. `null` for unpriced rows. */
  share: number | null;
  kind: 'model' | 'others' | 'unpriced';
}

export interface BreakdownView {
  rows: BreakdownRow[];
  totalCost: number;
  pricedTokens: number;
  unpricedTokens: number;
  hasUnpriced: boolean;
}

/** Split priced / unpriced, sort priced by cost desc, roll the tail past TOP_N
 *  into an `others (N)` row, and pin unpriced rows last. */
export function toRows(input: UsageBreakdownRow[]): BreakdownView {
  const priced = input
    .filter((r) => r.cost_usd !== null)
    .sort((a, b) => (b.cost_usd ?? 0) - (a.cost_usd ?? 0));
  const unpriced = input.filter((r) => r.cost_usd === null);

  const totalCost = priced.reduce((a, r) => a + (r.cost_usd ?? 0), 0);
  const pricedTokens = priced.reduce((a, r) => a + r.total_tokens, 0);
  const unpricedTokens = unpriced.reduce((a, r) => a + r.total_tokens, 0);
  const share = (c: number) => (totalCost > 0 ? c / totalCost : 0);

  const rows: BreakdownRow[] = [];
  const head = priced.slice(0, TOP_N);
  const tail = priced.slice(TOP_N);

  for (const r of head) {
    const cost = r.cost_usd ?? 0;
    rows.push({
      key: modelLabel(r),
      label: modelLabel(r),
      provider: r.provider,
      tokens: r.total_tokens,
      cost,
      share: share(cost),
      kind: 'model',
    });
  }

  if (tail.length > 0) {
    const cost = tail.reduce((a, r) => a + (r.cost_usd ?? 0), 0);
    rows.push({
      key: '__others__',
      label: `others (${tail.length})`,
      provider: '',
      tokens: tail.reduce((a, r) => a + r.total_tokens, 0),
      cost,
      share: share(cost),
      kind: 'others',
    });
  }

  for (const r of unpriced) {
    rows.push({
      key: `unpriced:${modelLabel(r)}`,
      label: modelLabel(r),
      provider: r.provider,
      tokens: r.total_tokens,
      cost: null,
      share: null,
      kind: 'unpriced',
    });
  }

  return { rows, totalCost, pricedTokens, unpricedTokens, hasUnpriced: unpriced.length > 0 };
}

export default function ModelBreakdownTable({ rows }: { rows: UsageBreakdownRow[] }) {
  const view = toRows(rows);
  const colors = assignModelColors(rows.map((r) => modelLabel(r)));
  const colorFor = (r: BreakdownRow) =>
    r.kind === 'others' ? OTHER_COLOR : colors.get(r.label) ?? OTHER_COLOR;

  if (view.rows.length === 0) {
    return <p className="text-xs text-ink-mute py-8 text-center">No model usage in this window</p>;
  }

  // First row of the unpriced block — render a divider above it.
  const firstUnpricedKey = view.rows.find((r) => r.kind === 'unpriced')?.key;

  return (
    <div className="flex flex-col">
      {/* table-fixed + sized number columns: with auto layout the table grew
          past `w-full` on the narrow card and the Share bar bled outside the
          card border. Fixed layout lets the Model name flex + truncate while
          the numeric/Share columns keep their width — nothing overflows. */}
      <table className="w-full table-fixed text-xs">
        <thead>
          <tr className="text-ink-mute text-meta uppercase tracking-wide">
            <th className="text-left font-medium pb-2">Model</th>
            <th className="text-right font-medium pb-2 w-14">Tokens</th>
            <th className="text-right font-medium pb-2 w-20">Cost</th>
            <th className="text-right font-medium pb-2 w-24">Share</th>
          </tr>
        </thead>
        <tbody>
          {view.rows.map((r) => (
            <tr
              key={r.key}
              className={
                r.key === firstUnpricedKey
                  ? 'border-t border-dashed border-border'
                  : undefined
              }
            >
              <td className="py-1.5 pr-2">
                <div className="flex items-center gap-2 min-w-0">
                  <span
                    className="w-2.5 h-2.5 rounded-sm shrink-0"
                    style={{ background: colorFor(r) }}
                  />
                  <span className="min-w-0">
                    <span className="text-ink font-medium truncate block">{r.label}</span>
                    {r.provider && (
                      <span className="text-ink-mute text-meta truncate block">{r.provider}</span>
                    )}
                  </span>
                </div>
              </td>
              <td className="py-1.5 text-right tabular-nums text-ink-dim">{formatTokens(r.tokens)}</td>
              <td className="py-1.5 text-right tabular-nums text-ink font-medium">
                {r.cost === null ? '—' : formatCost(r.cost)}
              </td>
              <td className="py-1.5 pl-2">
                {r.share === null ? (
                  <span className="text-ink-mute text-meta italic block text-right">unpriced</span>
                ) : (
                  <div className="flex items-center gap-1.5 justify-end">
                    <span className="text-ink-dim tabular-nums text-meta w-9 text-right">
                      {(r.share * 100).toFixed(0)}%
                    </span>
                    <span className="h-1.5 rounded-full bg-surface w-12 overflow-hidden">
                      <span
                        className="block h-full rounded-full"
                        style={{ width: `${Math.round(r.share * 100)}%`, background: colorFor(r) }}
                      />
                    </span>
                  </div>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <div className="mt-3 pt-2 border-t border-border text-note text-ink-dim tabular-nums">
        Total: <span className="text-ink font-semibold">{formatCost(view.totalCost)}</span> (priced)
        {view.hasUnpriced && (
          <>
            {' · '}
            <span className="text-ink font-semibold">{formatTokens(view.unpricedTokens)}</span> tokens unpriced
          </>
        )}
      </div>
    </div>
  );
}
