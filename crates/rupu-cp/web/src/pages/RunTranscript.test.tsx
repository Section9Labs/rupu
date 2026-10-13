// @vitest-environment jsdom
/**
 * The transcript page's job is to turn query params into TranscriptPanel
 * props. It used to read `path`, `live` and `host` but silently drop `run` —
 * so a REMOTE transcript opened through this route reached the backend with no
 * run to authorize it against, and a not-yet-mirrored path 400d (spec §3.3:
 * `run` is what proves the path is one that run's own artifacts claim).
 */

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { TranscriptResponse } from '../lib/transcript';
import RunTranscript from './RunTranscript';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const EMPTY: TranscriptResponse = { events: [], summary: null };

function renderAt(search: string) {
  return render(
    <MemoryRouter initialEntries={[`/transcript${search}`]}>
      <RunTranscript />
    </MemoryRouter>,
  );
}

describe('RunTranscript query params', () => {
  it('forwards host and run to the transcript fetch', async () => {
    const spy = vi.spyOn(api, 'getTranscript').mockResolvedValue(EMPTY);
    renderAt('?path=%2Fremote%2Ft%2Frun-1.jsonl&host=h&run=r');

    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy).toHaveBeenCalledWith('/remote/t/run-1.jsonl', { host: 'h', run: 'r' });
  });

  it('leaves both undefined for a plain local transcript', async () => {
    const spy = vi.spyOn(api, 'getTranscript').mockResolvedValue(EMPTY);
    renderAt('?path=%2Ft%2Frun-1.jsonl');

    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy).toHaveBeenCalledWith('/t/run-1.jsonl', { host: undefined, run: undefined });
  });
});

describe('RunTranscript children', () => {
  it('lists a standalone run\'s children under a collapsible section', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(EMPTY);
    const children = vi.spyOn(api, 'getRunChildren').mockResolvedValue([
      {
        handle: 'ch_1', kind: 'subagent', name: 'helper', run_id: 'sub_1', parent_run_id: 'run_01ABC',
        depth: 1, codename: null, address: null, status: 'done', success: true, error: null,
        tokens: null, duration_ms: null, started_at: null, finished_at: null,
        transcript_path: '/t/sub_1.jsonl', legacy: false,
      },
    ]);
    const { findByRole, getByTestId } = renderAt('?path=%2Ft%2Frun_01ABC.jsonl');

    const toggle = await findByRole('button', { name: /Children \(1\)/ });
    expect(children).toHaveBeenCalledWith('run_01ABC', expect.any(AbortSignal));
    expect(getByTestId('children-panel')).toHaveTextContent('helper');
    toggle.click();
    await waitFor(() => expect(toggle).toHaveAttribute('aria-expanded', 'false'));
  });

  it('asks for no children for a remote run or a transcript not named after a run', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(EMPTY);
    const children = vi.spyOn(api, 'getRunChildren').mockResolvedValue([]);
    renderAt('?path=%2Ft%2Frun_01ABC.jsonl&host=h&run=run_01ABC');
    cleanup();
    renderAt('?path=%2Ft%2Fsession-turn.jsonl');
    await waitFor(() => expect(api.getTranscript).toHaveBeenCalledTimes(2));
    expect(children).not.toHaveBeenCalled();
  });
});
