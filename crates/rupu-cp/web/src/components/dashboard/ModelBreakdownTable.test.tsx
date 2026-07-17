// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import ModelBreakdownTable, { toRows } from './ModelBreakdownTable';
import type { UsageBreakdownRow } from '../../lib/usage';

function row(model: string, cost: number | null, tokens = 1000): UsageBreakdownRow {
  return {
    provider: 'anthropic',
    model,
    agent: '',
    workflow: '',
    host_id: '',
    workspace_id: '',
    input_tokens: tokens,
    output_tokens: 0,
    cached_tokens: 0,
    total_tokens: tokens,
    cost_usd: cost,
    priced: cost !== null,
    runs: 1,
  };
}

// 8 priced (descending cost) + 1 unpriced.
const ROWS: UsageBreakdownRow[] = [
  row('m1', 8), row('m2', 7), row('m3', 6), row('m4', 5),
  row('m5', 4), row('m6', 3), row('m7', 2), row('m8', 1),
  row('m-unpriced', null, 500),
];

describe('toRows', () => {
  it('keeps the top 6 priced and rolls the rest into one others row', () => {
    const v = toRows(ROWS);
    const models = v.rows.filter((r) => r.kind === 'model');
    const others = v.rows.filter((r) => r.kind === 'others');
    expect(models).toHaveLength(6);
    expect(others).toHaveLength(1);
    expect(others[0].label).toBe('others (2)');
    // others = m7 + m8 = 2 + 1 = 3
    expect(others[0].cost).toBe(3);
  });

  it('pins unpriced rows last with a null cost (never $0)', () => {
    const v = toRows(ROWS);
    const last = v.rows[v.rows.length - 1];
    expect(last.kind).toBe('unpriced');
    expect(last.cost).toBeNull();
    expect(last.share).toBeNull();
    expect(last.tokens).toBe(500);
  });

  it('splits priced cost from unpriced tokens for the footer', () => {
    const v = toRows(ROWS);
    expect(v.totalCost).toBe(36); // 8+7+6+5+4+3+2+1
    expect(v.unpricedTokens).toBe(500);
    expect(v.hasUnpriced).toBe(true);
  });
});

describe('ModelBreakdownTable', () => {
  it('renders top-6 + others, unpriced as em-dash, and a split footer', () => {
    render(<ModelBreakdownTable rows={ROWS} />);
    expect(screen.getByText('others (2)')).toBeInTheDocument();
    // The unpriced model is shown, with an em-dash cost (not $0).
    expect(screen.getByText('m-unpriced')).toBeInTheDocument();
    expect(screen.getByText('—')).toBeInTheDocument();
    expect(screen.queryByText('$0.0000')).not.toBeInTheDocument();
    // Footer: priced total + unpriced tokens.
    expect(screen.getByText('$36.00')).toBeInTheDocument();
    expect(screen.getByText(/tokens unpriced/)).toBeInTheDocument();
    // "unpriced" share marker present.
    expect(screen.getByText('unpriced')).toBeInTheDocument();
  });

  it('renders an empty state with no rows', () => {
    render(<ModelBreakdownTable rows={[]} />);
    expect(screen.getByText(/No usage in this window/)).toBeInTheDocument();
  });
});
