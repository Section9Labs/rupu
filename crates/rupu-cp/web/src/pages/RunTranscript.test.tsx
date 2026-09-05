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
