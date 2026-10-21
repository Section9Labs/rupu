// RunEdge — the run graph's connector: a curve whose stroke runs from the
// source step's kind color into the target's. It draws itself from source to
// target as the graph arrives. Untaken, it sits faint; once the run has come
// this way it is solid with a soft luminous halo; into a running step a glow
// travels along it toward the step (amber and slower toward a gate that waits
// on the operator); out of a failed step it is a faint dashed red dead end.
// Motion lives in styles/graph-motion.css; colors arrive resolved in `data`.

import { memo, useId } from 'react';
import { getBezierPath, type Edge, type EdgeProps } from '@xyflow/react';
import { svgId, type EdgeTone } from '../../lib/graph/runMotion';

export interface RunEdgeData extends Record<string, unknown> {
  tone: EdgeTone;
  /** The source step's kind color: the stroke's start. */
  from: string;
  /** The target step's kind color: the stroke's end. */
  to: string;
  /** The travelling glow, on a live or awaiting edge. */
  light: string;
  /** Entrance slot of the source node (see `useEntranceSlots`). */
  slot: number;
}

export type RunFlowEdge = Edge<RunEdgeData, 'run'>;

function RunEdgeView({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  data,
}: EdgeProps<RunFlowEdge>) {
  const gid = svgId('rgx-g', useId());
  if (!data) return null;
  const [path] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition });
  const flowing = data.tone === 'live' || data.tone === 'await';

  return (
    <g className="gx-edge" data-tone={data.tone} style={{ ['--i' as string]: data.slot }}>
      <defs>
        <linearGradient id={gid} gradientUnits="userSpaceOnUse" x1={sourceX} y1={sourceY} x2={targetX} y2={targetY}>
          <stop offset="0" stopColor={data.from} />
          <stop offset="1" stopColor={data.to} />
        </linearGradient>
      </defs>
      <path className="gx-e-halo" d={path} pathLength={1} stroke={`url(#${gid})`} />
      <path className="gx-e-base" d={path} pathLength={1} stroke={`url(#${gid})`} />
      <path className="gx-e-dead" d={path} />
      {flowing && (
        <>
          <path className="gx-e-comet" d={path} pathLength={1} stroke={data.light} />
          <path className="gx-e-core" d={path} pathLength={1} stroke={data.light} />
        </>
      )}
    </g>
  );
}

export default memo(RunEdgeView);
