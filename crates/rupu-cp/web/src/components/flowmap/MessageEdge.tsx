// MessageEdge — a message as an arc between two cards (xyflow edge `message`).
// The run graph's edge family: same round caps and draw-in, a packet that
// travels once when the message arrives, a dashed red dead end when it
// reached nobody. Hover shows the body; so does selection (`tip`: on one arc
// of the selected message, until it is unselected). The route (`arcRoute`) leaves a card
// from its side and runs through the gutters, from the card boxes the map
// hands over (`from` / `to`) and xyflow's anchor point for a board item; a
// stub is drawn short at its one card. Motion is in styles/graph-motion.css.

import { memo, useState } from 'react';
import type { Edge, EdgeProps } from '@xyflow/react';
import { useThemeColors } from '../../lib/useThemeColors';
import { deadPaint, messagePaint } from '../../lib/flow/messagePaint';
import type { FlowMessage, FlowUndelivered } from '../../lib/flow/flowModel';
import type { Stub } from '../../lib/flow/flowEdges';
import { ARC_RADIUS, routeArc, spreadOf, stubRoute, type Box } from '../../lib/flow/arcRoute';
import { cutAt, pointAt, roundedPath, type Pt } from '../../lib/graph/polyline';
import { undeliveredText } from '../../lib/messages';

export interface MessageEdgeData extends Record<string, unknown> {
  message: FlowMessage;
  highlighted: boolean;
  /** Plays the packet once, as the message lands. */
  arriving: boolean;
  /** Selects the message (its list row). */
  onSelect?: (id: string) => void;
  /** The sender's card (a stub's one card), as laid out. */
  from?: Box;
  /** The receiver's card; absent for a board item (xyflow's anchor point). */
  to?: Box;
  stub?: Stub;
  /** The cards an arc's runs keep clear of (shared by every arc). */
  obstacles?: readonly Box[];
  /** A stub's far end: `operator` or `off map`. */
  label?: string;
  /** The recipient this end never reached (else looked up by target). */
  dead?: FlowUndelivered;
  /** Show the body without hover: the arc of the selected message that
   *  carries its tip. */
  tip?: boolean;
  /** The arc leaves from / lands on a fan-out unit's square (its handle's
   *  point, xyflow's source / target point), on the `from` / `to` card. */
  fromUnit?: boolean;
  toUnit?: boolean;
}

export type MessageFlowEdge = Edge<MessageEdgeData, 'message'>;

/** Where a dead end stops along its arc. */
const DEAD_AT = 0.72;
const BODY_CHARS = 280;

const f = (n: number) => Math.round(n * 10) / 10;

/** A card's message to itself (units of one step): a loop over its top,
 *  clear of the ports and the edges at its sides. */
function topLoop(b: Box): { d: string; mid: Pt; start: Pt } {
  const cx = b.x + b.width * 0.72;
  const a = { x: cx - 14, y: b.y };
  const z = { x: cx + 14, y: b.y };
  return {
    d: `M ${f(a.x)} ${f(a.y)} C ${f(a.x - 16)} ${f(a.y - 40)}, ${f(z.x + 16)} ${f(z.y - 40)}, ${f(z.x)} ${f(z.y)}`,
    mid: { x: cx, y: b.y - 30 },
    start: z,
  };
}

/** The same without a box: a loop off the port. */
function loop(from: Pt): { d: string; mid: Pt } {
  const c1 = { x: from.x + 44, y: from.y - 40 };
  const c2 = { x: from.x + 44, y: from.y + 40 };
  return {
    d: `M ${f(from.x)} ${f(from.y)} C ${f(c1.x)} ${f(c1.y)}, ${f(c2.x)} ${f(c2.y)}, ${f(from.x)} ${f(from.y)}`,
    mid: { x: from.x + 33, y: from.y },
  };
}

const slotOf = (key: string) => Math.abs(spreadOf(key)) % 3;

/** Two unit squares of one card: an arch over the grid from one square to
 *  the other (a unit's message to itself: a small loop over its square). */
export function unitArch(a: Pt, b: Pt): { d: string; mid: Pt } {
  if (Math.abs(a.x - b.x) < 1 && Math.abs(a.y - b.y) < 1) {
    return {
      d: `M ${f(a.x - 4)} ${f(a.y - 6)} C ${f(a.x - 14)} ${f(a.y - 30)}, ${f(a.x + 14)} ${f(a.y - 30)}, ${f(a.x + 4)} ${f(a.y - 6)}`,
      mid: { x: a.x, y: a.y - 24 },
    };
  }
  const top = Math.min(a.y, b.y) - (16 + Math.abs(b.x - a.x) * 0.12);
  return {
    d: `M ${f(a.x)} ${f(a.y)} C ${f(a.x)} ${f(top)}, ${f(b.x)} ${f(top)}, ${f(b.x)} ${f(b.y)}`,
    mid: { x: (a.x + b.x) / 2, y: top + (Math.min(a.y, b.y) - top) * 0.25 },
  };
}

/** A card's box narrowed to the row at `y` (a unit's square): an arc enters
 *  and leaves it at that height. */
const rowOf = (b: Box, y: number): Box => ({ x: b.x, y: y - 1, width: b.width, height: 2 });
const sameBox = (a: Box, b: Box | undefined) =>
  !!b && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

