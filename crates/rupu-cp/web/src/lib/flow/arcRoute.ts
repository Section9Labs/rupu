// Where a message arc runs (spec `2026-10-21-rupu-agentiflow-flow-design.md`,
// anti-goal "spaghetti at real density"). Pure; no React. An arc leaves its
// sender's card from a side, runs straight along the sender's row to a lane
// in the gutter next to the receiving column, runs along that lane and
// enters the receiver from the side that faces it; a board write ends with a
// short horizontal approach to its item on the rail. When the sender's row
// is blocked by another card on the way, the run drops into the gutter
// beside the sender, crosses at the nearest free row gap, and climbs to the
// receiver in its lane (`obstacles`). Corners are rounded when drawn
// (`roundedPath`). A stub (`Stub` in flowEdges) is a short hook
// at the one card it touches: an entry from the operator or an address off
// the map, an exit to them, or a dead end beside the sender.

import type { Pt } from '../graph/polyline';
import type { Stub } from './flowEdges';

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A card's lane sits this far into the gutter beside the column it serves
 *  (the run graph's tree trunks run down the gutter's middle). */
export const LANE = 14;
/** The board rail's lane: its arcs gather in the wide gutter before it. */
export const RAIL_LANE = 28;
/** How far a lane may shift so arcs sharing it read as separate lines. */
const SPREAD = 2;
/** A gutter narrower than this has no room for a lane. */
const MIN_GUTTER = 2 * LANE + 4;
/** Corner rounding, for `roundedPath`. */
export const ARC_RADIUS = 10;

/** A small, stable spread for `key`: -SPREAD..SPREAD. */
export function spreadOf(key: string): number {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
  return (Math.abs(h) % (2 * SPREAD + 1)) - SPREAD;
}

const isBox = (b: Box | Pt): b is Box => 'width' in b;
const midY = (b: Box) => b.y + b.height / 2;

/** Clearance kept from a card a run passes. */
const CLEAR = 6;

/** Whether a horizontal run at `y` from `x1` to `x2` passes no obstacle. */
function clearRow(obstacles: readonly Box[], y: number, x1: number, x2: number): boolean {
  const [a, b] = x1 < x2 ? [x1, x2] : [x2, x1];
  return !obstacles.some((o) => y > o.y - CLEAR && y < o.y + o.height + CLEAR && b > o.x && a < o.x + o.width);
}

/** The free row nearest `y` for a run from `x1` to `x2`: `y` itself, else
 *  the middle of a gap above or below a card in the way (of two as near,
 *  the one toward `toward`, where the run is going). */
function freeRow(obstacles: readonly Box[], y: number, x1: number, x2: number, toward: number): number {
  if (clearRow(obstacles, y, x1, x2)) return y;
  const [a, b] = x1 < x2 ? [x1, x2] : [x2, x1];
  const near = obstacles.filter((o) => b > o.x && a < o.x + o.width);
  const tries = near
    .flatMap((o) => [o.y - 12, o.y + o.height + 12])
    .sort((p, q) => Math.abs(p - y) - Math.abs(q - y) || Math.abs(p - toward) - Math.abs(q - toward));
  return tries.find((t) => clearRow(obstacles, t, x1, x2)) ?? y;
}

/** Out of the sender's side at `y`, across at a free row, along `lane` to
 *  `end` (`outX`: the gutter beside the sender, for the drop to that row). */
function across(start: Pt, outX: number, lane: number, end: Pt, obstacles: readonly Box[]): Pt[] {
  const row = freeRow(obstacles, start.y, start.x, lane, end.y);
  if (row === start.y) return [start, { x: lane, y: start.y }, { x: lane, y: end.y }, end];
  return [start, { x: outX, y: start.y }, { x: outX, y: row }, { x: lane, y: row }, { x: lane, y: end.y }, end];
}

/** The route from `from` to `to`: a card, or a board item's anchor (its
 *  left edge). `spread` shifts the lanes (`spreadOf(message id)`); the
 *  sender's row avoids `obstacles` (the other cards; the ends' own boxes
 *  are left out here). */
export function routeArc(from: Box, to: Box | Pt, spread = 0, obstacles: readonly Box[] = []): Pt[] {
  const right = from.x + from.width;
  const y = midY(from);
  const others = obstacles.filter((o) => o !== from && o !== to && !same(o, from) && !(isBox(to) && same(o, to)));
  if (!isBox(to)) {
    // A board item, approached from its left.
    if (to.x - right >= 2 * RAIL_LANE) {
      const lane = to.x - RAIL_LANE + spread * 4;
      return clean(across({ x: right, y }, right + LANE + spread * 2, lane, to, others));
    }
    // The item is beside or left of the card (a frame's strip under its
    // steps): round the card's left side.
    const lane = Math.min(from.x, to.x) - LANE + spread * 2;
    return [{ x: from.x, y }, { x: lane, y }, { x: lane, y: to.y }, to];
  }
  const tRight = to.x + to.width;
  const ty = midY(to);
  if (to.x - right >= MIN_GUTTER) {
    // A column to the right: the lane in the gutter before it.
    const lane = to.x - LANE + spread * 2;
    return clean(across({ x: right, y }, right + LANE + spread * 2, lane, { x: to.x, y: ty }, others));
  }
  if (from.x - tRight >= MIN_GUTTER) {
    // A column to the left: leave from the left, the lane after it.
    const lane = tRight + LANE + spread * 2;
    return clean(across({ x: from.x, y }, from.x - LANE + spread * 2, lane, { x: tRight, y: ty }, others));
  }
  // The same column: out and back on the right.
  const lane = Math.max(right, tRight) + LANE + spread * 2;
  return [{ x: right, y }, { x: lane, y }, { x: lane, y: ty }, { x: tRight, y: ty }];
}

const same = (a: Box, b: Box) => a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

/** Drops a point that adds no turn (a straight run through it). */
function clean(ps: Pt[]): Pt[] {
  return ps.filter((p, i) => {
    const a = ps[i - 1];
    const b = ps[i + 1];
    if (!a || !b) return true;
    return !((a.x === p.x && p.x === b.x) || (a.y === p.y && p.y === b.y));
  });
}

/** A stub at `at` (a card, or a board item's anchor), from its first point
 *  to its last; `slot` (0..2) keeps a card's stubs apart. */
export function stubRoute(at: Box | Pt, stub: Stub, slot = 0): Pt[] {
  const rise = 26 + slot * 10;
  if (stub === 'dead') {
    // Out of the sender's right side, up, and stop.
    const x = isBox(at) ? at.x + at.width : at.x;
    const y = isBox(at) ? midY(at) : at.y;
    return [{ x, y }, { x: x + 16, y }, { x: x + 16, y: y - rise }, { x: x + 34, y: y - rise }];
  }
  // In at the left side from above-left: the entry; the exit runs it back.
  const x = at.x;
  const y = isBox(at) ? midY(at) : at.y;
  const hook = [{ x: x - 46, y: y - rise }, { x: x - 16, y: y - rise }, { x: x - 16, y }, { x, y }];
  return stub === 'enter' ? hook : hook.reverse();
}
