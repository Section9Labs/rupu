// The agentiflow flow's model (spec `2026-10-21-rupu-agentiflow-flow-design.md`):
// rounds down the lead's spine, the children each round launched as a tree
// branching right (a workflow child as a frame holding its run graph), every
// message of every loaded space with its endpoints resolved to those nodes,
// the boards, and the timeline the map replays. Pure; no React. The density
// rules (collapsed rounds, "+N more", arcs at rest) are decided here so the
// layout and the canvas only draw what this says.

import {
  roundEvents,
  type AgentiflowDetail,
  type AgentiflowRoundEvent,
  type BoardView,
  type MessagesUnavailable,
  type RunChild,
  type SpaceMessage,
  type SpaceParticipant,
} from '../api';
import { LEAD, OPERATOR, rowKind, type RowKind } from '../messages';
import type { RunGraphModel } from '../runGraphModel';
import type { TimelineBand, TimelineItem } from '../timeline/timeline';

/** The latest finished rounds kept expanded; the running one is too. */
export const KEEP_EXPANDED_ROUNDS = 2;
/** Cards a round shows before the rest fold into "+N more". */
export const MAX_CARDS_PER_ROUND = 8;
/** Above this many messages in one space (the flow's, or a frame's), only
 *  that space's arcs to or from the operator are drawn at rest; the badge
 *  counts, hover and selection carry the rest. */
export const ARC_REST_LIMIT = 12;

/** The flow's own space (and board) id. */
export const FLOW_SPACE = 'flow';
/** A board post's (and directive's) destination: the rail, or a frame's strip. */
export const BOARD = 'board';
/** Where an address no node answers to goes. */
export const OFFCANVAS = 'offcanvas';
/** An undelivered recipient no node answers to: a dead end at its sender. */
export const DEAD_END = 'deadend:';
export const deadEnd = (address: string) => `${DEAD_END}${address}`;
export const isDeadEnd = (endpoint: string) => endpoint.startsWith(DEAD_END);
/** An endpoint drawn as a short entry at the card it reaches, not a node. */
export const isOutside = (endpoint: string) => endpoint === OPERATOR || endpoint === OFFCANVAS;

export type BoardSource = BoardView | MessagesUnavailable;

export interface FlowSpace {
  /** `'flow'`, or the run id of the workflow child the space belongs to. */
  id: string;
  participants: SpaceParticipant[];
  messages: SpaceMessage[];
  /** Everything its log held when opened has been read (`useMessageSpace`'s
   *  `caughtUp`): messages arriving after this are new. */
  caughtUp?: boolean;
}

export interface FlowInput {
  detail: AgentiflowDetail;
  children: RunChild[];
  spaces: FlowSpace[];
  /** By space id, as loaded. */
  boards: Map<string, BoardSource>;
  /** A workflow child's run graph, by its run id, once its frame loaded it. */
  childGraphs: Map<string, RunGraphModel>;
}

export interface GoalAfter {
  id: string;
  met: boolean;
  current: number;
}

export interface FlowRound {
  round: number;
  /** `null` when the run predates `started_at` (and for a round still running). */
  startedAt: number | null;
  /** `null` while the round runs. */
  endedAt: number | null;
  decision: string | null;
  /** The assessment that followed the round: the next round's `goals`, the
   *  record's for the last round of a completed flow (the record holds goals
   *  only once the flow completes); `null` when unknown or running. */
  goalsAfter: GoalAfter[] | null;
  /** The run's cumulative spend as the round ended. */
  spentUsd: number | null;
  /** The lead's call failed this round (its event's `outcome: error`). */
  failed: boolean;
  /** Why, when it failed (the event's `error`). */
  error: string | null;
  /** The round stopped at its turn budget (`outcome: turn_budget_hit`). */
  turnBudgetHit: boolean;
  transcriptPath: string | null;
  collapsed: boolean;
  /** Every child the round launched, any depth, in tree order. */
  childIds: string[];
  /** Its children folded into "+N more" (0 when collapsed or expanded). */
  hidden: number;
}

export type FlowNodeKind = 'lead' | 'round' | 'child' | 'more' | 'frame';

