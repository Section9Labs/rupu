// The agentiflow map's arc routing (spec `2026-10-21-rupu-agentiflow-flow-
// design.md`, "Messages" and "Density"). Pure; no React. Given the model and
// which nodes are drawn now, it decides the card each message endpoint lands
// on (a hidden child falls back to its round's summary or "+N more" card,
// the lead to the round it was in, a board write to its item on the rail or
// a frame's strip), which arcs are drawn (at rest, or for the focused /
// hovered card, the selected round or the selected message), the claim
// arcs, and each card's message count. The operator, an address off the map
// and a recipient nobody answers to are no card: their arcs are stubs at the
// one card they touch. A board shows its latest items per section
// (`railWindow`); an arc to a folded item lands on its "N earlier" row.

import type { ClaimView, DirectiveView } from '../api';
import { KIND_FILTERS, LEAD, OPERATOR, type KindFilter } from '../messages';
import {
  BOARD,
  FLOW_SPACE,
  isDeadEnd,
  isOutside,
  makeRoundAt,
  type BoardSource,
  type FlowMessage,
  type FlowStructure,
  type FlowUndelivered,
} from './flowModel';

/** The flow's board on the canvas (the rail node). */
export const RAIL = 'rail';

export interface Anchor {
  /** The drawn node the endpoint lands on. */
  node: string;
  /** A board item's anchor in that node (a claim key or message id). */
  handle?: string;
}

export interface Router {
  /** The card `endpoint` (a model endpoint of `m`) lands on as drawn; `null`
   *  when nothing it could land on is drawn, or when it is no card at all
   *  (the operator, an address off the map, a dead end: drawn as a short
   *  stub at the card on the other end). */
  anchorOf(endpoint: string, m: FlowMessage): Anchor | null;
  /** The drawn cards `m` touches, sender first. */
  endsOf(m: FlowMessage): string[];
  /** The round a drawn node belongs to. */
  roundOfNode(id: string): number | undefined;
}

/** The key a board item's anchor goes by: a claim's, else the message id
 *  (a retraction lands on the directive it retracts). */
export const claimKey = (c: Pick<ClaimView, 'key' | 'acquired_at'>) => `claim:${c.key}:${c.acquired_at}`;

export const boardKey = (m: FlowMessage) => (m.kind === 'retract' ? (m.message.refs?.retracts ?? m.id) : m.id);

export function makeRouter(model: Pick<FlowStructure, 'nodes' | 'rounds'>, visible: ReadonlySet<string>): Router {
  const roundOf = new Map<string, number>();
  for (const n of model.nodes) if (n.kind !== 'lead') roundOf.set(n.id, n.round);
  for (const r of model.rounds) for (const id of r.childIds) roundOf.set(id, r.round);
  const byRound = new Map(model.rounds.map((r) => [r.round, r]));
  // The lead's round at a time: the same lookup the model places children by.
  const leadRound = makeRoundAt(model.rounds);
  const drawn = (node: string): Anchor | null => (visible.has(node) ? { node } : null);

  const anchorOf = (endpoint: string, m: FlowMessage): Anchor | null => {
    if (endpoint === LEAD) {
      const r = leadRound(m.at);
      return r === undefined ? null : drawn(`round:${r}`);
    }
    if (isOutside(endpoint) || isDeadEnd(endpoint)) return null;
    if (endpoint === BOARD) {
      const node = m.space === FLOW_SPACE ? RAIL : m.space;
      return visible.has(node) || node === RAIL ? { node, handle: boardKey(m) } : null;
    }
    if (visible.has(endpoint)) return { node: endpoint };
    // An inner step of a frame not drawn: the frame.
    const slash = endpoint.indexOf('/');
    if (slash > 0) return anchorOf(endpoint.slice(0, slash), m);
    // A child not drawn as a card: its round's summary, its "+N more", its round.
    const round = roundOf.get(endpoint);
    if (round === undefined) return null;
    const r = byRound.get(round);
    const id = r?.collapsed ? `summary:${round}` : r && r.hidden > 0 ? `more:${round}` : `round:${round}`;
    return drawn(id);
  };

  const endsOf = (m: FlowMessage) =>
    [m.from, ...m.to].map((e) => anchorOf(e, m)?.node).filter((x): x is string => !!x);

  const roundOfNode = (id: string): number | undefined => {
    const colon = id.indexOf(':');
    if (colon > 0 && ['round', 'more', 'summary'].includes(id.slice(0, colon))) return Number(id.slice(colon + 1));
    const base = id.includes('/') ? id.slice(0, id.indexOf('/')) : id;
    return roundOf.get(base);
  };

  return { anchorOf, endsOf, roundOfNode };
}

