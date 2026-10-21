// The workflow run page's message and board layer (spec
// `2026-10-21-rupu-agentiflow-flow-design.md`, goal 6): the run's messages
// with their endpoints on the run graph's step cards, its board, and the
// timeline the page replays (a band per step, ticks per message). Pure; no
// React. The agentiflow map's message model (`flowMessage`) and arc routing
// (`flowEdges`) are reused as they are; this only says which card an
// address is.
//
// Run events carry no time of their own. A step's start is the last known
// time before its `step_started` in the event log: the run's start, then
// each step's and unit's recorded end (step results, unit checkpoints). That
// is when it was launched (a step starts as the steps before it end), never
// later. A step with no start event appears at its recorded end, with no
// band; one with neither is shown whole (unless pending) rather than placed
// by guess.

import type { RunEvent, RunGraphResponse, SpaceMessage, SpaceParticipant } from '../api';
import { OPERATOR } from '../messages';
import type { RunGraphModel } from '../runGraphModel';
import type { TimelineBand, TimelineItem } from '../timeline/timeline';
import {
  ARC_REST_LIMIT,
  OFFCANVAS,
  borrowedTimes,
  flowMessage,
  type BoardSource,
  type FlowBoardSpace,
  type FlowMessage,
  type Resolve,
} from './flowModel';

/** The run's own space (and board) id. */
export const RUN_SPACE = 'run';

export interface RunLayer {
  messages: FlowMessage[];
  /** The run's board: its view, its posts, each writer's card. */
  board: FlowBoardSpace;
  timeline: { items: TimelineItem[]; bands: TimelineBand[] };
  /** Cards, messages and board items with no time: shown through a replay. */
  untimed: string[];
}

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

/** A participant's step: its own `step_id`, else the step its agent run
 *  started in (`agent_started`); a sub-agent's (`a>b`) is its nearest listed
 *  hop's. An address no step answers to is off canvas. */
function resolver(model: RunGraphModel, participants: SpaceParticipant[], events: RunEvent[]): Resolve {
  const byAddress = new Map(participants.map((p) => [p.address, p]));
  const stepOfRun = new Map<string, string>();
  for (const e of events) {
    if (e.type === 'agent_started' && typeof e.agent_run_id === 'string' && typeof e.step_id === 'string') {
      stepOfRun.set(e.agent_run_id, e.step_id);
    }
  }
  const step = (id: string | undefined) => (id && model.nodeById(id) ? id : null);
  const ofParticipant = (p: SpaceParticipant) => step(p.step_id) ?? step(stepOfRun.get(p.run_id));
  return (address) => {
    if (address === OPERATOR) return OPERATOR;
    const p = byAddress.get(address);
    if (p) return ofParticipant(p) ?? OFFCANVAS;
    for (let a = address; a.includes('>'); ) {
      a = a.slice(0, a.lastIndexOf('>'));
      const q = byAddress.get(a);
      if (q) return ofParticipant(q) ?? OFFCANVAS;
    }
    if (address.startsWith('step:')) return step(address.slice('step:'.length)) ?? OFFCANVAS;
    return OFFCANVAS;
  };
}

