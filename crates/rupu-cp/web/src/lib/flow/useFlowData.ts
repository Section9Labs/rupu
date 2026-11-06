// What the agentiflow map is drawn from (spec `2026-10-21-rupu-agentiflow-flow-
// design.md`, "Live load"): the flow's children, re-read on each detail poll;
// the flow's board, re-read with it while the flow runs; the flow's messages
// from the page's `useMessageSpace`; and, for each workflow child whose frame
// asked (`loadFrame`), its run graph, board and messages, fetched once and
// then followed only while that child runs. A frame's messages come through
// `setFrameSpace` from a `useMessageSpace` the map holds per loaded frame
// (hooks can't be called per list item here).
//
// A frame holds no connection of its own (the browser allows about six per
// host, and the page already holds the shared firehose and the flow's
// message stream): a running child's events so far are read once from its
// log (`readRunLog`, closed once replayed), then followed through the shared
// firehose; its board and messages are re-read on the detail tick.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  apiErrorMessage,
  boardUnavailable,
  type AgentiflowDetail,
  type BoardResult,
  type RunChild,
  type RunEvent,
  type RunGraphResponse,
} from '../api';
import { buildRunGraphModel, type RunGraphModel } from '../runGraphModel';
import type { MessageSpace } from '../useMessageSpace';
import { FLOW_SPACE, type BoardSource, type FlowInput, type FlowSpace } from './flowModel';

/** How long a followed frame batches its run-log events before a rebuild:
 *  each rebuild that changes a state re-lays the whole map, so a busy child
 *  is redrawn at most this often. */
const FRAME_FLUSH_MS = 1000;
/** A followed frame keeps at most this many events (as RunDetail does). */
const FRAME_MAX_EVENTS = 5000;
/** While the flow runs, its children (a walk of every child's ledger) are
 *  re-read at most this often when the detail shows no change of its own
 *  (a unit's sub-agents appear without one). */
export const CHILDREN_LIVE_MS = 15_000;

/** When the children are worth re-reading: the flow's status, its units'
 *  statuses and its rounds changed, or (while it runs) CHILDREN_LIVE_MS
 *  passed. A poll that changed none of it re-walks nothing. */
export function childrenKey(detail: AgentiflowDetail, running: boolean, now: number): string {
  const units = detail.units.map((u) => `${u.unit_id}:${u.status}`).join(',');
  const rounds = detail.events.filter((e) => e.kind === 'round').length;
  return `${detail.record.status}|${rounds}|${units}|${running ? Math.floor(now / CHILDREN_LIVE_MS) : ''}`;
}

export interface FlowData extends FlowInput {
  /** Asks for a workflow child's graph, board and messages (once). */
  loadFrame(childRunId: string): void;
  /** The workflow children whose frame asked to load, in the order asked. */
  loadedFrames: string[];
  /** A loaded frame's message space (`null` while it has none). */
  setFrameSpace(childRunId: string, space: FlowSpace | null): void;
  /** The children have been read (or failed to be) at least once. */
  childrenLoaded: boolean;
  /** Why the children could not be read; `null` when they were. */
  childrenError: string | null;
  /** The server's walk stopped at its cap: sub-agents past it are missing. */
  childrenTruncated: boolean;
  /** Why a frame's graph could not be read, by its run id. */
  frameErrors: ReadonlyMap<string, string>;
}

const boardSource = (r: BoardResult): BoardSource => (r.kind === 'ok' ? r.board : r);

/** `next`, each child that reads the same as before kept as the object it
 *  was (and the whole list when every one is): a poll that changed one child
 *  re-renders only that child's card. */
export function reuseChildren(prev: RunChild[], next: RunChild[]): RunChild[] {
  const before = new Map(prev.map((c) => [c.run_id, c]));
  let same = prev.length === next.length;
  const out = next.map((c, i) => {
    const p = before.get(c.run_id);
    const keep = p && JSON.stringify(p) === JSON.stringify(c) ? p : c;
    if (keep !== prev[i]) same = false;
    return keep;
  });
  return same ? prev : out;
}
const isLive = (c: RunChild | undefined) => c?.status === 'running' || c?.status === 'pending';