export interface FlowNodeRef {
  /** A child's run id, `round:N`, `more:N` or `lead`. */
  id: string;
  kind: FlowNodeKind;
  child?: RunChild;
  /** A frame's run graph, once loaded. */
  graph?: RunGraphModel;
  /** `''` for the lead. */
  parentId: string;
  /** The column: 0 for the spine and round nodes, a round's children 1, theirs 2… */
  depth: number;
  /** -1 for the lead, which spans every round. */
  round: number;
  /** Placed by start time, or hung off its round for want of its parent. */
  inferred: boolean;
}

export interface FlowUndelivered {
  /** The node the dead end is drawn toward, or `deadend:<address>` (a stub
   *  at the sender) when no node is it. */
  to: string;
  /** The address it was sent to. */
  address: string;
  reason: string;
}

export interface FlowMessage {
  id: string;
  kind: RowKind;
  /** A node id, `'operator'` or `'offcanvas'`. */
  from: string;
  /** Node ids, `'board'`, `'operator'`, `'offcanvas'` for an unknown
   *  address, or `deadend:<address>` for an undelivered one no node is. */
  to: string[];
  undelivered: FlowUndelivered[];
  /** Drawn without hover or selection. */
  atRest: boolean;
  space: string;
  /** Its `ts`; for an unparseable one, the space's latest time before it
   *  (else its first), so it keeps its place in the list. */
  at: number;
  /** False when `at` was borrowed: the message has no timeline item. */
  atKnown: boolean;
  body: string;
  message: SpaceMessage;
}

export interface FlowBoardSpace {
  /** `null` when not loaded. */
  view: BoardSource | null;
  /** The space's board posts (and @mentions). */
  posts: FlowMessage[];
  /** Each claim owner's and directive author's node, by address. */
  writers: Map<string, string>;
}

export interface FlowBoard extends FlowBoardSpace {
  /** Each workflow child's strip, by its run id. */
  frames: Map<string, FlowBoardSpace>;
}

/** What the map lays out: the rounds and the cards, with their timeline
 *  (bands, round and child arrivals). Built from the detail, the children
 *  and the loaded graphs, never the messages, so a message flush (every
 *  60 ms on a live stream) doesn't lay the map out again. */
export interface FlowStructure {
  rounds: FlowRound[];
  nodes: FlowNodeRef[];
  timeline: { items: TimelineItem[]; bands: TimelineBand[] };
}

export interface FlowModel extends FlowStructure {
  messages: FlowMessage[];
  board: FlowBoard;
}

const roundId = (n: number) => `round:${n}`;
const moreId = (n: number) => `more:${n}`;

function time(v: string | null | undefined): number | null {
  if (!v) return null;
  const t = Date.parse(v);
  return Number.isFinite(t) ? t : null;
}

const unfinished = (c: RunChild) => c.status === 'running' || c.status === 'pending';
const alwaysShown = (c: RunChild) => c.status === 'running' || c.status === 'failed';

/** By start time, unknown last; ties keep the input order. */
function byStart(a: RunChild, b: RunChild): number {
  return (time(a.started_at) ?? Infinity) - (time(b.started_at) ?? Infinity);
}

interface RoundSpan {
  round: number;
  event: AgentiflowRoundEvent | null;
  startedAt: number | null;
  endedAt: number | null;
  /** Where the round's band starts: its start, else the previous end. */
  from: number | null;
}

