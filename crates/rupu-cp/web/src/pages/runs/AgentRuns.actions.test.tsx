// @vitest-environment jsdom
// AgentRuns — row actions (Archive/Restore/Delete), runs-section row-actions
// plan Task 3. Action targets are keyed off `source`:
//   - session-sourced rows act on the WHOLE session (Task 1's host-aware
//     session mutators) via `r.session_id`, never `r.run_id`.
//   - standalone rows act on the standalone transcript (Task 2's new
//     /api/transcripts routes) via `r.run_id`, using the new
//     api.archiveTranscript/deleteTranscript client methods added here.
//     There is no Restore for a standalone row — `rupu transcript restore`
//     doesn't exist.
//
// Design decision (stated in the task report too): AgentRuns has no
// active/archived scoping of its own — unlike Sessions.tsx/WorkflowRuns.tsx,
// a row here doesn't know whether its underlying session is currently
// archived. So EVERY session-sourced row renders Archive AND Restore (plus
// Delete); multiple rows sharing one session_id each act on that SAME shared
// session (the simplest option — the confirm copy already names the whole
// session).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AgentRunRow, HostView } from '../../lib/api';
import AgentRuns from './AgentRuns';

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

const REMOTE_HOST: HostView = {
  id: 'host_prod',
  name: 'prod',
  transport_kind: 'http_cp',
  status: 'online',
  active_run_count: 1,
};

const STANDALONE_ROW: AgentRunRow = {
  run_id: 'run-standalone-1',
  source: 'standalone',
  agent: 'fix-bug',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  turns: 3,
  usage: {
    input_tokens: 100,
    output_tokens: 50,
    cached_tokens: 0,
    total_tokens: 150,
    cost_usd: null,
    priced: false,
    runs: 1,
  },
  host_id: 'host_prod',
};

const SESSION_ROW: AgentRunRow = {
  run_id: 'run-session-1',
  source: 'session',
  agent: 'review-pr',
  session_id: 'sess-01HXYZ0123456789ABCDEF',
  trigger_source: 'session_turn',
  status: 'completed',
  started_at: '2026-06-02T00:00:00Z',
  turns: 5,
  usage: {
    input_tokens: 10,
    output_tokens: 5,
    cached_tokens: 0,
    total_tokens: 15,
    cost_usd: null,
    priced: false,
    runs: 1,
  },
  host_id: 'local',
};

function stubDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AgentRuns />
    </MemoryRouter>,
  );
}

// Source pill defaults to Standalone — session-sourced fixtures only show up
// once "All" (or "Session") is selected.
async function showAll() {
  fireEvent.click(await screen.findByRole('button', { name: 'All' }));
}

describe('AgentRuns — session-sourced row actions target the SESSION endpoint', () => {
  it('Archive calls api.archiveSession with session_id (not run_id) and the row host', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    const archiveSpy = vi.spyOn(api, 'archiveSession').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive session ${SESSION_ROW.session_id}`));

    await waitFor(() =>
      expect(archiveSpy).toHaveBeenCalledWith(SESSION_ROW.session_id, SESSION_ROW.host_id),
    );
  });

  it('Archive confirm copy names the session and says it covers all its turns', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    vi.spyOn(api, 'archiveSession').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive session ${SESSION_ROW.session_id}`));

    expect(confirmSpy.mock.calls[0][0]).toContain(SESSION_ROW.session_id);
    expect(confirmSpy.mock.calls[0][0]).toMatch(/all (of )?its turns/i);
  });

  it('Archive does nothing when the confirm dialog is dismissed', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    const archiveSpy = vi.spyOn(api, 'archiveSession').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive session ${SESSION_ROW.session_id}`));

    expect(archiveSpy).not.toHaveBeenCalled();
  });

  it('Restore calls api.restoreSession with session_id and the row host', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    const restoreSpy = vi.spyOn(api, 'restoreSession').mockResolvedValue(undefined);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Restore session ${SESSION_ROW.session_id}`));

    await waitFor(() =>
      expect(restoreSpy).toHaveBeenCalledWith(SESSION_ROW.session_id, SESSION_ROW.host_id),
    );
  });

  it('Delete is confirm-gated, names transcripts + "cannot be undone", and calls api.deleteSession with session_id', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    const deleteSpy = vi.spyOn(api, 'deleteSession').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete session ${SESSION_ROW.session_id}`));

    expect(confirmSpy.mock.calls[0][0]).toMatch(/transcripts/i);
    expect(confirmSpy.mock.calls[0][0]).toMatch(/cannot be undone/i);
    await waitFor(() =>
      expect(deleteSpy).toHaveBeenCalledWith(SESSION_ROW.session_id, SESSION_ROW.host_id),
    );
  });

  it('Delete: stubbed confirm() false ⇒ no API call', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);
    const deleteSpy = vi.spyOn(api, 'deleteSession').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete session ${SESSION_ROW.session_id}`));

    expect(deleteSpy).not.toHaveBeenCalled();
  });
});

