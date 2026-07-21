// @vitest-environment jsdom
// FindingsTable location-cell deep-link — Task 9's second consumer. Unlike
// FindingRow (only rendered by RunDetail's Findings tab), FindingsTable is
// what actually backs the global Findings page, ProjectFindingsTab, and
// CoverageDetail's findings list (post-#403 SortableTable migration). A row's
// own `ws_id` (FindingOut) wins; the table-level `wsId` prop is the fallback
// used by CoverageDetail, whose findings are plain FindingRecords with no
// per-row ws_id. Absent both, the location stays a plain, non-interactive
// span (unchanged legacy behavior).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { FindingsTable } from './FindingsTable';
import type { FindingOut, FindingRecord } from '../../lib/api';

afterEach(cleanup);

function baseFinding(overrides: Partial<FindingRecord>): FindingRecord {
  return {
    id: 'f1',
    scope: null,
    summary: 's',
    severity: 'high',
    evidence: { rationale: '' },
    declared_by: null,
    declared_at: '2026-06-01T00:00:00Z',
    ...overrides,
  };
}

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

describe('FindingsTable deep-link', () => {
  it("navigates using the row's own ws_id (FindingOut) when present", () => {
    const finding: FindingOut = {
      ...baseFinding({ file_path: 'src/billing.rs', line_range: [17, 19] }),
      ws_id: 'ws-row',
      project: 'proj',
      target_id: 'tgt',
    };

    render(
      <MemoryRouter initialEntries={['/findings']}>
        <FindingsTable findings={[finding]} showProvenance />
        <LocationProbe />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('button', { name: /src\/billing\.rs/ }));
    expect(screen.getByTestId('loc')).toHaveTextContent(
      '/projects/ws-row/code?path=src%2Fbilling.rs&line=17',
    );
  });

  it('falls back to the table-level wsId when the row carries none (CoverageDetail shape)', () => {
    const finding = baseFinding({ file_path: 'src/auth.rs', line_range: [5, 6] });

    render(
      <MemoryRouter initialEntries={['/coverage/tgt']}>
        <FindingsTable findings={[finding]} wsId="ws-fallback" />
        <LocationProbe />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('button', { name: /src\/auth\.rs/ }));
    expect(screen.getByTestId('loc')).toHaveTextContent(
      '/projects/ws-fallback/code?path=src%2Fauth.rs&line=5',
    );
  });

  it('renders a plain, non-interactive location when no wsId is resolvable', () => {
    const finding = baseFinding({ file_path: 'src/no_ws.rs', line_range: [1, 2] });

    render(
      <MemoryRouter initialEntries={['/findings']}>
        <FindingsTable findings={[finding]} />
      </MemoryRouter>,
    );

    expect(screen.getByText('src/no_ws.rs:1–2')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /src\/no_ws\.rs/ })).not.toBeInTheDocument();
  });
});