function roundSpans(detail: AgentiflowDetail, children: RunChild[]): RoundSpan[] {
  const events = [...roundEvents(detail.events)].sort((a, b) => a.round - b.round);
  const runStart = time(detail.record.started_at);
  const spans: RoundSpan[] = [];
  let prevEnd = runStart;
  for (const event of events) {
    const startedAt = time(event.started_at);
    const endedAt = time(event.ts);
    spans.push({ round: event.round, event, startedAt, endedAt, from: startedAt ?? prevEnd });
    prevEnd = endedAt ?? prevEnd;
  }
  // The round after the last event: the running one, or one that ended
  // without its event (a coordinator that died mid-round) but launched work.
  const last = spans[spans.length - 1];
  const next = last ? last.round + 1 : 0;
  const lastEnd = last?.endedAt ?? -Infinity;
  // (A launched child's own children follow it, wherever they start.)
  const listed = new Set(children.map((c) => c.run_id));
  const beyond = children.some((c) =>
    c.round != null
      ? c.round === next
      : !listed.has(c.parent_run_id) && (time(c.started_at) ?? -Infinity) > lastEnd,
  );
  const running = detail.record.status === 'running';
  if (running || (beyond && spans.length > 0)) {
    const endedAt = running ? null : time(detail.record.ended_at);
    spans.push({ round: next, event: null, startedAt: null, endedAt, from: prevEnd });
  }
  // A stopped flow with no round to hang work on (an unreadable event log):
  // one round 0 spanning its children, everything in it inferred.
  if (spans.length === 0 && children.length > 0) {
    const starts = children.map((c) => time(c.started_at)).filter((t): t is number => t !== null);
    const ends = children.map((c) => time(c.finished_at)).filter((t): t is number => t !== null);
    const startedAt = starts.length ? Math.min(...starts) : null;
    const endedAt = ends.length ? Math.max(...ends) : time(detail.record.ended_at);
    spans.push({ round: 0, event: null, startedAt, endedAt, from: startedAt ?? runStart });
  }
  return spans;
}

interface Placement {
  round: number;
  inferred: boolean;
}

/** The round in progress at `t`: the first round (in order) not over by
 *  then, a round with no end counting as never over; the last round when all
 *  were. The one lookup the model and the map's router share. A prefix
 *  maximum of the ends keeps the search right when an unparseable end sits
 *  between parseable ones. */
export function makeRoundAt(rounds: readonly { round: number; endedAt: number | null }[]): (t: number) => number | undefined {
  const reach: number[] = [];
  let max = -Infinity;
  for (const r of rounds) {
    max = Math.max(max, r.endedAt ?? Infinity);
    reach.push(max);
  }
  return (t) => {
    let lo = 0;
    let hi = reach.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (reach[mid] >= t) hi = mid;
      else lo = mid + 1;
    }
    return rounds[Math.min(lo, rounds.length - 1)]?.round;
  };
}

/** `child.round`; else a launched child's root ancestor's; else the first
 *  round not over when it started (the last round if none), inferred. */
function placer(spans: RoundSpan[], byId: Map<string, RunChild>): (c: RunChild) => Placement {
  const known = new Set(spans.map((s) => s.round));
  const memo = new Map<string, Placement>();
  const roundAt = makeRoundAt(spans);
  const byTime = (c: RunChild): Placement => {
    const t = time(c.started_at);
    return { round: (t === null ? undefined : roundAt(t)) ?? spans[spans.length - 1].round, inferred: true };
  };
  const place = (c: RunChild, seen: Set<string>): Placement => {
    const cached = memo.get(c.run_id);
    if (cached) return cached;
    let p: Placement;
    const parent = byId.get(c.parent_run_id);
    if (c.round != null && known.has(c.round)) p = { round: c.round, inferred: false };
    else if (parent && !seen.has(parent.run_id)) p = place(parent, seen.add(c.run_id));
    else p = byTime(c);
    memo.set(c.run_id, p);
    return p;
  };
  return (c) => place(c, new Set());
}

export type Resolve = (address: string) => string;

/** A space's addresses to node ids: participants to their run (a workflow
 *  child's step agents to `<child>/<step>`), `lead`, `operator`, else
 *  off canvas. */
