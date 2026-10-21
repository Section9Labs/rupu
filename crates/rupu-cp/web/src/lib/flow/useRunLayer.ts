// The workflow run page's message and board layer, as the page holds it:
// the run's board (`GET /api/runs/:id/board`, through the host transports),
// the layer built from the run's graph, events and message space
// (`runLayer`'s parts, each memoized on what it reads), the shared timeline
// (Replay, scrubber, live) and what it has revealed, and the selected
// message. `null` while the run has no
// message space: the page then shows today's graph.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, boardUnavailable, type RunEvent, type SpaceParticipant } from '../api';
import type { RunGraphModel } from '../runGraphModel';
import type { MessageSpace } from '../useMessageSpace';
import { countAtOrBefore, timeBounds } from '../timeline/timeline';
import { useTimeline, type TimelineState } from '../timeline/useTimeline';
import type { BoardSource, FlowMessage } from './flowModel';
import {
  composeRunLayer,
  LAYER_EVENT_TYPES,
  runBoard,
  runMessages,
  runResolver,
  RUN_SPACE,
  stepTimeline,
  type LayerStep,
  type RunLayer,
  type RunRecordInput,
} from './runLayer';
import { useStableList } from './useStableList';
import { newestAt } from './camera';

/** What a replay follows on the run graph: the newest step. */
const FOLLOW_KINDS: ReadonlySet<string> = new Set(['step']);

const sameStep = (a: LayerStep, b: LayerStep) => a.id === b.id && a.state === b.state;
const sameParticipant = (a: SpaceParticipant, b: SpaceParticipant) =>
  a.address === b.address && a.run_id === b.run_id && a.step_id === b.step_id && a.codename === b.codename;

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
  /** While a replay (or a scrub) shows a time: the newest step revealed,
   *  which the graph's camera follows. */
  follow: string | null;
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
  record: RunRecordInput | null;
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
          setBoard((cur) => cur ?? boardUnavailable(e));
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
  // Each part is keyed on what it reads, so a live event that changes none of
  // it (a tool call, a warning, a status tick) rebuilds nothing here.
  const { log } = space;
  const participants = useStableList(space.participants, sameParticipant);
  const layerEvents = useStableList(useMemo(() => events.filter((e) => LAYER_EVENT_TYPES.has(e.type)), [events]));
  const steps = useStableList(
    useMemo<LayerStep[]>(() => (model ? model.nodes.map((n) => ({ id: n.id, state: n.state })) : []), [model]),
    sameStep,
  );
  const stepIds = useMemo(() => new Set(steps.map((s) => s.id)), [steps]);
  const rec = record ?? undefined;
  const resolve = useMemo(() => runResolver(stepIds, participants, layerEvents, rec), [stepIds, participants, layerEvents, rec]);
  const stepTimes = useMemo(() => stepTimeline(steps, layerEvents, rec), [steps, layerEvents, rec]);
  const messages = useMemo(() => runMessages(log.messages, resolve), [log, resolve]);
  const boardSpace = useMemo(() => runBoard(board, messages, resolve), [board, messages, resolve]);
  const layer = useMemo(
    () => (on ? composeRunLayer(stepTimes, messages, boardSpace) : null),
    [on, stepTimes, messages, boardSpace],
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
  const follow = timeline.t === null ? null : (newestAt(sorted, timeline.t, FOLLOW_KINDS)?.key ?? null);
  const shown = useMemo(
    () => (layer ? (revealed ? layer.messages.filter((m) => revealed.has(m.id)) : layer.messages) : []),
    [layer, revealed],
  );

  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => setSelected(null), [id]);
  const select = useCallback((m: string | null) => setSelected(m), []);
  const spaces = useMemo(() => new Map([[RUN_SPACE, 'run']]), []);

  if (!layer) return null;
  return { layer, timeline, bounds, revealed, shown, selected, select, spaces, follow };
}
