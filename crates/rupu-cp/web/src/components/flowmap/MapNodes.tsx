// The agentiflow map's own small nodes (FlowMap): the lead's spine with a
// dot per round (its live end breathes), the lead's start, the flow's board
// as a rail on the canvas, and the operator's (or an unknown address's)
// entry left of the spine.

import type { ReactNode } from 'react';
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react';
import { RAIL_W } from '../../lib/flow/flowLayout';
import type { FlowMessage } from '../../lib/flow/flowModel';
import BoardRail from './BoardRail';

export interface SpineData extends Record<string, unknown> {
  height: number;
  dots: number[];
  live: boolean;
  tint: string;
}

export function SpineNode({ data }: NodeProps<Node<SpineData, 'spine'>>) {
  const top = data.dots[0] ?? 0;
  const end = data.dots[data.dots.length - 1] ?? top;
  return (
    <svg className="afx-spine-svg" width={24} height={Math.max(end + 24, 1)} aria-hidden style={{ color: data.tint }}>
      <line className="afx-spine-line" x1={12} y1={0} x2={12} y2={end} pathLength={1} />
      {data.dots.map((y, i) => (
        <circle key={i} className="afx-spine-dot" cx={12} cy={y} r={4} style={{ ['--i' as string]: Math.min(i, 10) }} />
      ))}
      {data.live && (
        <>
          <circle className="afx-spine-ring" cx={12} cy={end} r={5} />
          <circle className="afx-spine-dot" cx={12} cy={end} r={5} />
        </>
      )}
    </svg>
  );
}

export interface LeadData extends Record<string, unknown> {
  codename: string;
  agent: string | null;
  startedAt: string;
}

export function LeadNode({ data }: NodeProps<Node<LeadData, 'lead'>>) {
  const t = Date.parse(data.startedAt);
  return (
    <div className="afx-lead" data-testid="flow-lead">
      <b>Engagement started</b>
      <span>
        lead {data.codename}
        {data.agent ? ` · ${data.agent}` : ''}
        {Number.isFinite(t) ? ` · ${new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}
      </span>
    </div>
  );
}

export interface RailData extends Record<string, unknown> {
  view: Parameters<typeof BoardRail>[0]['board'];
  posts: FlowMessage[];
  minHeight: number;
  onSelect: (id: string) => void;
  anchor: (key: string) => ReactNode;
}

export function RailNode({ data }: NodeProps<Node<RailData, 'rail'>>) {
  return (
    <div className="afx-rail-node nodrag nowheel" style={{ width: RAIL_W, minHeight: data.minHeight }}>
      <Handle type="target" position={Position.Left} className="gx-port" />
      <div className="afx-rail-title">Board</div>
      <BoardRail board={data.view} posts={data.posts} onSelect={data.onSelect} anchor={data.anchor} />
    </div>
  );
}

export interface EntryData extends Record<string, unknown> {
  label: string;
}

export function EntryNode({ data }: NodeProps<Node<EntryData, 'entry'>>) {
  return (
    <div className="afx-entry" data-testid={`flow-entry-${data.label}`}>
      {data.label}
      <Handle type="source" position={Position.Right} className="gx-port" />
      <Handle type="target" position={Position.Right} className="gx-port" />
    </div>
  );
}
