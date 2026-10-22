// The workflow run page's message and board layer (spec
// `2026-10-21-rupu-agentiflow-flow-design.md`, goal 6): the run's messages
// with their endpoints on the run graph's step cards, its board, and the
// timeline the page replays (a band per step, ticks per message). Pure; no
// React. The agentiflow map's message model (`flowMessage`) and arc routing
// (`flowEdges`) are reused as they are; this only says which card an
// address is and when each step ran.
//
// When a step ran. Run events carry no time of their own, but a finished
// step's record has its `finished_at` and, from runners that record it, the
// `started_at` of its (latest) attempt: exact. A record without it (written
// before the field existed) still has its `step_completed` event's
// `duration_ms`, timed from that start, so its start is
// `finished_at − duration_ms`: exact too. A step with neither (running, or
// an older record whose completion event is outside the page's event
// window) gets a band from a lower bound, marked `inferred`: the last
// known time before its `step_started` in the event log (the run's start,
// exact starts, each step's and unit's recorded end). A step with neither a
// start event nor a duration appears at its recorded end, with no band; one
// with no time at all is shown whole (unless pending), never placed by guess.

import type { RunEvent, RunGraphResponse, SpaceMessage, SpaceParticipant } from '../api';
import { OPERATOR } from '../messages';
import type { GraphNode, RunGraphModel } from '../runGraphModel';
import type { TimelineBand, TimelineItem } from '../timeline/timeline';
import {
  OFFCANVAS,
  borrowedTimes,
  flowMessage,
  markAtRest,
  unitEnd,
  unitIndexOf,
  type BoardSource,
  type FlowBoardSpace,
  type FlowMessage,
  type Resolve,
} from './flowModel';

/** The run's own space (and board) id. */
export const RUN_SPACE = 'run';

/** What the layer reads of `GET /api/runs/:id/graph`: the recorded ends and
 *  the identities the server folded from the whole event log. */
export type RunRecordInput = Pick<
  RunGraphResponse,
  'run' | 'step_results' | 'units' | 'step_identities' | 'unit_identities' | 'subrun_identities'
>;

/** A step as the timeline needs it. */
export type LayerStep = Pick<GraphNode, 'id' | 'state'>;

export interface RunLayer {
  messages: FlowMessage[];
  /** The run's board: its view, its posts, each writer's card. */
  board: FlowBoardSpace;
  timeline: { items: TimelineItem[]; bands: TimelineBand[] };
  /** Cards, messages and board items with no time: shown through a replay. */
  untimed: string[];
}

/** The run events the layer reads (times, starts and ends, agent runs). */
export const LAYER_EVENT_TYPES: ReadonlySet<string> = new Set([
  'run_started',
  'run_completed',
  'run_failed',
  'step_started',
  'step_completed',
  'step_failed',
  'step_skipped',
  'unit_started',
  'unit_completed',
  'agent_started',
]);

export const claimItemKey = (key: string, acquiredAt: string) => `claim:${RUN_SPACE}:${key}:${acquiredAt}`;
export const directiveItemKey = (id: string) => `directive:${RUN_SPACE}:${id}`;

/** `view` as of the replay: its claims and directives revealed so far
 *  (`revealed` null = all of it; an untimed item is always in it). */
export function boardAt(view: BoardSource | null, revealed: ReadonlySet<string> | null): BoardSource | null {
  if (!view || 'kind' in view || revealed === null) return view;
  return {
    ...view,
    claims: view.claims.filter((c) => revealed.has(claimItemKey(c.key, c.acquired_at))),
    directives: view.directives.filter((d) => revealed.has(directiveItemKey(d.id))),
  };
}

function time(v: unknown): number | null {
  if (typeof v !== 'string' || !v) return null;
  const t = Date.parse(v);
  return Number.isFinite(t) ? t : null;
}

/** An address's step card. A participant's step is its own `step_id`; else
 *  the step whose recorded identity carries its codename (or its nearest
 *  ancestor hop's: a sub-agent `a>b` lands on `a`'s step), the codename
 *  being the participant's own or, for a dispatched sub-agent, the server's
 *  fold of it (`subrun_identities`); else the step its agent run started in
 *  (`agent_started`, as far as the event window reaches). The server's folds
 *  cover the whole event log, so they come before the window. An address no
 *  step answers to is off canvas. On a for_each step (`fanouts`) it is the
 *  unit (`unitEnd`): the participant's `unit`, else the unit its codename or
 *  agent run was recorded under. */