function MessageEdgeView({
  id,
  source,
  target,
  targetHandleId,
  sourceX,
  sourceY,
  targetX,
  targetY,
  data,
}: EdgeProps<MessageFlowEdge>) {
  const colors = useThemeColors();
  const [hover, setHover] = useState(false);
  if (!data) return null;
  const { message, highlighted, arriving, stub } = data;
  const dead = data.dead ?? message.undelivered.find((u) => u.to === target);
  const paint = messagePaint(message.kind, colors);
  // A board item's anchor; a card's box when the map gave one.
  const point = { x: targetX, y: targetY };
  const source0 = data.from ?? { x: sourceX, y: sourceY, width: 0, height: 0 };
  let full: { d: string; mid: Pt };
  let shownD: string;
  let tip: Pt;
  let far: Pt | null = null;
  if (stub) {
    // A board item's stub sits at its anchor, a card's at the card.
    const at = targetHandleId ? point : (data.from ?? point);
    const ps = stubRoute(at, stub, slotOf(id));
    const d = roundedPath(ps, 8);
    full = { d, mid: pointAt(ps, 0.5) };
    shownD = d;
    tip = ps[ps.length - 1];
    far = stub === 'enter' ? ps[0] : ps[ps.length - 1];
  } else if (source === target && data.fromUnit && data.toUnit) {
    full = unitArch({ x: sourceX, y: sourceY }, { x: targetX, y: targetY });
    shownD = full.d;
    tip = { x: targetX, y: targetY };
  } else if (source === target && !targetHandleId) {
    const l = data.from ? topLoop(data.from) : null;
    full = l ?? loop({ x: sourceX, y: sourceY });
    shownD = full.d;
    tip = l?.start ?? { x: sourceX, y: sourceY };
  } else if (data.fromUnit || data.toUnit) {
    // The cards' sides at the units' rows, then into the squares.
    const from = data.fromUnit && data.from ? rowOf(data.from, sourceY) : source0;
    const to = data.toUnit && data.to ? rowOf(data.to, targetY) : targetHandleId && !data.toUnit ? point : (data.to ?? point);
    const obstacles = (data.obstacles ?? []).filter((o) => !sameBox(o, data.from) && !sameBox(o, data.to));
    let ps = routeArc(from, to, spreadOf(message.id), obstacles);
    if (data.fromUnit) ps = [{ x: sourceX, y: sourceY }, ...ps];
    if (data.toUnit) ps = [...ps, { x: targetX, y: targetY }];
    full = { d: roundedPath(ps, ARC_RADIUS), mid: pointAt(ps, 0.5) };
    const cut = dead ? cutAt(ps, DEAD_AT) : ps;
    shownD = dead ? roundedPath(cut, ARC_RADIUS) : full.d;
    tip = cut[cut.length - 1];
  } else {
    const ps = routeArc(source0, targetHandleId ? point : (data.to ?? point), spreadOf(message.id), data.obstacles);
    full = { d: roundedPath(ps, ARC_RADIUS), mid: pointAt(ps, 0.5) };
    const cut = dead ? cutAt(ps, DEAD_AT) : ps;
    shownD = dead ? roundedPath(cut, ARC_RADIUS) : full.d;
    tip = cut[cut.length - 1];
  }
  const stroke = dead ? deadPaint(colors).stroke : paint.stroke;
  const body = message.body.length > BODY_CHARS ? `${message.body.slice(0, BODY_CHARS)}…` : message.body;

  return (
    <g
      className="afx-msg"
      data-kind={message.kind}
      data-highlighted={highlighted}
      data-dead={dead ? 'true' : undefined}
    >
      <path className="afx-msg-halo" d={shownD} pathLength={1} stroke={stroke} />
      {dead ? (
        // Dashed in real units (no pathLength), so the dash survives; it
        // fades in rather than drawing.
        <path className="afx-msg-dashed" d={shownD} stroke={stroke} strokeDasharray={deadPaint(colors).dash} />
      ) : (
        <path className="afx-msg-base" d={shownD} pathLength={1} stroke={stroke} />
      )}
      {arriving && !dead && <path className="afx-msg-packet" d={shownD} pathLength={1} stroke={stroke} />}
      {far && data.label && (
        <text className="afx-msg-label" x={f(far.x) - 4} y={f(far.y) + 3} textAnchor="end" fill={stroke}>
          {data.label}
        </text>
      )}
      {dead && (
        <g className="afx-msg-dead" transform={`translate(${f(tip.x)} ${f(tip.y)})`} stroke={deadPaint(colors).stroke}>
          <title>{undeliveredText({ to: dead.address, reason: dead.reason })}</title>
          <path d="M -4 -4 L 4 4 M 4 -4 L -4 4" />
        </g>
      )}
      <path
        data-testid={`msg-arc-${message.id}`}
        className="afx-msg-hit"
        d={full.d}
        tabIndex={0}
        role="button"
        aria-label={`${message.kind} message from ${message.message.from_codename ?? message.message.from}`}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        onFocus={() => setHover(true)}
        onBlur={() => setHover(false)}
        onClick={() => data.onSelect?.(message.id)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            data.onSelect?.(message.id);
          }
        }}
      />
      {(hover || data.tip) && (
        <foreignObject
          x={f(full.mid.x) - 130}
          y={f(full.mid.y) - 8}
          width={260}
          height={140}
          style={{ overflow: 'visible', pointerEvents: 'none' }}
        >
          <div className="afx-msg-tip" data-edge={id}>
            <div className="afx-msg-tip-head">
              <span className="afx-msg-tip-dot" style={{ background: stroke }} />
              {message.kind} · {message.message.from_codename ?? message.message.from}
            </div>
            <div className="afx-msg-tip-body">{body}</div>
            {dead && (
              <div className="afx-msg-tip-reason" data-testid="msg-reason">
                not delivered: {undeliveredText({ to: dead.address, reason: dead.reason })}
              </div>
            )}
          </div>
        </foreignObject>
      )}
    </g>
  );
}

export default memo(MessageEdgeView);
