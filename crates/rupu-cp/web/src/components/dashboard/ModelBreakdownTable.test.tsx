// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import ModelBreakdownTable, { toRows } from './ModelBreakdownTable';
import type { UsageBreakdownRow } from '../../lib/usage';

afterEach(() => {
  cleanup();
});

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

describe('ModelBreakdownTable host pivot', () => {
  function hostRow(hostId: string, cost = 1): UsageBreakdownRow {
    return {
      provider: '',
      model: '',
      agent: '',
      workflow: '',
      host_id: hostId,
      workspace_id: '',
      input_tokens: 100,
      output_tokens: 0,
      cached_tokens: 0,
      total_tokens: 100,
      cost_usd: cost,
      priced: true,
      runs: 1,
    };
  }

  it('maps host_id to the friendly host name when the hosts list has a match', () => {
    render(
      <ModelBreakdownTable
        rows={[hostRow('host_01KWREMOTE')]}
        pivot="host"
        hosts={[
          {
            host_id: 'host_01KWREMOTE',
            name: 'staging-box',
            transport_kind: 'http_cp',
            state: 'ok',
            captured_at: null,
            reason: null,
          },
        ]}
      />,
    );
    expect(screen.getByText('staging-box')).toBeInTheDocument();
    expect(screen.queryByText('host_01KWREMOTE')).not.toBeInTheDocument();
  });

  it('falls back to the raw host id when no matching host is found', () => {
    render(<ModelBreakdownTable rows={[hostRow('host_unknown')]} pivot="host" hosts={[]} />);
    expect(screen.getByText('host_unknown')).toBeInTheDocument();
  });
});

describe('toRows with showAll (the interactive /usage table)', () => {
  it('emits one row per key with no top-N rollup', () => {
    const v = toRows(ROWS, 'model', { showAll: true });
    const models = v.rows.filter((r) => r.kind === 'model');
    const others = v.rows.filter((r) => r.kind === 'others');
    expect(models).toHaveLength(8);
    expect(others).toHaveLength(0);
    expect(models.map((r) => r.label)).toContain('m8');
  });
});

describe('ModelBreakdownTable selectable', () => {
  it('shows every row (no others rollup) when selectable, unlike the default top-6 view', () => {
    render(<ModelBreakdownTable rows={ROWS} selectable excludedKeys={new Set()} onToggleKey={() => {}} />);
    expect(screen.queryByText(/others/)).not.toBeInTheDocument();
    expect(screen.getByText('m8')).toBeInTheDocument();
  });

  it('calls onToggleKey with the row pivot key when its checkbox is clicked', () => {
    const onToggleKey = vi.fn();
    render(<ModelBreakdownTable rows={ROWS} selectable excludedKeys={new Set()} onToggleKey={onToggleKey} />);
    fireEvent.click(screen.getByRole('checkbox', { name: 'm1' }));
    expect(onToggleKey).toHaveBeenCalledWith('m1');
  });

  it('renders an excluded row unchecked and visibly muted', () => {
    render(
      <ModelBreakdownTable rows={ROWS} selectable excludedKeys={new Set(['m2'])} onToggleKey={() => {}} />,
    );
    const checkbox = screen.getByRole('checkbox', { name: 'm2' }) as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    // A non-excluded row's checkbox stays checked.
    expect((screen.getByRole('checkbox', { name: 'm1' }) as HTMLInputElement).checked).toBe(true);
  });

  it('does not render checkboxes when not selectable', () => {
    render(<ModelBreakdownTable rows={ROWS} />);
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });
});

// Regression coverage for the real-world interaction bugs: (a) stuck
// checkboxes that can never be unchecked, (b) rows mislabeled "—", (c)
// toggling a row with no effect on the graph. Root cause: a selectable
// table must show EVERY row (no top-6/others rollup — already enforced by
// `toRows(rows, pivot, { showAll: selectable })` above) and a genuinely
// empty pivot value (`rawKey === ''`, e.g. a run with a blank `agent`) is a
// REAL, toggleable group — `aggregateRuns`/`buildTimeline` group by it via
// `pivotKeyOf`, and `excludedKeys.has('')` works — so it must never be
// `disabled`, only the (non-existent-in-selectable-mode) `others` rollup may
// be.
describe('ModelBreakdownTable selectable — empty pivot key (Fix 2/3)', () => {
  it('renders an enabled, checked checkbox for an empty-key row and toggles with the empty string', () => {
    const rows = [...ROWS, row('', 2)];
    const onToggleKey = vi.fn();
    render(<ModelBreakdownTable rows={rows} selectable excludedKeys={new Set()} onToggleKey={onToggleKey} />);

    // Legible label for the empty group, distinct from the raw "—" the
    // non-selectable dashboard table / graph legend show.
    const label = screen.getByText('(unattributed)');
    const tr = label.closest('tr');
    expect(tr).not.toBeNull();
    const checkbox = within(tr as HTMLElement).getByRole('checkbox') as HTMLInputElement;

    expect(checkbox).not.toBeDisabled();
    expect(checkbox.checked).toBe(true);

    fireEvent.click(checkbox);
    expect(onToggleKey).toHaveBeenCalledWith('');
  });

  it('shows every row with no others rollup in selectable mode even with >6 priced rows', () => {
    render(<ModelBreakdownTable rows={ROWS} selectable excludedKeys={new Set()} onToggleKey={() => {}} />);
    expect(screen.queryByText(/others \(/)).not.toBeInTheDocument();
    expect(screen.getAllByRole('checkbox').length).toBe(ROWS.length);
    for (const checkbox of screen.getAllByRole('checkbox')) {
      expect(checkbox).not.toBeDisabled();
    }
  });
});
