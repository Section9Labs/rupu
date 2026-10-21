// FlowEdge — the workflow editor's connector, and the line a connection draws
// while it is being dragged (FlowConnectionLine).
//
// A plain edge is a curve whose gradient runs from its source step's kind
// color into its target's (a branch arm keeps its then/else color), over a
// soft halo, the way the run graph draws a path the run took. It draws itself
// from source to target when it appears — a new connection reads as made, not
// swapped in — and a soft light travels along it toward the step it feeds,
// brighter and quicker on the edge you selected. Motion lives in
// styles/graph-motion.css (`wfx-e-*`); colors arrive resolved in `data`.

import { memo, useId } from 'react';
import {
  EdgeText,
  getBezierPath,
  type ConnectionLineComponentProps,
  type Edge,
  type EdgeProps,
} from '@xyflow/react';
import { svgId } from '../../lib/graph/runMotion';

export interface FlowEdgeData extends Record<string, unknown> {
  /** Gradient start (the source step's kind color) — or the arm's color. */
  from: string;
  /** Gradient end (the target step's kind color) — or the arm's color. */
  to: string;
  /** Entrance slot of the source node, staggering the first draw. */
  slot: number;
}

export type FlowFlowEdge = Edge<FlowEdgeData, 'flow'>;

function FlowEdgeView({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  data,
  style,
  markerEnd,
  label,
  labelStyle,
  labelShowBg,
  labelBgStyle,
  labelBgPadding,
  labelBgBorderRadius,
  interactionWidth = 20,
}: EdgeProps<FlowFlowEdge>) {
  const [path, labelX, labelY] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition });
  const gid = svgId('wfx-g', useId());
  const width = Number(style?.strokeWidth ?? 2);
  const from = data?.from ?? String(style?.stroke ?? 'currentColor');
  const to = data?.to ?? from;

  return (
    <g className="wfx-e" style={{ ['--i' as string]: data?.slot ?? 0 }}>
      <defs>
        <linearGradient id={gid} gradientUnits="userSpaceOnUse" x1={sourceX} y1={sourceY} x2={targetX} y2={targetY}>
          <stop offset="0" stopColor={from} />
          <stop offset="1" stopColor={to} />
        </linearGradient>
      </defs>
      <path className="wfx-e-halo" d={path} style={{ stroke: `url(#${gid})` }} />
      <path
        className="wfx-e-base"
        d={path}
        pathLength={1}
        markerEnd={markerEnd}
        style={{ stroke: `url(#${gid})`, strokeWidth: width }}
      />
      <path className="wfx-e-light" d={path} pathLength={1} style={{ stroke: to, strokeWidth: width + 1 }} />
      {interactionWidth > 0 && (
        <path className="react-flow__edge-interaction" d={path} fill="none" strokeOpacity={0} strokeWidth={interactionWidth} />
      )}
      {label ? (
        <EdgeText
          x={labelX}
          y={labelY}
          label={label}
          labelStyle={labelStyle}
          labelShowBg={labelShowBg}
          labelBgStyle={labelBgStyle}
          labelBgPadding={labelBgPadding}
          labelBgBorderRadius={labelBgBorderRadius}
        />
      ) : null}
    </g>
  );
}

export default memo(FlowEdgeView);

/** The line a connection draws while being dragged: from the source handle to
 *  the pointer, ending in a glowing dot that turns green over a valid target
 *  and red over one the graph refuses. */
export function FlowConnectionLine({
  fromX,
  fromY,
  toX,
  toY,
  fromPosition,
  toPosition,
  connectionStatus,
}: ConnectionLineComponentProps) {
  const [path] = getBezierPath({
    sourceX: fromX,
    sourceY: fromY,
    sourcePosition: fromPosition,
    targetX: toX,
    targetY: toY,
    targetPosition: toPosition,
  });
  const tone =
    connectionStatus === 'valid'
      ? 'rgb(var(--c-status-done))'
      : connectionStatus === 'invalid'
        ? 'rgb(var(--c-status-failed))'
        : 'rgb(var(--c-brand-500))';
  return (
    <g className="wfx-cline" data-status={connectionStatus ?? undefined}>
      <path className="wfx-cline-path" d={path} style={{ stroke: tone }} />
      <circle className="wfx-cline-halo" cx={toX} cy={toY} r={9} style={{ fill: tone }} />
      <circle className="wfx-cline-dot" cx={toX} cy={toY} r={3.5} style={{ fill: tone }} />
    </g>
  );
}