function resolver(
  space: FlowSpace,
  childIds: Set<string>,
  graph: RunGraphModel | undefined,
  childByAddress: ReadonlyMap<string, string>,
): Resolve {
  const byAddress = new Map(space.participants.map((p) => [p.address, p]));
  const frame = space.id !== FLOW_SPACE;
  // The frame itself until its graph is loaded, or when the graph has no
  // such step.
  const inner = (step: string) => (graph?.nodeById(step) ? `${space.id}/${step}` : space.id);
  // A step agent's sub-agent (`probe#5>wren#1`) through its nearest listed
  // ancestor hop: that hop's step, else the frame.
  const viaHop = (address: string): string | null => {
    for (let a = address; a.includes('>'); ) {
      a = a.slice(0, a.lastIndexOf('>'));
      const q = byAddress.get(a);
      if (q) return q.step_id ? inner(q.step_id) : space.id;
    }
    return null;
  };
  return (address) => {
    if (address === OPERATOR) return OPERATOR;
    if (address === LEAD) return LEAD;
    const p = byAddress.get(address);
    if (frame && p?.step_id) return inner(p.step_id);
    // A run with its own card keeps its arcs.
    if (p && childIds.has(p.run_id)) return p.run_id;
    if (frame) {
      const hop = viaHop(address);
      if (hop) return hop;
    }
    if (frame && address.startsWith('step:')) return inner(address.slice('step:'.length));
    // A space with no participant registry (a pre-F1 flow's board): the
    // child that went by the address.
    if (!frame && !p) {
      const child = childByAddress.get(address);
      if (child) return child;
    }
    return OFFCANVAS;
  };
}

/** Drawn at rest: every message of a quiet space, else only those to or
 *  from the operator. */
export function markAtRest(list: FlowMessage[]): void {
  if (list.length <= ARC_REST_LIMIT) return;
  for (const m of list) m.atRest = m.kind === 'operator' || m.from === OPERATOR || m.to.includes(OPERATOR);
}

const BOARD_KINDS: ReadonlySet<RowKind> = new Set(['post', 'mention', 'directive', 'retract']);

/** `m` as the map draws it: its endpoints through `resolve`, a board write
 *  also landing on the board, every undelivered recipient its own end. */
export function flowMessage(m: SpaceMessage, space: string, resolve: Resolve, at: number | null): FlowMessage {
  const kind = rowKind(m);
  const undelivered = (m.delivery?.undelivered ?? []).map((u) => {
    const node = resolve(u.to);
    // Nobody to point at: the dead end stays at the sender.
    return { to: isOutside(node) ? deadEnd(u.to) : node, address: u.to, reason: u.reason };
  });
  let to = (m.delivery?.resolved ?? []).map(resolve);
  if (BOARD_KINDS.has(kind)) to = [BOARD, ...to];
  to = [...to, ...undelivered.map((u) => u.to)];
  // An older log without delivery still points somewhere.
  if (to.length === 0) to = [m.to ? resolve(m.to) : OFFCANVAS];
  return {
    id: m.id,
    kind,
    from: resolve(m.from),
    to: [...new Set(to)],
    undelivered,
    atRest: true,
    space,
    at: at ?? 0,
    atKnown: time(m.ts) !== null,
    body: m.body,
    message: m,
  };
}

/** Each message's time, in log order; an unparseable `ts` borrows the latest
 *  time before it (else the first after it), `null` when the log has none. */
export function borrowedTimes(messages: SpaceMessage[]): (number | null)[] {
  const out: (number | null)[] = [];
  let latest: number | null = null;
  for (const m of messages) {
    const t = time(m.ts);
    if (t !== null) latest = latest === null ? t : Math.max(latest, t);
    out.push(t ?? latest);
  }
  const first = out.find((t) => t !== null) ?? null;
  return out.map((t) => t ?? first);
}

function boardSpace(view: BoardSource | null, posts: FlowMessage[], resolve: Resolve): FlowBoardSpace {
  const writers = new Map<string, string>();
  if (view && !('kind' in view)) {
    for (const c of view.claims) writers.set(c.owner, resolve(c.owner));
    for (const d of view.directives) writers.set(d.author, resolve(d.author));
  }
  return { view, posts, writers };
}

export function buildFlowModel(input: FlowInput, opts?: { expanded?: ReadonlySet<string> }): FlowModel {
  return withFlowMessages(buildFlowStructure(input, opts), input);
}

