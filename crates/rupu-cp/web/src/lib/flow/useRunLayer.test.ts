// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { api, type RunEvent } from '../api';
import { EMPTY_LOG, withMessages } from '../messages';
import type { MessageSpace } from '../useMessageSpace';
import { useRunLayer } from './useRunLayer';
import { RUN_BOARD, RUN_EVENTS, RUN_ID, RUN_MESSAGES, RUN_MODEL, RUN_PARTICIPANTS, RUN_RECORD } from './runFixtures';

afterEach(() => vi.restoreAllMocks());

const space = {
  space: 'present',
  status: 'ready',
  participants: RUN_PARTICIPANTS,
  log: withMessages(EMPTY_LOG, RUN_MESSAGES),
  caughtUp: true,
} as unknown as MessageSpace;

describe('useRunLayer', () => {
  it('an_unrelated_live_event_rebuilds_nothing', async () => {
    vi.spyOn(api, 'getRunBoard').mockResolvedValue({ kind: 'ok', board: RUN_BOARD });
    const { result, rerender } = renderHook(
      ({ events, model }) =>
        useRunLayer({ id: RUN_ID, model, events, record: RUN_RECORD, space, running: false }),
      { initialProps: { events: RUN_EVENTS, model: RUN_MODEL } },
    );
    await waitFor(() => expect(result.current?.layer.board.view).toEqual(RUN_BOARD));
    const before = result.current!.layer;
    // A tool-call ping and a rebuilt model with the same steps and states.
    const ping = { type: 'step_working', run_id: RUN_ID, step_id: 'report', note: 'tool call' } as RunEvent;
    rerender({ events: [...RUN_EVENTS, ping], model: { ...RUN_MODEL, nodes: RUN_MODEL.nodes.map((n) => ({ ...n })) } });
    const after = result.current!.layer;
    expect(after.timeline.bands).toBe(before.timeline.bands);
    expect(after.messages).toBe(before.messages);
    expect(after).toBe(before);
  });
});