export function runResolver(
  stepIds: ReadonlySet<string>,
  participants: SpaceParticipant[],
  events: RunEvent[],
  record?: RunRecordInput,
  fanouts?: ReadonlyMap<string, GraphNode>,
): Resolve {
  type At = { step: string; unit?: number };
  const byAddress = new Map(participants.map((p) => [p.address, p]));
  const stepOfCodename = new Map<string, At>();
  const name = (codename: string | undefined, step: string, unit?: number) => {
    if (codename && stepIds.has(step) && !stepOfCodename.has(codename)) stepOfCodename.set(codename, { step, unit });
  };
  for (const [step, id] of Object.entries(record?.step_identities ?? {})) name(id.codename, step);
  for (const [step, units] of Object.entries(record?.unit_identities ?? {})) {
    for (const [index, id] of Object.entries(units)) name(id.codename, step, Number(index));
  }
  for (const r of record?.step_results ?? []) name(r.codename, r.step_id);
  for (const u of record?.units ?? []) name(u.codename, u.step_id, u.index);
  const stepOfRun = new Map<string, At>();
  for (const e of events) {
    if (e.type === 'agent_started' && typeof e.agent_run_id === 'string' && typeof e.step_id === 'string') {
      stepOfRun.set(e.agent_run_id, { step: e.step_id, unit: typeof e.unit_index === 'number' ? e.unit_index : undefined });
    }
  }
  const byCodename = (codename: string | undefined): At | null => {
    for (let a = codename; a; a = a.includes('>') ? a.slice(0, a.lastIndexOf('>')) : undefined) {
      const s = stepOfCodename.get(a);
      if (s) return s;
    }
    return null;
  };
  /** A step's card, or its unit's when the step is a for_each with it. */
  const card = (at: At | null | undefined, unitKey?: string): string | null => {
    if (!at || !stepIds.has(at.step)) return null;
    const node = fanouts?.get(at.step);
    const unit = unitIndexOf(node, unitKey) ?? unitIndexOf(node, at.unit === undefined ? undefined : String(at.unit));
    return unit === null ? at.step : unitEnd(at.step, unit);
  };
  const ofParticipant = (p: SpaceParticipant): string | null => {
    const codename = p.codename ?? record?.subrun_identities?.[p.run_id]?.codename;
    const recorded = byCodename(codename) ?? stepOfRun.get(p.run_id) ?? null;
    if (p.step_id && stepIds.has(p.step_id)) {
      // Its own step; the unit from its `unit`, else from what was recorded
      // of it on that same step.
      return card({ step: p.step_id, unit: recorded?.step === p.step_id ? recorded.unit : undefined }, p.unit);
    }
    return card(byCodename(codename)) ?? card(stepOfRun.get(p.run_id));
  };
  return (address) => {
    if (address === OPERATOR) return OPERATOR;
    const p = byAddress.get(address);
    if (p) return ofParticipant(p) ?? OFFCANVAS;
    for (let a = address; a.includes('>'); ) {
      a = a.slice(0, a.lastIndexOf('>'));
      const q = byAddress.get(a);
      if (q) return ofParticipant(q) ?? OFFCANVAS;
    }
    if (address.startsWith('step:')) {
      const step = address.slice('step:'.length);
      return stepIds.has(step) ? step : OFFCANVAS;
    }
    return OFFCANVAS;
  };
}

