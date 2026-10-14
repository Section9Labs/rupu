// @vitest-environment jsdom
// AutoflowRuns — row actions (Archive / Delete). A run row is a genuine
// workflow run with storage on disk and delegates to the EXISTING
// api.archiveRun / deleteRun on its own host. An attempt row is a history
// event, not a run record: it renders no action at all.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AutoflowRunRow, HostView } from '../../lib/api';
import AutoflowRuns from './AutoflowRuns';
import { withCustomerScope } from '../../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};

const RUN: AutoflowRunRow = {
  id: 'run-9',
  workflow_name: 'fix-issue',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  finished_at: '2026-06-01T00:05:00Z',
  trigger: 'event',
  turns: 3,
  duration_ms: 300_000,
  codename: 'cobalt-harbor/heron',
  codename_derived: false,
  host_id: 'local',
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  autoflow: { issue_ref: 'github:acme/widgets/issues/42' },
};

const ATTEMPT: AutoflowRunRow = {
  ...RUN,
  id: 'afe_1',
  status: 'failed',
  started_at: '2026-06-01T00:10:00Z',
  finished_at: '2026-06-01T00:10:00Z',
  turns: 0,
  duration_ms: null,
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
  attempt: { detail: 'dispatch failed', cycle_id: 'cyc_1', worker: 'local', claim_status: 'retry_backoff' },
};

function stubPage(rows: AutoflowRunRow[]) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
  vi.spyOn(api, 'getAutoflowRunList').mockResolvedValue(rows);
  vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      {withCustomerScope(<AutoflowRuns />)}
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — actions on run rows only', () => {
  it('a run row renders Archive and Delete', async () => {
    stubPage([RUN]);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    expect(screen.getByLabelText(`Archive run ${RUN.id}`)).toBeInTheDocument();
    expect(screen.getByLabelText(`Delete run ${RUN.id}`)).toBeInTheDocument();
  });

  it('an attempt row renders no action button', async () => {
    stubPage([ATTEMPT]);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    expect(screen.queryByText('Archive')).not.toBeInTheDocument();
    expect(screen.queryByText('Delete')).not.toBeInTheDocument();
  });

  it('mixed rows: only the run row gets an action', async () => {
    stubPage([RUN, ATTEMPT]);
    renderPage();
    await waitFor(() => expect(screen.getAllByText('fix-issue')).toHaveLength(2));

    expect(screen.getAllByLabelText(/^Archive run /)).toHaveLength(1);
    expect(screen.getAllByLabelText(/^Delete run /)).toHaveLength(1);
  });
});

describe('AutoflowRuns — Archive/Delete delegate to the EXISTING run endpoints', () => {
  it('Archive calls api.archiveRun with the run id and its host', async () => {
    stubPage([RUN]);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive run ${RUN.id}`));
    await waitFor(() => expect(archiveSpy).toHaveBeenCalledWith(RUN.id, RUN.host_id));
  });

  it('Delete is confirm-gated and calls api.deleteRun when confirmed', async () => {
    stubPage([RUN]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${RUN.id}`));
    expect(confirmSpy).toHaveBeenCalled();
    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith(RUN.id, RUN.host_id));
  });

  it('Delete: confirm() false ⇒ no API call', async () => {
    stubPage([RUN]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${RUN.id}`));
    expect(deleteSpy).not.toHaveBeenCalled();
  });
});

describe('AutoflowRuns — action buttons never trigger row navigation', () => {
  it('clicking Archive is outside the row link and cancels the default action', async () => {
    stubPage([RUN]);
    vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const button = screen.getByLabelText(`Archive run ${RUN.id}`);
    expect(button.closest('a')).toBeNull();
    expect(fireEvent.click(button)).toBe(false);
  });

  it('clicking Delete (confirmed) cancels the default action', async () => {
    stubPage([RUN]);
    vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    expect(fireEvent.click(screen.getByLabelText(`Delete run ${RUN.id}`))).toBe(false);
  });
});
