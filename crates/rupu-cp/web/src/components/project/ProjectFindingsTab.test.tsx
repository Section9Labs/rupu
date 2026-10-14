// @vitest-environment jsdom
// ProjectFindingsTab — the findings explorer scoped to one project.
// api.getFindings is a fake server over a mixed-severity page that honours a
// lone `severity:<x>` token, as the real one does: the metric tiles + every
// row render, clicking the "High" tile asks for `severity:high` within the
// project, and clicking the "Total" tile brings every row back.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type FindingOut } from '../../lib/api';
import { findingsServer } from '../findings/findingsTestUtils';
import ProjectFindingsTab from './ProjectFindingsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function finding(id: string, severity: string, summary: string): FindingOut {
  return {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
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
const ROWS: FindingOut[] = [
  finding('f-crit', 'critical', 'Critical SQL injection'),
  finding('f-high', 'high', 'High auth bypass'),
  finding('f-med', 'medium', 'Medium info leak'),
];

describe('ProjectFindingsTab', () => {
  it('renders tiles + rows, filters by High, then restores via re-click', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockImplementation(findingsServer(ROWS));

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
    await waitFor(() => expect(spy).toHaveBeenLastCalledWith({ wsId: 'x', q: 'severity:high' }));
    await waitFor(() => expect(screen.queryByText('Critical SQL injection')).not.toBeInTheDocument());
    expect(screen.getByText('High auth bypass')).toBeInTheDocument();
    expect(screen.queryByText('Critical SQL injection')).not.toBeInTheDocument();
    expect(screen.queryByText('Medium info leak')).not.toBeInTheDocument();

    // Click the Total tile → filter clears, all rows return.
    fireEvent.click(screen.getByRole('button', { name: 'Filter by Total' }));
    await waitFor(() => expect(screen.getByText('Critical SQL injection')).toBeInTheDocument());
    expect(spy).toHaveBeenLastCalledWith({ wsId: 'x' });
    expect(screen.getByText('High auth bypass')).toBeInTheDocument();
    expect(screen.getByText('Medium info leak')).toBeInTheDocument();
  });
});

describe('ProjectFindingsTab — export report', () => {
  it('exports the severity-filtered rows, scoped to the project', async () => {
    vi.spyOn(api, 'getFindings').mockImplementation(findingsServer(ROWS.map((f) => ({ ...f, profile: 'full' as const }))));
    const exportSpy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    render(
      <MemoryRouter>
        <ProjectFindingsTab wsId="x" />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('High auth bypass')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Filter by high' }));
    await waitFor(() => expect(screen.queryByText('Critical SQL injection')).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));
    const dialog = screen.getByRole('dialog');
    expect(within(dialog).getByLabelText('Title')).toHaveValue('proj findings report');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Export' }));

    await waitFor(() => expect(exportSpy).toHaveBeenCalled());
    expect(exportSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({ ids: ['f-high'], ws_id: 'x', format: 'md' }),
    );
  });

  it('is disabled when the severity filter leaves no rows', async () => {
    vi.spyOn(api, 'getFindings').mockImplementation(findingsServer(ROWS));
    render(
      <MemoryRouter>
        <ProjectFindingsTab wsId="x" />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('High auth bypass')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Filter by low' }));
    await screen.findByText('No findings match `severity:low`.');
    expect(screen.getByRole('button', { name: 'Export report' })).toBeDisabled();
  });
});
