// @vitest-environment jsdom
// ProjectDetail shell — the persistent header + rollup tiles stay mounted while
// the `tab`-driven body swaps. We render the shell at two routes and assert the
// right tab body shows: at /projects/x/findings the Findings tab is active
// (findings content), at /projects/x the Overview tab is active (recent-runs
// "see all" content). The header project name shows in both.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import {
  api,
  ApiError,
  type ProjectDetail as ProjectDetailType,
  type FindingsResponse,
  type RunListRow,
  type ConfigView,
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
        <Route path="/projects/:wsId/config" element={<ProjectDetail tab={tab} />} />
      </Routes>
    </MemoryRouter>,
  );
}

// ---------------------------------------------------------------------------
// Config tab fixtures (T6) — the project-resolved `GET /api/config?project=x`
// view: `default_model` is inherited from global (not locked, so editable
// here too); `permission_mode` is locked by the GLOBAL policy, so the Config
// tab must render it read-only; `default_provider` resolves from this
// project's own `.rupu/config.toml`.
// ---------------------------------------------------------------------------

const PROJECT_CONFIG: ConfigView = {
  effective: {
    default_provider: 'anthropic',
    default_model: 'claude-sonnet-4-6',
    permission_mode: 'ask',
    log_level: null,
    providers: {},
    scm: { default: null },
    issues: { default: null },
    autoflow: {
      enabled: null,
      repo: null,
      checkout: null,
      worktree_root: null,
      permission_mode: null,
      strict_templates: null,
      max_active: null,
      cleanup_after: null,
    },
    pricing: { agents: {} },
    cp: { max_workspace_bytes: null },
  },
  provenance: {
    default_model: { source: 'global', locked: false },
    permission_mode: { source: 'global', locked: true },
    default_provider: { source: 'project', locked: false },
  },
  raw_global: 'permission_mode = "ask"\n[policy]\nlock = ["permission_mode"]\n',
  raw_project: 'default_provider = "anthropic"\n',
  cp: { max_workspace_bytes: null },
  status: { bind: '127.0.0.1:7878', token_set: false, restart_required_keys: [] },
};

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

// ---------------------------------------------------------------------------
// Config tab (T6) — per-project `.rupu/config.toml` editor, reusing the same
// Form/Raw field components as the global Settings page.
// ---------------------------------------------------------------------------

describe('ProjectDetail Config tab', () => {
  it('calls getConfig(projectId) and renders the resolved project config with provenance', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    const getConfigSpy = vi.spyOn(api, 'getConfig').mockResolvedValue(PROJECT_CONFIG);

    renderAt('/projects/x/config', 'config');

    await waitFor(() => expect(screen.getByText('Acme Service')).toBeInTheDocument());
    await waitFor(() => expect(getConfigSpy).toHaveBeenCalledWith('x'));

    const modelInput = (await screen.findByLabelText('Default model')) as HTMLInputElement;
    expect(modelInput.value).toBe('claude-sonnet-4-6');

    // default_model is inherited from the GLOBAL layer — provenance badge shows it
    // (permission_mode is also global-sourced, so at least one "global" badge renders).
    expect(screen.getAllByText('global').length).toBeGreaterThan(0);
  });

  it('renders a key locked by global policy as read-only with an enforced note and no editable control', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    vi.spyOn(api, 'getConfig').mockResolvedValue(PROJECT_CONFIG);

    renderAt('/projects/x/config', 'config');

    await screen.findByLabelText('Default model');

    // permission_mode is locked by the global policy: no select/input control...
    expect(screen.queryByLabelText('Permission mode')).not.toBeInTheDocument();
    // ...but its resolved value is still shown, read-only...
    expect(screen.getByText('ask')).toBeInTheDocument();
    // ...next to the 🔒 + enforced note.
    expect(screen.getByText(/enforced by global policy/i)).toBeInTheDocument();
  });

  it('editing an unlocked field and clicking Save posts to putProjectConfig with the project id', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    vi.spyOn(api, 'getConfig').mockResolvedValue(PROJECT_CONFIG);
    const putSpy = vi.spyOn(api, 'putProjectConfig').mockResolvedValue({ ok: true, restart_required: [] });

    renderAt('/projects/x/config', 'config');

    const modelInput = await screen.findByLabelText('Default model');
    fireEvent.change(modelInput, { target: { value: 'claude-opus-4-6' } });
    fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

    await waitFor(() => expect(putSpy).toHaveBeenCalledTimes(1));
    expect(putSpy.mock.calls[0][0]).toBe('x');
    expect(putSpy.mock.calls[0][1]).toEqual({ patch: { default_model: 'claude-opus-4-6' } });
  });

  it('clearing a field never produces a silent no-op save on the Config tab', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    vi.spyOn(api, 'getConfig').mockResolvedValue(PROJECT_CONFIG);
    const putSpy = vi.spyOn(api, 'putProjectConfig').mockResolvedValue({ ok: true, restart_required: [] });

    renderAt('/projects/x/config', 'config');

    const modelInput = (await screen.findByLabelText('Default model')) as HTMLInputElement;
    fireEvent.change(modelInput, { target: { value: '' } });

    expect(screen.queryByText(/unsaved change/i)).not.toBeInTheDocument();
    expect(modelInput.value).toBe('claude-sonnet-4-6');

    const saveButton = screen.getByRole('button', { name: /save changes/i }) as HTMLButtonElement;
    expect(saveButton.disabled).toBe(true);

    fireEvent.click(saveButton);
    await Promise.resolve();

    expect(putSpy).not.toHaveBeenCalled();
  });

  it('surfaces a 400 locked-key rejection from putProjectConfig inline', async () => {
    vi.spyOn(api, 'getProject').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getProjectAssessedPct').mockResolvedValue({ assessed_pct: null });
    vi.spyOn(api, 'getConfig').mockResolvedValue(PROJECT_CONFIG);
    vi.spyOn(api, 'putProjectConfig').mockRejectedValue(
      new ApiError(
        400,
        'key `permission_mode` is enforced by global policy',
        '{"error":"key `permission_mode` is enforced by global policy"}',
      ),
    );

    renderAt('/projects/x/config', 'config');

    const modelInput = await screen.findByLabelText('Default model');
    fireEvent.change(modelInput, { target: { value: 'claude-opus-4-6' } });
    fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/enforced by global policy/i);
  });
});
