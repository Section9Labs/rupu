// @vitest-environment jsdom
// AutoflowRuns (Runs tab) — row actions (Archive/Delete), runs-section
// row-actions plan Task 3. Only events carrying a `run_id`
// (`kind: "run_launched"`) have addressable storage on disk — those
// delegate to the EXISTING api.archiveRun/deleteRun (they ARE genuine
// workflow runs, no new endpoint needed). Events without a `run_id`
// (awaiting_human / awaiting_external / cycle_failed) render no action
// column content at all — there is nothing to archive or delete.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AutoflowEventRow, HostView } from '../../lib/api';
import AutoflowRuns from './AutoflowRuns';

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

const RUN_EVENT: AutoflowEventRow = {
  event_id: 'evt-run-1',
  cycle_id: 'cyc-1',
  at: '2026-06-01T00:00:00Z',
  kind: 'run_launched',
  workflow: 'fix-issue',
  run_id: 'run-9',
  status: 'completed',
  host_id: 'local',
  usage: {
    input_tokens: 100,
    output_tokens: 50,
    cached_tokens: 0,
    total_tokens: 150,
    cost_usd: null,
    priced: false,
    runs: 1,
  },
};

const AWAITING_EVENT: AutoflowEventRow = {
  event_id: 'evt-await-1',
  cycle_id: 'cyc-2',
  at: '2026-06-01T00:05:00Z',
  kind: 'awaiting_human',
  workflow: 'fix-issue',
  usage: {
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    total_tokens: 0,
    cost_usd: null,
    priced: false,
    runs: 0,
  },
};

function stubPage(events: AutoflowEventRow[]) {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
  vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue(events);
  vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AutoflowRuns />
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — action rendering keyed off run_id presence', () => {
  it('a run_launched event (run_id present) renders Archive and Delete', async () => {
    stubPage([RUN_EVENT]);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    expect(screen.getByLabelText(`Archive run ${RUN_EVENT.run_id}`)).toBeInTheDocument();
    expect(screen.getByLabelText(`Delete run ${RUN_EVENT.run_id}`)).toBeInTheDocument();
  });

  it('an awaiting_human event (no run_id) renders no action button', async () => {
    stubPage([AWAITING_EVENT]);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    expect(screen.queryByText('Archive')).not.toBeInTheDocument();
    expect(screen.queryByText('Delete')).not.toBeInTheDocument();
  });

  it('mixed rows: only the run_id-bearing row gets an action, the other gets none', async () => {
    stubPage([RUN_EVENT, AWAITING_EVENT]);

    renderPage();
    await waitFor(() => expect(screen.getAllByText('fix-issue')).toHaveLength(2));

    expect(screen.getAllByLabelText(/^Archive run /)).toHaveLength(1);
    expect(screen.getAllByLabelText(/^Delete run /)).toHaveLength(1);
  });
});

describe('AutoflowRuns — Archive/Delete delegate to the EXISTING run endpoints', () => {
  it('Archive calls api.archiveRun with run_id and host', async () => {
    stubPage([RUN_EVENT]);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive run ${RUN_EVENT.run_id}`));

    await waitFor(() =>
      expect(archiveSpy).toHaveBeenCalledWith(RUN_EVENT.run_id, RUN_EVENT.host_id),
    );
  });

  it('Delete is confirm-gated and calls api.deleteRun with run_id when confirmed', async () => {
    stubPage([RUN_EVENT]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${RUN_EVENT.run_id}`));

    expect(confirmSpy).toHaveBeenCalled();
    await waitFor(() =>
      expect(deleteSpy).toHaveBeenCalledWith(RUN_EVENT.run_id, RUN_EVENT.host_id),
    );
  });

  it('Delete: stubbed confirm() false ⇒ no API call', async () => {
    stubPage([RUN_EVENT]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${RUN_EVENT.run_id}`));

    expect(deleteSpy).not.toHaveBeenCalled();
  });
});

describe('AutoflowRuns — action buttons never trigger row navigation', () => {
  it('clicking Archive prevents the enclosing rowHref default action', async () => {
    stubPage([RUN_EVENT]);
    vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const button = screen.getByLabelText(`Archive run ${RUN_EVENT.run_id}`);
    expect(button.closest('a')).toBeNull();
    const notCanceled = fireEvent.click(button);

    expect(notCanceled).toBe(false);
  });

  it('clicking Delete (confirmed) prevents the enclosing rowHref default action', async () => {
    stubPage([RUN_EVENT]);
    vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const button = screen.getByLabelText(`Delete run ${RUN_EVENT.run_id}`);
    const notCanceled = fireEvent.click(button);

    expect(notCanceled).toBe(false);
  });
});