describe('AgentRuns — standalone-sourced row actions target the TRANSCRIPT endpoint', () => {
  it('Archive calls api.archiveTranscript with run_id and the row host, when confirmed', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([STANDALONE_ROW]);
    const archiveSpy = vi.spyOn(api, 'archiveTranscript').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive run ${STANDALONE_ROW.run_id}`));

    await waitFor(() =>
      expect(archiveSpy).toHaveBeenCalledWith(STANDALONE_ROW.run_id, STANDALONE_ROW.host_id),
    );
  });

  // I3 (final-review finding): unlike every other destructive/hiding action
  // on this page, standalone Archive had NO confirm() — and unlike sessions
  // (whose collector scans both active + archived dirs), an archived
  // standalone run disappears from the CP entirely with no restore verb. The
  // confirm copy must say so.
  it('Archive is confirm-gated: stubbed confirm() false ⇒ no API call', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([STANDALONE_ROW]);
    const archiveSpy = vi.spyOn(api, 'archiveTranscript').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive run ${STANDALONE_ROW.run_id}`));

    expect(archiveSpy).not.toHaveBeenCalled();
    expect(confirmSpy).toHaveBeenCalledWith(
      expect.stringContaining('hidden from the CP'),
    );
  });

  it('Delete calls api.deleteTranscript with run_id when confirmed', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([STANDALONE_ROW]);
    const deleteSpy = vi.spyOn(api, 'deleteTranscript').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${STANDALONE_ROW.run_id}`));

    await waitFor(() =>
      expect(deleteSpy).toHaveBeenCalledWith(STANDALONE_ROW.run_id, STANDALONE_ROW.host_id),
    );
  });

  it('Delete: stubbed confirm() false ⇒ no API call', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([STANDALONE_ROW]);
    const deleteSpy = vi.spyOn(api, 'deleteTranscript').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Delete run ${STANDALONE_ROW.run_id}`));

    expect(deleteSpy).not.toHaveBeenCalled();
  });

  it('renders NO Restore control for a standalone row', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([STANDALONE_ROW]);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    expect(screen.queryByLabelText(`Restore run ${STANDALONE_ROW.run_id}`)).not.toBeInTheDocument();
    expect(screen.queryByText('Restore')).not.toBeInTheDocument();
  });
});

describe('AgentRuns — action buttons never trigger row navigation', () => {
  it('clicking Archive on a standalone row prevents the enclosing rowHref default action', async () => {
    stubDeps();
    const rowWithTranscript: AgentRunRow = {
      ...STANDALONE_ROW,
      transcript_path: '/rupu/agents/fix-bug/transcripts/run-standalone-1.jsonl',
    };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([rowWithTranscript]);
    vi.spyOn(api, 'archiveTranscript').mockResolvedValue(undefined);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const button = screen.getByLabelText(`Archive run ${STANDALONE_ROW.run_id}`);
    // sanity: the action column is `interactive`, so SortableTable renders
    // it as a plain cell — no enclosing <a> to nest inside.
    expect(button.closest('a')).toBeNull();
    const notCanceled = fireEvent.click(button);

    // dispatchEvent (what fireEvent.click returns) is false iff
    // preventDefault was called on a cancelable event in the dispatch path.
    expect(notCanceled).toBe(false);
  });

  it('clicking Delete (confirmed) on a session row prevents the enclosing rowHref default action', async () => {
    stubDeps();
    const rowWithTranscript: AgentRunRow = {
      ...SESSION_ROW,
      transcript_path: '/rupu/agents/review-pr/transcripts/run-session-1.jsonl',
    };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([rowWithTranscript]);
    vi.spyOn(api, 'deleteSession').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await showAll();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    const button = screen.getByLabelText(`Delete session ${SESSION_ROW.session_id}`);
    const notCanceled = fireEvent.click(button);

    expect(notCanceled).toBe(false);
  });
});

describe('AgentRuns — shared session_id across multiple rows', () => {
  it('two rows sharing one session_id both render actions that target that SAME session', async () => {
    stubDeps();
    const secondTurn: AgentRunRow = {
      ...SESSION_ROW,
      run_id: 'run-session-2',
      started_at: '2026-06-03T00:00:00Z',
    };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW, secondTurn]);
    const archiveSpy = vi.spyOn(api, 'archiveSession').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    await showAll();
    await waitFor(() =>
      expect(screen.getAllByLabelText(`Archive session ${SESSION_ROW.session_id}`)).toHaveLength(2),
    );

    const buttons = screen.getAllByLabelText(`Archive session ${SESSION_ROW.session_id}`);
    fireEvent.click(buttons[0]);
    fireEvent.click(buttons[1]);

    expect(archiveSpy).toHaveBeenCalledTimes(2);
    expect(archiveSpy).toHaveBeenNthCalledWith(1, SESSION_ROW.session_id, SESSION_ROW.host_id);
    expect(archiveSpy).toHaveBeenNthCalledWith(2, SESSION_ROW.session_id, SESSION_ROW.host_id);
  });
});
