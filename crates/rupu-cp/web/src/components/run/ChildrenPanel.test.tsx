// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { RunChild } from '../../lib/api';
import ChildrenPanel from './ChildrenPanel';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function child(over: Partial<RunChild> & Pick<RunChild, 'run_id' | 'parent_run_id'>): RunChild {
  return {
    handle: `ch_${over.run_id}`,
    kind: 'subagent',
    name: 'recon',
    depth: 1,
    codename: null,
    address: null,
    status: 'done',
    success: true,
    error: null,
    tokens: null,
    duration_ms: null,
    started_at: null,
    finished_at: null,
    transcript_path: null,
    legacy: false,
    ...over,
  };
}

function stubFetch(...bodies: RunChild[][]) {
  let i = 0;
  const fetchMock = vi.fn(async (_path: string, _init?: RequestInit) => {
    const body = bodies[Math.min(i++, bodies.length - 1)];
    return new Response(JSON.stringify({ children: body }), { status: 200 });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function renderPanel(onCount?: (n: number) => void) {
  return render(
    <MemoryRouter>
      <ChildrenPanel runId="run_root" onCount={onCount} />
    </MemoryRouter>,
  );
}

/** Each body row's cell texts, top to bottom. */
function rowTexts(): string[] {
  const table = screen.getByRole('table');
  return within(table)
    .getAllByRole('row')
    .slice(1)
    .map((r) => r.textContent ?? '');
}

describe('ChildrenPanel', () => {
  it('nests each child right after the run that launched it, and links each kind to its view', async () => {
    // The server's order: breadth-first by parent.
    stubFetch([
      child({ run_id: 'run_agent', parent_run_id: 'run_root', kind: 'agent', name: 'scanner', transcript_path: '/t/run_agent.jsonl', tokens: { input: 1200, output: 34 }, duration_ms: 2500 }),
      child({ run_id: 'run_wf', parent_run_id: 'run_root', kind: 'workflow', name: 'triage', status: 'running', success: null }),
      child({ run_id: 'sub_old', parent_run_id: 'run_root', name: 'legacy-helper', legacy: true, transcript_path: '/r/run_root/sub/sub_old/transcript.jsonl' }),
      child({ run_id: 'sub_grand', parent_run_id: 'run_agent', name: 'grandchild', depth: 2, status: 'abandoned', success: null, transcript_path: '/r/sub_grand.jsonl' }),
    ]);
    renderPanel();

    await screen.findByTestId('children-panel');
    const rows = rowTexts();
    expect(rows.map((t) => /scanner|triage|legacy-helper|grandchild/.exec(t)?.[0])).toEqual([
      'scanner',
      'grandchild',
      'triage',
      'legacy-helper',
    ]);
    expect(rows[1]).toContain('↳');
    expect(rows[0]).not.toContain('↳');
    expect(rows[0]).toContain('1,200 / 34');
    expect(rows[0]).toContain('2.5s');
    expect(rows[1]).toContain('abandoned');
    expect(rows[2]).toContain('workflow');
    expect(rows[3]).toContain('legacy');

    const hrefs = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(hrefs).toEqual([
      '/transcript?path=%2Ft%2Frun_agent.jsonl&live=0',
      '/transcript?path=%2Fr%2Fsub_grand.jsonl&live=0',
      '/runs/run_wf',
      '/transcript?path=%2Fr%2Frun_root%2Fsub%2Fsub_old%2Ftranscript.jsonl&live=0',
    ]);
  });

  it('shows a child its parent\'s pause or crash interrupted as interrupted, and does not poll for it', async () => {
    vi.useFakeTimers();
    const fetchMock = stubFetch([
      child({ run_id: 'run_a', parent_run_id: 'run_root', kind: 'agent', status: 'interrupted', success: null }),
    ]);
    renderPanel();
    await act(async () => {});
    const badge = within(screen.getByRole('table')).getByText('interrupted');
    expect(badge).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('says so when the run launched none', async () => {
    const onCount = vi.fn();
    stubFetch([]);
    renderPanel(onCount);
    expect(await screen.findByText('No children — this run launched none.')).toBeInTheDocument();
    expect(onCount).toHaveBeenCalledWith(0);
  });

  it('polls every 3 s while a child is unfinished, stops once all are, and aborts on unmount', async () => {
    vi.useFakeTimers();
    const running = [child({ run_id: 'run_a', parent_run_id: 'run_root', status: 'running', success: null })];
    const done = [child({ run_id: 'run_a', parent_run_id: 'run_root' })];
    const fetchMock = stubFetch(running, done);
    const { unmount } = renderPanel();

    await act(async () => {});
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/runs/run_root/children');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(screen.getByText('Completed')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);

    const signal = fetchMock.mock.calls[0][1]?.signal;
    unmount();
    expect(signal?.aborted).toBe(true);
  });
});
