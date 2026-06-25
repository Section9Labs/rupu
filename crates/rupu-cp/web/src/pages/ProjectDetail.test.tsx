// @vitest-environment jsdom
// ProjectDetail shell — the persistent header + rollup tiles stay mounted while
// the `tab`-driven body swaps. We render the shell at two routes and assert the
// right tab body shows: at /projects/x/findings the Findings tab is active
// (findings content), at /projects/x the Overview tab is active (recent-runs
// "see all" content). The header project name shows in both.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import {
  api,
  type ProjectDetail as ProjectDetailType,
  type FindingsResponse,
  type RunListRow,
} from '../lib/api';
import { type UsageSummary } from '../lib/usage';
import ProjectDetail, { type ProjectTab } from './ProjectDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const USAGE: UsageSummary = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  priced: true,
  runs: 0,
};

const RECENT_RUN: RunListRow = {
  id: 'run-1',
  workflow_name: 'nightly-scan',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  finished_at: '2026-06-01T00:05:00Z',
  trigger: 'cron',
  turns: 3,
  usage: USAGE,
};

const DETAIL: ProjectDetailType = {
  project: {
    ws_id: 'x',
    name: 'Acme Service',
    path: '/srv/acme',
    repo_remote: null,
    branch: null,
    created_at: '2026-05-01T00:00:00Z',
    last_run_at: null,
    usage: USAGE,
    run_count: 1,
    last_active: null,
  },
  runs: { total: 1, running: 0, by_status: {}, by_surface: { workflow: 1, autoflow: 0 } },
  sessions: { total: 0, active: 0 },
  coverage: { targets: 0, findings: 0 },
  recent_runs: [RECENT_RUN],
  usage: USAGE,
};

const FINDINGS: FindingsResponse = {
  findings: [
    {
      id: 'f-crit',
      ws_id: 'x',
      project: 'Acme Service',
      target_id: 'tgt',
      file_path: 'src/a.rs',
      line_range: [1, 2],
      scope: null,
      summary: 'Critical SQL injection',
      severity: 'critical',
      concern_id: null,
      evidence: { rationale: '' },
      declared_by: null,
      declared_at: '2026-06-01T00:00:00Z',
    },
  ],
  summary: { total: 1, critical: 1, high: 0, medium: 0, low: 0, info: 0 },
};

function renderAt(path: string, tab: ProjectTab) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/projects/:wsId" element={<ProjectDetail tab={tab} />} />
        <Route path="/projects/:wsId/findings" element={<ProjectDetail tab={tab} />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('ProjectDetail shell', () => {
  it('renders the Findings tab body at /projects/x/findings', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);

    renderAt('/projects/x/findings', 'findings');

    // Persistent header shows the project name.
    await waitFor(() => expect(screen.getByText('Acme Service')).toBeInTheDocument());

    // Findings tab content appears (the finding row summary).
    await waitFor(() =>
      expect(screen.getByText('Critical SQL injection')).toBeInTheDocument(),
    );

    // Overview-only content ("Recent runs" section header) is NOT shown.
    expect(screen.queryByText('Recent runs')).not.toBeInTheDocument();
  });

  it('renders the Overview tab body at /projects/x', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    const findingsSpy = vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);

    renderAt('/projects/x', 'overview');

    // Persistent header shows the project name.
    await waitFor(() => expect(screen.getByText('Acme Service')).toBeInTheDocument());

    // Overview content: the "Recent runs" section + its inline run row.
    expect(screen.getByText('Recent runs')).toBeInTheDocument();
    expect(screen.getByText('nightly-scan')).toBeInTheDocument();

    // The Overview tab does not fetch findings (that's the Findings tab's job).
    expect(findingsSpy).not.toHaveBeenCalled();
  });
});
