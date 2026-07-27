// @vitest-environment jsdom
// ProjectDefinitions — Task 4 (table-standardization plan): all three
// sub-tables (Agents, Workflows, Autoflows) adopt whole-row navigation
// (rowHref), matching the standalone Agents.tsx/AutoflowsDefs.tsx pages that
// were already row-clickable — this page was the inconsistency.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import {
  api,
  type AgentSummary,
  type AutoflowDefRow,
  type WorkflowSummary,
} from '../lib/api';
import ProjectDefinitions from './ProjectDefinitions';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

const AGENT: AgentSummary = {
  name: 'fix-bug',
  scope: 'project',
  usage: USAGE,
  run_count: 2,
};

const WORKFLOW: WorkflowSummary = {
  name: 'deploy-prod',
  scope: 'project',
  usage: USAGE,
  run_count: 1,
};

const AUTOFLOW: AutoflowDefRow = {
  name: 'Auto Deploy',
  slug: 'auto-deploy',
  trigger: 'cron',
  scope: 'project',
};

function stub() {
  vi.spyOn(api, 'getProjectAgents').mockResolvedValue([AGENT]);
  vi.spyOn(api, 'getProjectWorkflows').mockResolvedValue([WORKFLOW]);
  vi.spyOn(api, 'getProjectAutoflows').mockResolvedValue([AUTOFLOW]);
}

function renderPage(wsId = 'ws1') {
  return render(
    <MemoryRouter initialEntries={[`/projects/${wsId}/definitions`]}>
      <Routes>
        <Route path="/projects/:wsId/definitions" element={<ProjectDefinitions />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('ProjectDefinitions — Agents tab whole-row navigation', () => {
  it('renders each agent row as a link to /agents/:name', async () => {
    stub();
    renderPage();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());
    const link = screen.getByText('fix-bug').closest('a');
    expect(link).toHaveAttribute('href', '/agents/fix-bug');

    // The name cell already carried its own inline <Link> before this task —
    // prove the WHOLE row is now link-wrapped (not just that one cell) via
    // the plain, never-linked scope chip cell.
    const scopeLink = screen.getByText('project').closest('a');
    expect(scopeLink).toHaveAttribute('href', '/agents/fix-bug');
  });
});

describe('ProjectDefinitions — Workflows tab whole-row navigation', () => {
  it('renders each workflow row as a link to /workflows/:name', async () => {
    stub();
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: /Workflows/ }));
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const link = screen.getByText('deploy-prod').closest('a');
    expect(link).toHaveAttribute('href', '/workflows/deploy-prod');

    const scopeLink = screen.getByText('project').closest('a');
    expect(scopeLink).toHaveAttribute('href', '/workflows/deploy-prod');
  });
});

describe('ProjectDefinitions — Autoflows tab whole-row navigation', () => {
  it('renders each autoflow row as a link to /workflows/:slug', async () => {
    stub();
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: /Autoflows/ }));
    await waitFor(() => expect(screen.getByText('Auto Deploy')).toBeInTheDocument());

    const link = screen.getByText('Auto Deploy').closest('a');
    expect(link).toHaveAttribute('href', '/workflows/auto-deploy');

    const triggerLink = screen.getByText('cron').closest('a');
    expect(triggerLink).toHaveAttribute('href', '/workflows/auto-deploy');
  });
});