/** Whether a message passes the toolbar's kind and participant filters. */
export function passesFilters(m: FlowMessage, kinds: ReadonlySet<KindFilter>, participant: string | null): boolean {
  const k = KIND_FILTERS.find((f) => f.kinds.includes(m.message.kind));
  if (k && !kinds.has(k.key)) return false;
  if (participant === null) return true;
  const msg = m.message;
  return (
    msg.from === participant ||
    msg.to === participant ||
    (msg.delivery?.resolved ?? []).includes(participant) ||
    (msg.delivery?.undelivered ?? []).some((u) => u.to === participant)
  );
}

/** Each board item's anchor as drawn, `<node>|<key>` → the handle it lands
 *  on: its own, or its section's "N earlier" row while it is folded there. */
export function boardAnchors(node: string, win: RailWindow): [string, string][] {
  return [...win.handleOf].map(([key, handle]) => [`${node}|${key}`, handle]);
}

/** The anchors a board shows, as `<node>|<key>`, every item shown. */
export function boardAnchorKeys(node: string, view: BoardSource | null, posts: FlowMessage[]): string[] {
  return boardAnchors(node, railWindow(view, posts, ALL_OPEN, Infinity)).map(([k]) => k);
}

// --- the board's window -----------------------------------------------------

export type RailSection = 'claims' | 'directives' | 'posts';
const ALL_OPEN: ReadonlySet<RailSection> = new Set(['claims', 'directives', 'posts']);
/** Items a section of the flow's rail shows before the rest fold into one
 *  "N earlier" row. */
export const RAIL_LIMIT = 5;
/** The same for a workflow frame's strip. */
export const STRIP_LIMIT = 3;
/** The handle of a section's "N earlier" row. */
export const earlierKey = (section: RailSection) => `earlier:${section}`;

export interface RailPart<T> {
  items: T[];
  /** Folded into the "N earlier" row (0: no row). */
  earlier: number;
}

export interface RailWindow {
  claims: RailPart<ClaimView>;
  directives: RailPart<DirectiveView>;
  posts: RailPart<FlowMessage>;
  /** Every item's key → the handle its arcs land on. */
  handleOf: Map<string, string>;
}

/** A board as its rail shows it: per section the latest `limit` items
 *  (a held claim and a standing directive always), the rest folded into an
 *  "N earlier" row above them unless the section is `open`. Pure: the rail
 *  and the map's arc anchors compute the same window. */
export function railWindow(
  view: BoardSource | null,
  posts: FlowMessage[],
  open: ReadonlySet<RailSection>,
  limit: number,
): RailWindow {
  const handleOf = new Map<string, string>();
  const part = <T>(section: RailSection, list: T[], key: (x: T) => string, keep: (x: T) => boolean): RailPart<T> => {
    const cut = open.has(section) ? 0 : Math.max(0, list.length - limit);
    const items = list.filter((x, i) => i >= cut || keep(x));
    const shown = new Set(items);
    for (const x of list) handleOf.set(key(x), shown.has(x) ? key(x) : earlierKey(section));
    return { items, earlier: list.length - items.length };
  };
  const v = view && !('kind' in view) ? view : null;
  return {
    claims: part('claims', v?.claims ?? [], claimKey, (c) => c.state === 'held'),
    directives: part('directives', v?.directives ?? [], (d) => d.id, (d) => !d.retracted),
    posts: part('posts', posts, (p) => p.id, () => false),
    handleOf,
  };
}

/** Each claim on `view` as a board write from the card that holds it (an
 *  arc onto its place on the board), drawn at rest when `atRest`. */
export function claimMessages(
  spaceId: string,
  view: BoardSource | null,
  writers: ReadonlyMap<string, string>,
  atRest: boolean,
): FlowMessage[] {
  if (!view || 'kind' in view) return [];
  const out: FlowMessage[] = [];
  for (const c of view.claims) {
    const writer = writers.get(c.owner);
    if (!writer) continue;
    const at = Date.parse(c.acquired_at);
    const id = claimKey(c);
    out.push({
      id,
      kind: 'post',
      from: writer,
      to: [BOARD],
      undelivered: [],
      atRest,
      space: spaceId,
      at: Number.isFinite(at) ? at : 0,
      atKnown: Number.isFinite(at),
      body: `claim: ${c.key_known ? c.key : '(key not recorded)'} · ${c.state}`,
      message: {
        id,
        ts: c.acquired_at,
        kind: 'post',
        post_kind: 'claim',
        from: c.owner,
        to: null,
        body: c.key,
        delivery: { resolved: [] },
        seen: [],
      },
    });
  }
  return out;
}

