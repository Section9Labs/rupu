// Spend breakdown table for the active `/usage` pivot. Top 6 priced rows by
// cost, an `others (N)` rollup for the rest, then unpriced rows pinned below a
// divider (cost `—`, never $0).
//
// `pivot` (default `'model'`, preserving this component's original
// model-only behavior) selects both the row label (`pivotLabel`) and the
// color source: the model pivot keeps the dedicated `modelColors.ts` palette
// (those colors are model IDENTITY, not an arbitrary category), every other
// pivot uses the themed categorical ramp from `pivotColors.ts`.
//
// Pure presentational component (no recharts, no data fetching). The `toRows`
// transform is exported for unit testing.

import { formatCost, formatTokens, type UsageBreakdownRow } from '../../lib/usage';
import type { Pivot } from '../../lib/api';
import { useThemeColors } from '../../lib/useThemeColors';
import { assignModelColors, pivotLabel, OTHER_COLOR } from './modelColors';
import { assignCategoricalColors } from '../usage/pivotColors';
import { PIVOT_LABEL } from '../usage/PivotPicker';

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
 *  into an `others (N)` row, and pin unpriced rows last. `pivot` (default
 *  `'model'`) selects the row label; defaulting preserves this function's
 *  original behavior for every existing call site. */
export function toRows(input: UsageBreakdownRow[], pivot: Pivot = 'model'): BreakdownView {
  const priced = input
    .filter((r) => r.cost_usd !== null)
    .sort((a, b) => (b.cost_usd ?? 0) - (a.cost_usd ?? 0));
  const unpriced = input.filter((r) => r.cost_usd === null);

  const totalCost = priced.reduce((a, r) => a + (r.cost_usd ?? 0), 0);
  const pricedTokens = priced.reduce((a, r) => a + r.total_tokens, 0);
  const unpricedTokens = unpriced.reduce((a, r) => a + r.total_tokens, 0);
  const share = (c: number) => (totalCost > 0 ? c / totalCost : 0);

  // The provider sub-label under a row's name is only meaningful for the
  // model pivot (which model belongs to which provider) — for every other
  // pivot the label itself already IS `r.provider` or something orthogonal
  // to it, and showing it again would just duplicate or confuse.
  const subLabel = (r: UsageBreakdownRow) => (pivot === 'model' ? r.provider : '');

  const rows: BreakdownRow[] = [];
  const head = priced.slice(0, TOP_N);
  const tail = priced.slice(TOP_N);

  for (const r of head) {
    const cost = r.cost_usd ?? 0;
    rows.push({
      key: pivotLabel(r, pivot),
      label: pivotLabel(r, pivot),
      provider: subLabel(r),
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
      key: `unpriced:${pivotLabel(r, pivot)}`,
      label: pivotLabel(r, pivot),
      provider: subLabel(r),
      tokens: r.total_tokens,
      cost: null,
      share: null,
      kind: 'unpriced',
    });
  }

  return { rows, totalCost, pricedTokens, unpricedTokens, hasUnpriced: unpriced.length > 0 };
}

export default function ModelBreakdownTable({
  rows,
  pivot = 'model',
}: {
  rows: UsageBreakdownRow[];
  pivot?: Pivot;
}) {
  const theme = useThemeColors();
  const view = toRows(rows, pivot);
  const labels = rows.map((r) => pivotLabel(r, pivot));
  const colors = pivot === 'model' ? assignModelColors(labels) : assignCategoricalColors(labels, theme);
  const colorFor = (r: BreakdownRow) =>
    r.kind === 'others' ? OTHER_COLOR : colors.get(r.label) ?? OTHER_COLOR;

  if (view.rows.length === 0) {
    return <p className="text-xs text-ink-mute py-8 text-center">No usage in this window</p>;
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
            <th className="text-left font-medium pb-2">{PIVOT_LABEL[pivot]}</th>
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
