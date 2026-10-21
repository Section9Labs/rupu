// What the agentiflow map is drawn from (spec `2026-10-21-rupu-agentiflow-flow-
// design.md`, "Live load"): the flow's children, re-read on each detail poll;
// the flow's board, re-read with it while the flow runs; the flow's messages
// from the page's `useMessageSpace`; and, for each workflow child whose frame
// asked (`loadFrame`), its run graph, board and messages, fetched once and
// then followed only while that child runs. A frame's messages come through
// `setFrameSpace` from a `useMessageSpace` the map holds per loaded frame
// (hooks can't be called per list item here).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  apiErrorMessage,
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
  /** Why a frame's graph could not be read, by its run id. */
  frameErrors: ReadonlyMap<string, string>;
}

const boardSource = (r: BoardResult): BoardSource => (r.kind === 'ok' ? r.board : r);
const isLive = (c: RunChild | undefined) => c?.status === 'running' || c?.status === 'pending';

export function useFlowData(
  detail: AgentiflowDetail,
  opts: { running: boolean; flowSpace?: MessageSpace | null },
): FlowData {
  const { running, flowSpace } = opts;
  const id = detail.record.id;

  const [children, setChildren] = useState<RunChild[]>([]);
  const [childrenError, setChildrenError] = useState<string | null>(null);
  const [childrenLoaded, setChildrenLoaded] = useState(false);
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
    setFlowBoard(null);
    setLoadedFrames([]);
    setGraphs(new Map());
    setFrameBoards(new Map());
    setFrameSpaces(new Map());
    setFrameErrors(new Map());
  }, [id]);

  // Children: re-read on each detail (each poll while running).
  useEffect(() => {
    const ctl = new AbortController();
    api
      .getAgentiflowChildren(id, ctl.signal)
      .then((c) => {
        if (ctl.signal.aborted) return;
        setChildren(c);
        setChildrenError(null);
        setChildrenLoaded(true);
      })
      .catch((e: unknown) => {
        if (ctl.signal.aborted) return;
        setChildrenError(apiErrorMessage(e));
        setChildrenLoaded(true);
      });
    return () => ctl.abort();
  }, [id, detail]);

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
        if (!ctl.signal.aborted) setFlowBoard((b) => b ?? { kind: 'unavailable', reason: apiErrorMessage(e) });
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
    const events: RunEvent[] = [];
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
        setFrameBoards((m) => (m.has(f) ? m : new Map(m).set(f, { kind: 'unavailable', reason: apiErrorMessage(e) })));
      },
    );
    const unsubscribe = live
      ? api.subscribeRunLog(f, (ev) => {
          events.push(ev);
          if (events.length > FRAME_MAX_EVENTS) events.splice(0, events.length - FRAME_MAX_EVENTS);
          if (!timer) timer = setTimeout(rebuild, FRAME_FLUSH_MS);
        })
      : null;
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
    frameErrors,
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