export interface ArcOptions {
  kinds: ReadonlySet<KindFilter>;
  participant: string | null;
  /** The clicked card (its arcs are drawn and lit; a round's are all its round's). */
  focus: string | null;
  hover: string | null;
  /** The selected message. */
  selected: string | null;
  /** Messages landing now (live, or revealed by a replay): drawn while their
   *  packet travels, at rest or not. */
  arriving?: { has(id: string): boolean };
  /** The board anchors drawn now, `<node>|<key>` → the handle it lands on
   *  (`boardAnchors`); a key not in it lands on the board itself. */
  anchors: ReadonlyMap<string, string>;
}

/** An arc that has a card at one end only: the operator's or an off-map
 *  address's message entering the card it reached (`enter`), a card's
 *  message to one of them leaving it (`exit`), or a message that reached
 *  nobody ending in a dead end beside its sender (`dead`). Drawn short, at
 *  that card (its `source` and `target` both). */
export type Stub = 'enter' | 'exit' | 'dead';

export interface Arc {
  id: string;
  source: string;
  target: string;
  targetHandle?: string;
  message: FlowMessage;
  highlighted: boolean;
  stub?: Stub;
  /** What a stub's far end is: `operator` or `off map`. */
  label?: string;
  /** The recipient it never reached, when that is this arc's end. */
  dead?: FlowUndelivered;
}

const outsideLabel = (endpoint: string) => (endpoint === OPERATOR ? 'operator' : 'off map');

/** The arcs to draw for `messages`, one per drawn recipient. */
export function messageArcs(messages: FlowMessage[], router: Router, o: ArcOptions): Arc[] {
  const out: Arc[] = [];
  const lit = [o.focus, o.hover].filter((x): x is string => !!x);
  const focusRound = o.focus?.startsWith('round:') ? Number(o.focus.slice('round:'.length)) : null;
  for (const m of messages) {
    if (!passesFilters(m, o.kinds, o.participant)) continue;
    const ends = router.endsOf(m);
    const touched = lit.some((f) => ends.includes(f));
    const inRound = focusRound !== null && ends.some((e) => router.roundOfNode(e) === focusRound);
    const highlighted = m.id === o.selected || touched;
    if (!m.atRest && !highlighted && !inRound && !o.arriving?.has(m.id)) continue;
    const base = { message: m, highlighted };
    const handleOf = (to: Anchor) => (to.handle ? o.anchors.get(`${to.node}|${to.handle}`) : undefined);
    const deadOf = (t: string) => m.undelivered.find((u) => u.to === t);

    // From the operator or off the map: an entry at each card it reached.
    if (isOutside(m.from)) {
      m.to.forEach((t, i) => {
        const to = router.anchorOf(t, m);
        if (!to) return;
        out.push({
          ...base,
          id: `m:${m.space}:${m.id}:${to.node}:${i}`,
          source: to.node,
          target: to.node,
          targetHandle: handleOf(to),
          stub: 'enter',
          label: outsideLabel(m.from),
          dead: deadOf(t),
        });
      });
      continue;
    }
    const from = router.anchorOf(m.from, m);
    if (!from) continue;
    m.to.forEach((t, i) => {
      const id = `m:${m.space}:${m.id}:${t}:${i}`;
      if (isOutside(t) || isDeadEnd(t)) {
        const dead = deadOf(t);
        out.push({
          ...base,
          id,
          source: from.node,
          target: from.node,
          stub: dead ? 'dead' : 'exit',
          label: dead ? undefined : outsideLabel(t),
          dead,
        });
        return;
      }
      const to = router.anchorOf(t, m);
      if (!to) return;
      // Two ends folded onto one card (a summary, a "+N more", an unloaded
      // frame) say nothing; a card's message to itself (units of one step)
      // keeps its loop.
      if (to.node === from.node && !to.handle && (m.from !== from.node || t !== to.node)) return;
      out.push({
        ...base,
        id: `m:${m.space}:${m.id}:${to.node}:${i}`,
        source: from.node,
        target: to.node,
        targetHandle: handleOf(to),
        dead: deadOf(t),
      });
    });
  }
  return out;
}

/** How many messages touch each drawn card (the rail aside). */
export function messageCounts(messages: FlowMessage[], router: Router): Map<string, number> {
  const c = new Map<string, number>();
  for (const m of messages) {
    for (const id of new Set(router.endsOf(m))) if (id !== RAIL) c.set(id, (c.get(id) ?? 0) + 1);
  }
  return c;
}