/** Each step's band and arrival, each unit's arrival (see the header). */
export function stepTimeline(
  steps: LayerStep[],
  events: RunEvent[],
  record?: RunRecordInput,
): { items: TimelineItem[]; bands: TimelineBand[]; untimed: string[] } {
  const known = new Set(steps.map((s) => s.id));
  const stepEnd = new Map<string, number>();
  // The latest row's own start, paired with that row's end: a later row
  // without one (an older runner's) clears an earlier row's.
  const recordedStart = new Map<string, number | null>();
  for (const r of record?.step_results ?? []) {
    const t = time(r.finished_at);
    if (t !== null) stepEnd.set(r.step_id, t);
    recordedStart.set(r.step_id, time(r.started_at));
  }
  const unitEnd = new Map<string, number>();
  for (const u of record?.units ?? []) {
    const t = time(u.finished_at);
    if (t !== null) unitEnd.set(`${u.step_id}:${u.index}`, t);
  }
  // The latest completion's duration, timed from that attempt's start.
  const duration = new Map<string, number>();
  for (const e of events) {
    if (e.type === 'step_completed' && typeof e.step_id === 'string' && typeof e.duration_ms === 'number') {
      duration.set(e.step_id, e.duration_ms);
    }
  }
  const exactStart = new Map<string, number>();
  for (const [id, d] of duration) {
    const end = stepEnd.get(id);
    if (end !== undefined && known.has(id)) exactStart.set(id, end - d);
  }
  for (const [id, t] of recordedStart) {
    if (t !== null && known.has(id)) exactStart.set(id, t);
  }

  // The lower bound: the last known time before each start event.
  let bound = time(record?.run.started_at);
  const see = (t: number | null | undefined) => {
    if (t != null) bound = bound === null ? t : Math.max(bound, t);
  };
  const lowerStart = new Map<string, number>();
  const items: TimelineItem[] = [];
  const unitSeen = new Set<string>();
  for (const e of events) {
    const stepId = typeof e.step_id === 'string' ? e.step_id : null;
    switch (e.type) {
      case 'run_started':
        see(time(e.started_at));
        break;
      case 'step_started':
        if (!stepId || !known.has(stepId)) break;
        see(exactStart.get(stepId));
        // A resumed step's latest attempt.
        if (bound !== null) lowerStart.set(stepId, bound);
        break;
      case 'unit_started': {
        const key = `unit:${stepId}:${String(e.index)}`;
        if (stepId && !unitSeen.has(key) && bound !== null) {
          unitSeen.add(key);
          items.push({ key, at: bound, kind: 'unit' });
        }
        break;
      }
      case 'unit_completed':
        see(unitEnd.get(`${stepId}:${String(e.index)}`));
        break;
      case 'step_completed':
      case 'step_failed':
      case 'step_skipped':
        if (stepId) see(stepEnd.get(stepId));
        break;
      case 'run_completed':
      case 'run_failed':
        see(time(e.finished_at));
        break;
    }
  }

  const bands: TimelineBand[] = [];
  const untimed: string[] = [];
  const stepItems: TimelineItem[] = [];
  for (const s of steps) {
    const end = stepEnd.get(s.id);
    const exact = exactStart.get(s.id);
    const lower = lowerStart.get(s.id);
    const from = exact ?? lower;
    if (from !== undefined) {
      stepItems.push({ key: s.id, at: from, kind: 'step' });
      const band: TimelineBand = { key: s.id, label: s.id, from, to: end !== undefined && end >= from ? end : null, row: 'step' };
      if (exact === undefined) band.inferred = true;
      bands.push(band);
    } else if (end !== undefined) stepItems.push({ key: s.id, at: end, kind: 'step' });
    else if (s.state !== 'pending') untimed.push(s.id);
  }
  bands.sort((a, b) => a.from - b.from);
  return { items: [...stepItems, ...items], bands, untimed };
}

/** The run's for_each steps by id (whose units an address can land on). */
export function fanoutsOf(model: RunGraphModel): Map<string, GraphNode> {
  return new Map(model.nodes.filter((n) => n.kind === 'for_each').map((n) => [n.id, n]));
}

/** The space's messages, endpoints through `resolve`. */
export function runMessages(log: SpaceMessage[], resolve: Resolve): FlowMessage[] {
  const times = borrowedTimes(log);
  const messages = log.map((m, i) => flowMessage(m, RUN_SPACE, resolve, times[i]));
  markAtRest(messages);
  return messages.sort((a, b) => a.at - b.at);
}

/** The board, its posts, and each claim owner's and directive author's card. */
export function runBoard(view: BoardSource | null, messages: FlowMessage[], resolve: Resolve): FlowBoardSpace {
  const writers = new Map<string, string>();
  if (view && !('kind' in view)) {
    for (const c of view.claims) writers.set(c.owner, resolve(c.owner));
    for (const d of view.directives) writers.set(d.author, resolve(d.author));
  }
  return { view, posts: messages.filter((m) => m.kind === 'post' || m.kind === 'mention'), writers };
}

/** The layer from its parts (the page memoizes each part on its own inputs). */
export function composeRunLayer(
  steps: { items: TimelineItem[]; bands: TimelineBand[]; untimed: string[] },
  messages: FlowMessage[],
  board: FlowBoardSpace,
): RunLayer {
  const items = [...steps.items];
  const untimed = [...steps.untimed];
  for (const m of messages) {
    if (m.atKnown) items.push({ key: m.id, at: m.at, kind: 'message', sub: m.kind });
    else untimed.push(m.id);
  }
  const view = board.view;
  if (view && !('kind' in view)) {
    for (const c of view.claims) {
      const key = claimItemKey(c.key, c.acquired_at);
      const at = time(c.acquired_at);
      if (at !== null) items.push({ key, at, kind: 'board' });
      else untimed.push(key);
    }
    for (const d of view.directives) {
      const key = directiveItemKey(d.id);
      const at = time(d.ts);
      if (at !== null) items.push({ key, at, kind: 'board' });
      else untimed.push(key);
    }
  }
  return { messages, board, timeline: { items, bands: steps.bands }, untimed };
}

export function buildRunLayer(
  model: RunGraphModel,
  events: RunEvent[],
  space: { participants: SpaceParticipant[]; messages: SpaceMessage[] },
  board: BoardSource | null,
  opts?: { record?: RunRecordInput },
): RunLayer {
  const record = opts?.record;
  const resolve = runResolver(new Set(model.nodes.map((n) => n.id)), space.participants, events, record, fanoutsOf(model));
  const messages = runMessages(space.messages, resolve);
  return composeRunLayer(stepTimeline(model.nodes, events, record), messages, runBoard(board, messages, resolve));
}
