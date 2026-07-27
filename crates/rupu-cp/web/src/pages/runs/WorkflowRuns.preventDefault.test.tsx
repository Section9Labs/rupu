// @vitest-environment jsdom
// WorkflowRuns — action-column preventDefault bug (table-standardization
// Task 5 follow-up from Task 4). The action column's buttons only called
// `stopPropagation()`, not `preventDefault()`.
//
// A later a11y fix (SortableTable's `interactive` columns, final re-review)
// changed how this cell is wrapped: `interactive` columns now render as a
// plain, unwrapped `<td>` (never link-wrapped) instead of a mouse-only `<a>`
// like every other non-subject cell — so the Archive/Delete buttons here are
// no longer nested inside an anchor at all, and there is no enclosing <a>
// whose default navigation to worry about. The `preventDefault()` call is
// still present in the button handlers (harmless, and still exercised by
// this test) and this test still proves it fires; it's just no longer load
// bearing for row navigation, since the interactive cell was never wrapped
// in the first place.
//
// jsdom doesn't perform real cross-document navigation (it logs "Not
// implemented: navigation to another Document" and leaves `window.location`
// untouched either way), and `MemoryRouter`'s own location only changes via
// React Router's `<Link>` onClick — which a plain `stopPropagation()` in a
// descendant handler already prevents from firing in React's synthetic
// event system, regardless of whether `preventDefault()` was ALSO called.
// So neither `window.location` nor `useLocation()` distinguishes the fixed
// code from the buggy code here. The one signal that does: per the DOM
// spec, `EventTarget.dispatchEvent` (what `fireEvent.click` returns) yields
// `false` iff some handler in the dispatch path called `preventDefault()`
// on a cancelable event. That is exactly the mechanism the browser consults
// before running the anchor's default navigation — so asserting the click
// returns `false` is a direct, environment-independent proof that the
// bug's fix (`preventDefault()`) fired.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { HostView, RunListRow } from '../../lib/api';
import WorkflowRuns from './WorkflowRuns';

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

function stubDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
}

function makeRun(overrides: Partial<RunListRow>): RunListRow {
  return {
    id: 'run_1',
    workflow_name: 'deploy-prod',
    status: 'completed',
    started_at: '2026-07-20T10:00:00Z',
    finished_at: '2026-07-20T10:05:00Z',
    trigger: 'manual',
    turns: 3,
    duration_ms: 300_000,
    usage: {
      input_tokens: 1000,
      output_tokens: 500,
      cached_tokens: 0,
      total_tokens: 1500,
      cost_usd: 0.12,
      priced: true,
      runs: 1,
    },
    host_id: 'local',
    ...overrides,
  };
}

describe('WorkflowRuns — action buttons call preventDefault (not just stopPropagation)', () => {
  it('clicking Archive prevents the enclosing rowHref <a> from following its default action', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);

    render(
      <MemoryRouter initialEntries={['/runs']}>
        <WorkflowRuns />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const button = screen.getByLabelText('Archive run run_x');
    // sanity: the action column is `interactive`, so SortableTable renders
    // it as a plain cell — no enclosing <a> to nest inside.
    expect(button.closest('a')).toBeNull();
    const notCanceled = fireEvent.click(button);

    // dispatchEvent (what fireEvent.click returns) is false iff preventDefault
    // was called on a cancelable event somewhere in the dispatch path.
    expect(notCanceled).toBe(false);
  });

  it('clicking Delete (confirmed) prevents the enclosing rowHref <a> from following its default action', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/runs']}>
        <WorkflowRuns />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const button = screen.getByLabelText('Delete run run_x');
    const notCanceled = fireEvent.click(button);

    expect(notCanceled).toBe(false);
  });
});