function stepTimeline(
  model: RunGraphModel,
  events: RunEvent[],
  record: Pick<RunGraphResponse, 'run' | 'step_results' | 'units'> | undefined,
): { items: TimelineItem[]; bands: TimelineBand[]; untimed: string[] } {
  const stepEnd = new Map<string, number>();
  for (const r of record?.step_results ?? []) {
    const t = time(r.finished_at);
    if (t !== null) stepEnd.set(r.step_id, t);
  }
  const unitEnd = new Map<string, number>();
  for (const u of record?.units ?? []) {
    const t = time(u.finished_at);
    if (t === null) continue;
    unitEnd.set(`${u.step_id}:${u.index}`, t);
    // A fan-out with no step result yet: its last unit's end.
    if (!record?.step_results.some((r) => r.step_id === u.step_id && time(r.finished_at) !== null)) {
      stepEnd.set(u.step_id, Math.max(stepEnd.get(u.step_id) ?? -Infinity, t));
    }
  }

  let known = time(record?.run.started_at);
  const see = (t: number | null) => {
    if (t !== null) known = known === null ? t : Math.max(known, t);
  };
  const stepStart = new Map<string, number>();
  const items: TimelineItem[] = [];
  const unitSeen = new Set<string>();
  for (const e of events) {
    const stepId = typeof e.step_id === 'string' ? e.step_id : null;
    switch (e.type) {
      case 'run_started':
        see(time(e.started_at));
        break;
      case 'step_started':
        if (stepId && !stepStart.has(stepId) && known !== null && model.nodeById(stepId)) stepStart.set(stepId, known);
        break;
      case 'unit_started': {
        const key = `unit:${stepId}:${String(e.index)}`;
        if (stepId && !unitSeen.has(key) && known !== null) {
          unitSeen.add(key);
          items.push({ key, at: known, kind: 'unit' });
        }
        break;
      }
      case 'unit_completed':
        see(unitEnd.get(`${stepId}:${String(e.index)}`) ?? null);
        break;
      case 'step_completed':
      case 'step_failed':
      case 'step_skipped':
        if (stepId) {
          see(stepEnd.get(stepId) ?? null);
        }
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
  for (const n of model.nodes) {
    const from = stepStart.get(n.id);
    if (from !== undefined) {
      stepItems.push({ key: n.id, at: from, kind: 'step' });
      const end = stepEnd.get(n.id);
      bands.push({ key: n.id, label: n.id, from, to: end !== undefined && end >= from ? end : null, row: 'step' });
      continue;
    }
    const end = stepEnd.get(n.id);
    if (end !== undefined) stepItems.push({ key: n.id, at: end, kind: 'step' });
    else if (n.state !== 'pending') untimed.push(n.id);
  }
  bands.sort((a, b) => a.from - b.from);
  return { items: [...stepItems, ...items], bands, untimed };
}

export function buildRunLayer(
  model: RunGraphModel,
  events: RunEvent[],
  space: { participants: SpaceParticipant[]; messages: SpaceMessage[] },
  board: BoardSource | null,
  opts?: { record?: Pick<RunGraphResponse, 'run' | 'step_results' | 'units'> },
): RunLayer {
  const resolve = resolver(model, space.participants, events);
  const times = borrowedTimes(space.messages);
  const messages = space.messages.map((m, i) => flowMessage(m, RUN_SPACE, resolve, times[i]));
  if (messages.length > ARC_REST_LIMIT) for (const m of messages) m.atRest = m.kind === 'operator';
  messages.sort((a, b) => a.at - b.at);

  const writers = new Map<string, string>();
  if (board && !('kind' in board)) {
    for (const c of board.claims) writers.set(c.owner, resolve(c.owner));
    for (const d of board.directives) writers.set(d.author, resolve(d.author));
  }
  const posts = messages.filter((m) => m.kind === 'post' || m.kind === 'mention');

  const steps = stepTimeline(model, events, opts?.record);
  const items = [...steps.items];
  const untimed = [...steps.untimed];
  for (const m of messages) {
    if (m.atKnown) items.push({ key: m.id, at: m.at, kind: 'message', sub: m.kind });
    else untimed.push(m.id);
  }
  if (board && !('kind' in board)) {
    for (const c of board.claims) {
      const key = claimItemKey(c.key, c.acquired_at);
      const at = time(c.acquired_at);
      if (at !== null) items.push({ key, at, kind: 'board' });
      else untimed.push(key);
    }
    for (const d of board.directives) {
      const key = directiveItemKey(d.id);
      const at = time(d.ts);
      if (at !== null) items.push({ key, at, kind: 'board' });
      else untimed.push(key);
    }
  }

  return {
    messages,
    board: { view: board, posts, writers },
    timeline: { items, bands: steps.bands },
    untimed,
  };
}
