// MessageEdge — a message as an arc between two cards (xyflow edge `message`).
// The run graph's edge family: same round caps and draw-in, a packet that
// travels once when the message arrives, a dashed red dead end when it
// reached nobody. Hover shows the body. Motion is in styles/graph-motion.css.

import { memo, useState } from 'react';
import type { Edge, EdgeProps } from '@xyflow/react';
import { useThemeColors } from '../../lib/useThemeColors';
import { deadPaint, messagePaint } from '../../lib/flow/messagePaint';
import type { FlowMessage } from '../../lib/flow/flowModel';
import { undeliveredText } from '../../lib/messages';

export interface MessageEdgeData extends Record<string, unknown> {
  message: FlowMessage;
  highlighted: boolean;
  /** Plays the packet once, as the message lands. */
  arriving: boolean;
  /** Selects the message (its list row). */
  onSelect?: (id: string) => void;
}

export type MessageFlowEdge = Edge<MessageEdgeData, 'message'>;

/** Where a dead end stops along its arc. */
const DEAD_AT = 0.72;
const BODY_CHARS = 280;

interface Pt {
  x: number;
  y: number;
}
const lerp = (a: Pt, b: Pt, t: number): Pt => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });
const f = (n: number) => Math.round(n * 10) / 10;

/** The arc: a quadratic bowed to one side of the chord; a loop on the node
 *  for a message from a card to itself. Returns the path from `from` to the
 *  point at `t` (the whole arc at 1), that point, and the arc's midpoint. */
function arc(from: Pt, to: Pt, self: boolean, t: number): { d: string; end: Pt; mid: Pt } {
  if (self) {
    const c1 = { x: from.x + 44, y: from.y - 40 };
    const c2 = { x: from.x + 44, y: from.y + 40 };
    const end = { x: from.x, y: from.y };
    return {
      d: `M ${f(from.x)} ${f(from.y)} C ${f(c1.x)} ${f(c1.y)}, ${f(c2.x)} ${f(c2.y)}, ${f(end.x)} ${f(end.y)}`,
      end,
      mid: { x: from.x + 33, y: from.y },
    };
  }
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const len = Math.hypot(dx, dy) || 1;
  const bow = Math.min(60, len * 0.22);
  const ctl = { x: (from.x + to.x) / 2 + (-dy / len) * bow, y: (from.y + to.y) / 2 + (dx / len) * bow };
  const mid = lerp(lerp(from, ctl, 0.5), lerp(ctl, to, 0.5), 0.5);
  if (t >= 1) return { d: `M ${f(from.x)} ${f(from.y)} Q ${f(ctl.x)} ${f(ctl.y)} ${f(to.x)} ${f(to.y)}`, end: to, mid };
  // De Casteljau: the sub-curve from the start to t.
  const a = lerp(from, ctl, t);
  const b = lerp(ctl, to, t);
  const end = lerp(a, b, t);
  return { d: `M ${f(from.x)} ${f(from.y)} Q ${f(a.x)} ${f(a.y)} ${f(end.x)} ${f(end.y)}`, end, mid };
}

function MessageEdgeView({ id, source, target, sourceX, sourceY, targetX, targetY, data }: EdgeProps<MessageFlowEdge>) {
  const colors = useThemeColors();
  const [hover, setHover] = useState(false);
  if (!data) return null;
  const { message, highlighted, arriving } = data;
  const self = source === target;
  const dead = message.undelivered.find((u) => u.to === target);
  const paint = messagePaint(message.kind, colors);
  const from = { x: sourceX, y: sourceY };
  const to = { x: targetX, y: targetY };
  const full = arc(from, to, self, 1);
  const shown = dead && !self ? arc(from, to, false, DEAD_AT) : full;
  const stroke = dead ? deadPaint(colors).stroke : paint.stroke;
  const body = message.body.length > BODY_CHARS ? `${message.body.slice(0, BODY_CHARS)}…` : message.body;
  const tip = shown.end;

  return (
    <g
      className="afx-msg"
      data-kind={message.kind}
      data-highlighted={highlighted}
      data-dead={dead ? 'true' : undefined}
    >
      <path className="afx-msg-halo" d={shown.d} pathLength={1} stroke={stroke} />
      {dead ? (
        // Dashed in real units (no pathLength), so the dash survives; it
        // fades in rather than drawing.
        <path className="afx-msg-dashed" d={shown.d} stroke={stroke} strokeDasharray={deadPaint(colors).dash} />
      ) : (
        <path className="afx-msg-base" d={shown.d} pathLength={1} stroke={stroke} />
      )}
      {arriving && !dead && <path className="afx-msg-packet" d={shown.d} pathLength={1} stroke={stroke} />}
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
      {hover && (
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
