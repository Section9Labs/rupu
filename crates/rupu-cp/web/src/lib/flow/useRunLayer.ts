// The workflow run page's message and board layer, as the page holds it:
// the run's board (`GET /api/runs/:id/board`, through the host transports),
// the layer built from the run's graph, events and message space
// (`buildRunLayer`), the shared timeline (Replay, scrubber, live) and what
// it has revealed, and the selected message. `null` while the run has no
// message space: the page then shows today's graph.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, apiErrorMessage, type RunEvent, type RunGraphResponse } from '../api';
import type { RunGraphModel } from '../runGraphModel';
import type { MessageSpace } from '../useMessageSpace';
import { countAtOrBefore, timeBounds } from '../timeline/timeline';
import { useTimeline, type TimelineState } from '../timeline/useTimeline';
import type { BoardSource, FlowMessage } from './flowModel';
import { buildRunLayer, RUN_SPACE, type RunLayer } from './runLayer';

/** How often a local running run's board is re-read. */
export const BOARD_POLL_MS = 10_000;

export interface RunLayerView {
  layer: RunLayer;
  timeline: TimelineState;
  bounds: { start: number; end: number };
  /** `null` = the whole run (Replay idle). */
  revealed: Set<string> | null;
  /** The messages revealed now. */
  shown: FlowMessage[];
  selected: string | null;
  select(id: string | null): void;
  /** The space's labels for the Messages list. */
  spaces: Map<string, string>;
}

export function useRunLayer({
  id,
  host,
  model,
  events,
  record,
  space,
  running,
}: {
  id: string;
  host?: string;
  model: RunGraphModel | null;
  events: RunEvent[];
  record: Pick<RunGraphResponse, 'run' | 'step_results' | 'units'> | null;
  space: MessageSpace;
  running: boolean;
}): RunLayerView | null {
  const on = space.space === 'present' && !!model;
  const remote = host && host !== 'local' ? host : undefined;

  // --- the board -----------------------------------------------------------
  const [board, setBoard] = useState<BoardSource | null>(null);
  useEffect(() => setBoard(null), [id, remote]);
  useEffect(() => {
    if (!on) return;
    const ctl = new AbortController();
    const read = () =>
      api.getRunBoard(id, { host: remote, signal: ctl.signal }).then(
        (r) => {
          if (!ctl.signal.aborted) setBoard(r.kind === 'ok' ? r.board : r);
        },
        (e: unknown) => {
          if (ctl.signal.aborted) return;
          // A host that can't serve it (an older CP's 404 included) says why
          // in place; a failed re-read keeps what was read.
          setBoard((cur) => cur ?? { kind: 'unavailable', reason: apiErrorMessage(e) });
        },
      );
    void read();
    // A remote host is never polled.
    const timer = running && !remote ? setInterval(() => void read(), BOARD_POLL_MS) : null;
    return () => {
      ctl.abort();
      if (timer) clearInterval(timer);
    };
  }, [on, id, remote, running]);

  // --- the layer -----------------------------------------------------------
  const { participants, log } = space;
  const layer = useMemo(
    () =>
      on && model
        ? buildRunLayer(model, events, { participants, messages: log.messages }, board, { record: record ?? undefined })
        : null,
    [on, model, events, participants, log, board, record],
  );

  // --- the timeline --------------------------------------------------------
  const items = useMemo(() => layer?.timeline.items ?? [], [layer]);
  const bands = useMemo(() => layer?.timeline.bands ?? [], [layer]);
  const nowMs = useMemo(() => (running ? Date.now() : null), [running, events, log]);
  const timeline = useTimeline({ items, bands, nowMs });
  const bounds = useMemo(() => timeBounds(items, bands, nowMs), [items, bands, nowMs]);
  // Keyed on how many items are at or before `t`, so a replay frame that
  // reveals nothing new changes nothing below it.
  const sorted = useMemo(() => [...items].sort((a, b) => a.at - b.at), [items]);
  const count = timeline.t === null ? null : countAtOrBefore(sorted, timeline.t);
  const untimed = layer?.untimed;
  const revealed = useMemo(
    () => (count === null ? null : new Set([...sorted.slice(0, count).map((i) => i.key), ...(untimed ?? [])])),
    [sorted, count, untimed],
  );
  const shown = useMemo(
    () => (layer ? (revealed ? layer.messages.filter((m) => revealed.has(m.id)) : layer.messages) : []),
    [layer, revealed],
  );

  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => setSelected(null), [id]);
  const select = useCallback((m: string | null) => setSelected(m), []);
  const spaces = useMemo(() => new Map([[RUN_SPACE, 'run']]), []);

  if (!layer) return null;
  return { layer, timeline, bounds, revealed, shown, selected, select, spaces };
}
