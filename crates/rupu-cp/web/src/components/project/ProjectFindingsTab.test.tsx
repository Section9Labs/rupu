// @vitest-environment jsdom
// ProjectFindingsTab — interactive severity filter scoped to one project.
// api.getFindings is spied to return a mixed-severity page + matching summary;
// the metric tiles + every row render, clicking the "High" tile narrows the
// list to high-severity rows, and clicking the "Total" tile brings every row
// back.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type FindingOut, type FindingsResponse } from '../../lib/api';
import ProjectFindingsTab from './ProjectFindingsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function finding(id: string, severity: string, summary: string): FindingOut {
  return {
    id,
    ws_id: 'x',
    project: 'proj',
    target_id: 'tgt',
    file_path: `src/${id}.rs`,
    line_range: [1, 2],
    scope: null,
    summary,
    severity,
    concern_id: null,
    evidence: { rationale: '' },
    declared_by: null,
    declared_at: '2026-06-01T00:00:00Z',
  };
}

// Backend pre-sorts critical → info; mirror that here.
const RESP: FindingsResponse = {
  findings: [
    finding('f-crit', 'critical', 'Critical SQL injection'),
    finding('f-high', 'high', 'High auth bypass'),
    finding('f-med', 'medium', 'Medium info leak'),
  ],
  summary: { total: 3, critical: 1, high: 1, medium: 1, low: 0, info: 0 },
};

describe('ProjectFindingsTab', () => {
  it('renders tiles + rows, filters by High, then restores via re-click', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(RESP);

    render(
      <MemoryRouter>
        <ProjectFindingsTab wsId="x" />
      </MemoryRouter>,
    );

    // All three rows show once loaded.
    await waitFor(() => expect(screen.getByText('Critical SQL injection')).toBeInTheDocument());
    expect(screen.getByText('High auth bypass')).toBeInTheDocument();
    expect(screen.getByText('Medium info leak')).toBeInTheDocument();

    // Metric tiles present (Total + each severity tile is a filter button).
    // Tile aria-labels use the SEVERITY_STYLE label (lowercase); Total is literal.
    expect(screen.getByRole('button', { name: 'Filter by Total' })).toBeInTheDocument();
    const highTile = screen.getByRole('button', { name: 'Filter by high' });
    expect(highTile).toBeInTheDocument();

    // Click "high" → only the high-severity row remains.
    fireEvent.click(highTile);
    expect(screen.getByText('High auth bypass')).toBeInTheDocument();
    expect(screen.queryByText('Critical SQL injection')).not.toBeInTheDocument();
    expect(screen.queryByText('Medium info leak')).not.toBeInTheDocument();

    // Click the Total tile → filter clears, all rows return.
    fireEvent.click(screen.getByRole('button', { name: 'Filter by Total' }));
    expect(screen.getByText('Critical SQL injection')).toBeInTheDocument();
    expect(screen.getByText('High auth bypass')).toBeInTheDocument();
    expect(screen.getByText('Medium info leak')).toBeInTheDocument();
  });
});