/** The rounds, the cards and their timeline (`FlowStructure`). */
export function buildFlowStructure(
  input: Pick<FlowInput, 'detail' | 'children' | 'childGraphs'>,
  opts?: { expanded?: ReadonlySet<string> },
): FlowStructure {
  const expanded = opts?.expanded ?? new Set<string>();
  const { detail, children } = input;
  const byId = new Map(children.map((c) => [c.run_id, c]));
  const spans = roundSpans(detail, children);
  const place = placer(spans, byId);

  // --- the tree --------------------------------------------------------
  const kids = new Map<string, RunChild[]>();
  const tops = new Map<number, RunChild[]>(spans.map((s) => [s.round, []]));
  const orphaned = new Set<string>();
  for (const c of children) {
    const parent = byId.get(c.parent_run_id);
    const round = place(c).round;
    if (parent && parent.run_id !== c.run_id && place(parent).round === round) {
      kids.set(parent.run_id, [...(kids.get(parent.run_id) ?? []), c]);
    } else {
      if (parent || c.depth > 1) orphaned.add(c.run_id);
      tops.get(round)!.push(c);
    }
  }
  for (const list of [...kids.values(), ...tops.values()]) list.sort(byStart);

  const subtree = (c: RunChild, depth: number, parentId: string, out: FlowNodeRef[], seen: Set<string>) => {
    if (seen.has(c.run_id)) return;
    seen.add(c.run_id);
    const p = place(c);
    const node: FlowNodeRef = {
      id: c.run_id,
      kind: c.kind === 'workflow' ? 'frame' : 'child',
      child: c,
      parentId,
      depth,
      round: p.round,
      inferred: p.inferred || orphaned.has(c.run_id),
    };
    const graph = input.childGraphs.get(c.run_id);
    if (node.kind === 'frame' && graph) node.graph = graph;
    out.push(node);
    for (const k of kids.get(c.run_id) ?? []) subtree(k, depth + 1, c.run_id, out, seen);
  };

  const withEvents = spans.filter((s) => s.event);
  const lastEvents = withEvents[withEvents.length - 1];
  const transcripts = new Map(detail.lead_transcripts.map((t) => [t.round, t.path]));
  const running = detail.record.status === 'running';
  const isLive = (s: RoundSpan) => running && s.event === null;
  const latest = new Set(
    spans
      .filter((s) => !isLive(s))
      .slice(-KEEP_EXPANDED_ROUNDS)
      .map((s) => s.round),
  );
  const seen = new Set<string>();

  const rounds: FlowRound[] = [];
  const nodes: FlowNodeRef[] = [{ id: LEAD, kind: 'lead', parentId: '', depth: 0, round: -1, inferred: false }];
  spans.forEach((s, i) => {
    const id = roundId(s.round);
    const top = tops.get(s.round) ?? [];
    const tree: FlowNodeRef[] = [];
    for (const c of top) subtree(c, 1, id, tree, seen);
    const busy = tree.some((n) => n.child && unfinished(n.child));
    const open = expanded.has(id);
    const collapsed = !isLive(s) && !latest.has(s.round) && !open && !busy;

    let hidden = 0;
    nodes.push({ id, kind: 'round', parentId: LEAD, depth: 0, round: s.round, inferred: false });
    if (!collapsed) {
      // A top-level child stays a card when it, or anything below it,
      // failed or is running.
      const keep = new Set<string>();
      let head = '';
      for (const n of tree) {
        if (n.parentId === id) head = n.id;
        if (n.child && alwaysShown(n.child)) keep.add(head);
      }
      const shown = open ? top : top.filter((c, k) => k < MAX_CARDS_PER_ROUND || keep.has(c.run_id));
      const visible = new Set(shown.map((c) => c.run_id));
      hidden = top.length - shown.length;
      // Each shown top-level child brings its whole subtree.
      let take = false;
      for (const n of tree) {
        if (n.parentId === id) take = visible.has(n.id);
        if (take) nodes.push(n);
      }
      if (hidden > 0) {
        nodes.push({ id: moreId(s.round), kind: 'more', parentId: id, depth: 1, round: s.round, inferred: false });
      }
    }

    const next = spans[i + 1]?.event;
    let goalsAfter: GoalAfter[] | null = null;
    if (s.event) {
      if (next) goalsAfter = next.goals ?? null;
      else if (s === lastEvents && detail.record.status === 'completed') {
        goalsAfter = detail.record.goals.map(({ id: g, met, current }) => ({ id: g, met, current }));
      }
    }
    rounds.push({
      round: s.round,
      startedAt: s.startedAt,
      endedAt: s.endedAt,
      decision: s.event?.decision ?? null,
      goalsAfter,
      spentUsd: s.event ? s.event.spent_usd : null,
      failed: s.event?.outcome === 'error',
      error: s.event?.outcome === 'error' ? (s.event.error ?? null) : null,
      turnBudgetHit: s.event?.outcome === 'turn_budget_hit',
      transcriptPath: transcripts.get(s.round) ?? null,
      collapsed,
      childIds: tree.map((n) => n.id),
      hidden,
    });
  });

  // --- timeline: the rounds and the cards ------------------------------
  const items: TimelineItem[] = [];
  const bands: TimelineBand[] = [];
  const startOf = new Map<number, number | null>();
  for (const s of spans) {
    startOf.set(s.round, s.from);
    if (s.from !== null) {
      items.push({ key: roundId(s.round), at: s.from, kind: 'round' });
      bands.push({ key: roundId(s.round), label: `Round ${s.round}`, from: s.from, to: s.endedAt, row: 'round' });
    }
  }
  for (const r of rounds) {
    for (const id of r.childIds) {
      const at = time(byId.get(id)?.started_at) ?? startOf.get(r.round) ?? null;
      if (at !== null) items.push({ key: id, at, kind: 'child' });
    }
  }

  return { rounds, nodes, timeline: { items, bands } };
}

