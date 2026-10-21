// RunEdge — the run graph's connector. Three layers on one smooth-step path:
//
//   track  the faint route, in the source step's kind color, drawn with the
//          graph on load;
//   fill   the traversed route, a gradient from the source's kind color to the
//          target's, which fills (draws along the path) once the target step
//          is reached;
//   light  on the live frontier only, a short glowing dash that travels toward
//          the running step (amber and slower toward a gate that waits on you).
//
// Motion lives in styles/graph-motion.css; colors arrive resolved in `data`
// (RunGraph's edge memo) so the edge needs no theme lookup of its own.

import { memo, useId } from 'react';
import { getSmoothStepPath, type Edge, type EdgeProps } from '@xyflow/react';
import { svgId, type EdgeTone } from '../../lib/graph/runMotion';

export interface RunEdgeData extends Record<string, unknown> {
  tone: EdgeTone;
  /** The source step's kind color: the fill's start. */
  from: string;
  /** The target step's kind color: the fill's end and the arrowhead. */
  to: string;
  /** The untraversed route. */
  track: string;
  /** The travelling light, on a live or awaiting edge. */
  light: string;
  /** Entrance slot of the source node (see `entranceSlots`). */
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
  const [path] = getSmoothStepPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    borderRadius: 14,
  });
  const on = data.tone !== 'idle';
  const flowing = data.tone === 'live' || data.tone === 'await';
  const head = `M ${targetX - 7} ${targetY - 4.5} L ${targetX - 1} ${targetY} L ${targetX - 7} ${targetY + 4.5}`;

  return (
    <g className="rgx-edge" data-tone={data.tone} style={{ ['--i' as string]: data.slot }}>
      <defs>
        <linearGradient id={gid} gradientUnits="userSpaceOnUse" x1={sourceX} y1={sourceY} x2={targetX} y2={targetY}>
          <stop offset="0" stopColor={data.from} />
          <stop offset="1" stopColor={data.to} />
        </linearGradient>
      </defs>
      <path className="rgx-edge-track" d={path} pathLength={1} stroke={data.track} />
      <path className={on ? 'rgx-edge-fill is-on' : 'rgx-edge-fill'} d={path} pathLength={1} stroke={`url(#${gid})`} />
      {flowing && (
        <>
          <path className="rgx-edge-halo" d={path} pathLength={1} stroke={data.light} />
          <path className="rgx-edge-comet" d={path} pathLength={1} stroke={data.light} />
        </>
      )}
      <path className="rgx-edge-head" d={head} stroke={on ? data.to : data.track} />
    </g>
  );
}

export default memo(RunEdgeView);