export function useFlowData(
  detail: AgentiflowDetail,
  opts: {
    running: boolean;
    flowSpace?: MessageSpace | null;
    /** The clock `childrenKey` reads (tests). */
    now?: () => number;
  },
): FlowData {
  const { running, flowSpace } = opts;
  const now = opts.now ?? Date.now;
  const id = detail.record.id;

  const [children, setChildren] = useState<RunChild[]>([]);
  const [childrenError, setChildrenError] = useState<string | null>(null);
  const [childrenLoaded, setChildrenLoaded] = useState(false);
  const [childrenTruncated, setChildrenTruncated] = useState(false);
  const [flowBoard, setFlowBoard] = useState<BoardSource | null>(null);
  const [loadedFrames, setLoadedFrames] = useState<string[]>([]);
  const [graphs, setGraphs] = useState<Map<string, RunGraphModel>>(new Map());
  const [frameBoards, setFrameBoards] = useState<ReadonlyMap<string, BoardSource>>(new Map());
  const [frameSpaces, setFrameSpaces] = useState<ReadonlyMap<string, FlowSpace>>(new Map());
  const [frameErrors, setFrameErrors] = useState<ReadonlyMap<string, string>>(new Map());

  // A different flow starts over.
  useEffect(() => {
    setChildren([]);
    setChildrenError(null);
    setChildrenLoaded(false);
    setChildrenTruncated(false);
    setFlowBoard(null);
    setLoadedFrames([]);
    setGraphs(new Map());
    setFrameBoards(new Map());
    setFrameSpaces(new Map());
    setFrameErrors(new Map());
  }, [id]);

  // Children: re-read when the detail says something changed (see
  // `childrenKey`), not on every poll.
  const childKey = childrenKey(detail, running, now());
  useEffect(() => {
    const ctl = new AbortController();
    api
      .getAgentiflowChildTree(id, ctl.signal)
      .then(({ children: c, truncated }) => {
        if (ctl.signal.aborted) return;
        setChildren((prev) => reuseChildren(prev, c));
        setChildrenTruncated(truncated);
        setChildrenError(null);
        setChildrenLoaded(true);
      })
      .catch((e: unknown) => {
        if (ctl.signal.aborted) return;
        setChildrenError(apiErrorMessage(e));
        setChildrenLoaded(true);
      });
    return () => ctl.abort();
  }, [id, childKey]);

  // The flow's board: once, then with each detail while the flow runs.
  const boardTick = running ? detail : null;
  useEffect(() => {
    const ctl = new AbortController();
    api
      .getAgentiflowBoard(id, { signal: ctl.signal })
      .then((r) => {
        if (!ctl.signal.aborted) setFlowBoard(boardSource(r));
      })
      .catch((e: unknown) => {
        // A failed read keeps the last board; with none yet, the rail says
        // why it has none.
        if (!ctl.signal.aborted) setFlowBoard((b) => b ?? boardUnavailable(e));
      });
    return () => ctl.abort();
  }, [id, boardTick]);

  const loadFrame = useCallback((childRunId: string) => {
    setLoadedFrames((l) => (l.includes(childRunId) ? l : [...l, childRunId]));
  }, []);

  const setFrameSpace = useCallback((childRunId: string, space: FlowSpace | null) => {
    setFrameSpaces((m) => {
      if (space === null) {
        if (!m.has(childRunId)) return m;
        const next = new Map(m);
        next.delete(childRunId);
        return next;
      }
      return new Map(m).set(childRunId, space);
    });
  }, []);

  // Each loaded frame's graph: fetched once, followed while its child runs.
  // When the child ends the graph is read once more (its step results then
  // carry every state) and the follow stops.
  const childById = useMemo(() => new Map(children.map((c) => [c.run_id, c])), [children]);
  const follows = useRef(new Map<string, { live: boolean; stop: () => void }>());
  const followKey = loadedFrames.map((f) => `${f}:${isLive(childById.get(f)) ? 1 : 0}`).join(',');
  useEffect(() => {
    const want = new Map(loadedFrames.map((f) => [f, isLive(childById.get(f))]));
    for (const [f, cur] of follows.current) {
      if (want.get(f) !== cur.live) {
        cur.stop();
        follows.current.delete(f);
      }
    }
    for (const [f, live] of want) {
      if (follows.current.has(f)) continue;
      follows.current.set(f, { live, stop: followFrame(f, live) });
    }
    // `childById` is read through `followKey`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followKey]);
  useEffect(
    () => () => {
      for (const cur of follows.current.values()) cur.stop();
      follows.current.clear();
    },
    [],
  );

  function followFrame(f: string, live: boolean): () => void {
    const ctl = new AbortController();
    let graph: RunGraphResponse | null = null;
    const log = eventWindow(FRAME_MAX_EVENTS);
    const events = log.events;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let signature = '';
    const rebuild = () => {
      timer = null;
      if (!graph || ctl.signal.aborted) return;
      const model = buildRunGraphModel(graph, events);
      const sig = graphSignature(model);
      // Most events change no state the map shows: keep the model the map
      // already has, so it isn't rebuilt.
      if (sig === signature) return;
      signature = sig;
      setGraphs((m) => new Map(m).set(f, model));
    };
    api.getRunGraph(f).then(
      (g) => {
        if (ctl.signal.aborted) return;
        graph = g;
        setFrameErrors((m) => {
          if (!m.has(f)) return m;
          const next = new Map(m);
          next.delete(f);
          return next;
        });
        rebuild();
      },
      (e: unknown) => {
        if (!ctl.signal.aborted) setFrameErrors((m) => new Map(m).set(f, apiErrorMessage(e)));
      },
    );
    api.getRunBoard(f, { signal: ctl.signal }).then(
      (r) => {
        if (!ctl.signal.aborted) setFrameBoards((m) => new Map(m).set(f, boardSource(r)));
      },
      (e: unknown) => {
        if (ctl.signal.aborted) return;
        setFrameBoards((m) => (m.has(f) ? m : new Map(m).set(f, boardUnavailable(e))));
      },
    );
    const push = (ev: RunEvent) => {
      if (log.add(ev) && !timer) timer = setTimeout(rebuild, FRAME_FLUSH_MS);
    };
    // Live: the firehose's lines for this run, held until the log's replay
    // is in, so the replay's order comes first.
    let held: RunEvent[] | null = live ? [] : null;
    const unsubscribe = live
      ? api.subscribeEvents((ev) => {
          if (ev.run_id !== f) return;
          if (held) held.push(ev);
          else push(ev);
        })
      : null;
    if (live) {
      api.readRunLog(f, { signal: ctl.signal }).then(
        (history) => {
          if (ctl.signal.aborted) return;
          const late = held ?? [];
          held = null;
          for (const e of [...history, ...late]) push(e);
        },
        () => {
          // No history: follow what the firehose brings from now on.
          if (ctl.signal.aborted) return;
          const late = held ?? [];
          held = null;
          for (const e of late) push(e);
        },
      );
    }
    return () => {
      ctl.abort();
      if (timer) clearTimeout(timer);
      unsubscribe?.();
    };
  }

  // A running frame's board is re-read with each detail.
  const liveFrames = loadedFrames.filter((f) => isLive(childById.get(f))).join(',');
  const firstPoll = useRef(true);
  useEffect(() => {
    // The follow already read it once; this is for the polls after.
    if (firstPoll.current) {
      firstPoll.current = false;
      return;
    }
    if (!liveFrames) return;
    const ctl = new AbortController();
    for (const f of liveFrames.split(',')) {
      api.getRunBoard(f, { signal: ctl.signal }).then(
        (r) => {
          if (!ctl.signal.aborted) setFrameBoards((m) => new Map(m).set(f, boardSource(r)));
        },
        // A failed re-read keeps the board the frame already shows (it was
        // read once when the frame loaded); the next poll tries again.
        () => {},
      );
    }
    return () => ctl.abort();
    // Re-read on each poll, not when the set of running frames changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail]);

  const flowMessages = flowSpace?.log.messages;
  const flowParticipants = flowSpace?.participants;
  const spaces = useMemo<FlowSpace[]>(
    () => [
      { id: FLOW_SPACE, participants: flowParticipants ?? [], messages: flowMessages ?? [] },
      ...loadedFrames.flatMap((f) => frameSpaces.get(f) ?? []),
    ],
    [flowMessages, flowParticipants, loadedFrames, frameSpaces],
  );
  const boards = useMemo(() => {
    const m = new Map<string, BoardSource>();
    if (flowBoard) m.set(FLOW_SPACE, flowBoard);
    for (const [f, b] of frameBoards) m.set(f, b);
    return m;
  }, [flowBoard, frameBoards]);

  return {
    detail,
    children,
    spaces,
    boards,
    childGraphs: graphs,
    loadFrame,
    loadedFrames,
    setFrameSpace,
    childrenLoaded,
    childrenError,
    childrenTruncated,
    frameErrors,
  };
}

/** A followed frame's events: the latest `max`, each line kept once for the
 *  frame's whole life. The firehose replays a run's log from its start when
 *  it first attaches it and on every reconnect, so lines the log's replay
 *  already brought come again; a line's identity is its serialized form
 *  (firehose frames carry no id; both streams frame a line identically).
 *  `add` says whether the line was new. */
export function eventWindow(max: number): { events: RunEvent[]; add(ev: RunEvent): boolean } {
  const events: RunEvent[] = [];
  const seen = new Set<string>();
  return {
    events,
    add(ev) {
      const key = JSON.stringify(ev);
      if (seen.has(key)) return false;
      seen.add(key);
      events.push(ev);
      if (events.length > max) {
        events.splice(0, events.length - max);
        // Keep the identity set near the window.
        if (seen.size > 2 * max) {
          seen.clear();
          for (const e of events) seen.add(JSON.stringify(e));
        }
      }
      return true;
    },
  };
}

/** What the map draws of a run graph: each step's state, identity, loop
 *  round and warnings, its units' states, identities (codename, agent,
 *  provider/model) and warnings, its members' states and identities. Two
 *  models with the same signature draw the same. */
export function graphSignature(m: RunGraphModel): string {
  return m.nodes
    .map((n) => {
      const units =
        n.fanout?.units
          .map((u) => `${u.state},${u.codename ?? ''},${u.agent ?? ''},${u.provider ?? ''},${u.model ?? ''},${u.warnings?.length ?? 0}`)
          .join(';') ?? '';
      const subs =
        n.parallel?.map((p) => `${p.state},${p.codename ?? ''},${p.agent ?? ''},${p.provider ?? ''},${p.model ?? ''}`).join(';') ?? '';
      const who = `${n.codename ?? ''},${n.agent ?? ''},${n.provider ?? ''},${n.model ?? ''}`;
      return `${n.id}:${n.state}:${n.round?.current ?? ''}:${who}:${n.warnings?.length ?? 0}:${units}:${subs}`;
    })
    .join('|');
}