/** `structure` with every loaded space's messages (endpoints on its cards),
 *  the boards, and their timeline items. */
export function withFlowMessages(
  structure: FlowStructure,
  input: Pick<FlowInput, 'children' | 'spaces' | 'boards' | 'childGraphs'>,
): FlowModel {
  const { children } = input;
  // --- messages and boards ---------------------------------------------
  const childIds = new Set(children.map((c) => c.run_id));
  const childByAddress = new Map<string, string>();
  for (const c of children) if (c.address && !childByAddress.has(c.address)) childByAddress.set(c.address, c.run_id);
  const resolvers = new Map<string, Resolve>();
  const resolverOf = (spaceId: string) => {
    let r = resolvers.get(spaceId);
    if (!r) {
      const space = input.spaces.find((s) => s.id === spaceId) ?? { id: spaceId, participants: [], messages: [] };
      r = resolver(space, childIds, input.childGraphs.get(spaceId), childByAddress);
      resolvers.set(spaceId, r);
    }
    return r;
  };
  const messages = input.spaces
    .flatMap((s) => {
      const times = borrowedTimes(s.messages);
      const list = s.messages.map((m, i) => flowMessage(m, s.id, resolverOf(s.id), times[i]));
      markAtRest(list);
      return list;
    })
    .sort((a, b) => a.at - b.at);

  const postsOf = (spaceId: string) =>
    messages.filter((m) => m.space === spaceId && (m.kind === 'post' || m.kind === 'mention'));
  const boardOf = (spaceId: string) =>
    boardSpace(input.boards.get(spaceId) ?? null, postsOf(spaceId), resolverOf(spaceId));
  const frames = new Map<string, FlowBoardSpace>();
  for (const id of [...input.spaces.map((s) => s.id), ...input.boards.keys()]) {
    if (id !== FLOW_SPACE && !frames.has(id)) frames.set(id, boardOf(id));
  }
  const board: FlowBoard = { ...boardOf(FLOW_SPACE), frames };

  // --- timeline: the messages and the board ----------------------------
  const items: TimelineItem[] = [...structure.timeline.items];
  for (const m of messages) if (m.atKnown) items.push({ key: m.id, at: m.at, kind: 'message', sub: m.kind });
  for (const [spaceId, view] of input.boards) {
    if ('kind' in view) continue;
    for (const c of view.claims) {
      const at = time(c.acquired_at);
      if (at !== null) items.push({ key: `claim:${spaceId}:${c.key}:${c.acquired_at}`, at, kind: 'board' });
    }
    for (const d of view.directives) {
      const at = time(d.ts);
      if (at !== null) items.push({ key: `directive:${spaceId}:${d.id}`, at, kind: 'board' });
    }
  }

  return { ...structure, messages, board, timeline: { items, bands: structure.timeline.bands } };
}
