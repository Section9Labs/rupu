// rupu-native run graph — a read-only, left-to-right linear chain of the
// run's steps rendered with @xyflow/react.
//
// Built (not ported) because Okesu's OrchestrationRunCanvas is coupled to
// Okesu's OrchestrationStepView types, fan-out cards, and CP federation. This
// reuses the SAME visual language as that canvas — node-card layout, colored
// status stripe + badge + icon, blue/amber pulsing rings for live state, the
// dotted brand-tinted backdrop — but is typed to rupu's StepResultRecord and
// the live status map derived from rupu's SSE events.

import { useMemo } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react';
import {
  AlertCircle,
  CheckCircle2,
  ChevronRight,
  Cpu,
  Loader2,
  Pause,
  XCircle,
  type LucideIcon,
} from 'lucide-react';
import { cn } from '../lib/cn';
import type { StepResultRecord } from '../lib/api';
import type { StepState } from './StatusPill';
import { resolveStepState, type RunStatusState } from '../lib/runStatus';

import '@xyflow/react/dist/style.css';

const NODE_W = 240;
const NODE_GAP = 60; // horizontal gap between cards

interface RunStepNodeData {
  stepID: string;
  agent?: string | null;
  status: StepState;
  [key: string]: unknown;
}

type RunStepNode = Node<RunStepNodeData>;

function tone(status: StepState): { stripe: string; badge: string; iconColor: string } {
  switch (status) {
    case 'running':
      return { stripe: 'bg-blue-500', badge: 'bg-blue-50 text-blue-700', iconColor: 'text-blue-600' };
    case 'completed':
      return { stripe: 'bg-green-500', badge: 'bg-green-50 text-green-700', iconColor: 'text-green-600' };
    case 'failed':
      return { stripe: 'bg-red-500', badge: 'bg-red-50 text-red-700', iconColor: 'text-red-600' };
    case 'awaiting_approval':
      return { stripe: 'bg-amber-400', badge: 'bg-amber-50 text-amber-700', iconColor: 'text-amber-600' };
    case 'skipped':
      return { stripe: 'bg-slate-300', badge: 'bg-slate-100 text-slate-600', iconColor: 'text-slate-400' };
    default:
      return { stripe: 'bg-slate-300', badge: 'bg-slate-100 text-slate-600', iconColor: 'text-slate-400' };
  }
}

function stepIcon(status: StepState): LucideIcon {
  switch (status) {
    case 'running':
      return Loader2;
    case 'completed':
      return CheckCircle2;
    case 'failed':
      return XCircle;
    case 'awaiting_approval':
      return Pause;
    case 'skipped':
      return ChevronRight;
    default:
      return AlertCircle;
  }
}

function RunStepNodeView({ data, selected }: NodeProps<RunStepNode>) {
  const t = tone(data.status);
  const Icon = stepIcon(data.status);
  const spin = data.status === 'running';
  return (
    <div
      className={cn(
        'bg-panel border rounded-lg shadow-card overflow-hidden text-left',
        selected ? 'ring-2 ring-brand-300 border-brand-200' : 'border-border',
        // High-affordance live-state highlights — the reason the operator
        // opened the run page. Ring + pulse make "happening NOW" unmistakeable.
        data.status === 'running' && 'ring-2 ring-blue-400 shadow-lg',
        data.status === 'awaiting_approval' && 'ring-2 ring-amber-400 animate-pulse',
      )}
      style={{ width: NODE_W }}
    >
      <Handle type="target" position={Position.Left} className="!bg-slate-400 !w-2 !h-2 !border-0" />
      <div className={cn('h-1', t.stripe)} />
      <div className="p-3">
        <div className="flex items-center gap-2 mb-1.5">
          <Icon size={14} className={cn(t.iconColor, spin && 'animate-spin')} />
          <span className="text-xs font-semibold text-ink truncate flex-1">{data.stepID}</span>
          <span
            className={cn(
              'text-[9px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded',
              t.badge,
            )}
          >
            {data.status.replace(/_/g, ' ')}
          </span>
        </div>
        {data.agent && (
          <div className="inline-flex items-center gap-1 text-[10px] uppercase tracking-wide text-brand-700 bg-brand-50 ring-1 ring-brand-200 px-1.5 py-0.5 rounded font-medium">
            <Cpu size={9} />
            {data.agent}
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Right} className="!bg-slate-400 !w-2 !h-2 !border-0" />
    </div>
  );
}

const nodeTypes = { runStep: RunStepNodeView };

interface Props {
  steps: StepResultRecord[];
  /** Live status overrides derived from the SSE stream. */
  live: RunStatusState;
  /** Optional map of step_id → agent for richer node cards. */
  agentByStepId?: Record<string, string>;
}

export default function RunGraph(props: Props) {
  return (
    <ReactFlowProvider>
      <RunGraphInner {...props} />
    </ReactFlowProvider>
  );
}

function RunGraphInner({ steps, live, agentByStepId }: Props) {
  const nodes = useMemo<Node[]>(() => {
    return steps.map((s, i) => {
      const status = resolveStepState(s, live);
      const data: RunStepNodeData = {
        stepID: s.step_id,
        agent: agentByStepId?.[s.step_id],
        status,
      };
      return {
        id: s.step_id,
        type: 'runStep',
        position: { x: i * (NODE_W + NODE_GAP), y: 0 },
        data,
      };
    });
  }, [steps, live, agentByStepId]);

  const edges = useMemo<Edge[]>(() => {
    return steps.slice(0, -1).map((s, i) => {
      const next = steps[i + 1];
      const status = resolveStepState(s, live);
      // Color the edge by the upstream step's status — a quick read of where
      // execution has flowed vs where it has stalled.
      const stroke =
        status === 'completed'
          ? '#16a34a'
          : status === 'running'
            ? '#2563eb'
            : status === 'failed'
              ? '#dc2626'
              : '#94a3b8';
      return {
        id: `${s.step_id}-${next.step_id}`,
        source: s.step_id,
        target: next.step_id,
        type: 'smoothstep',
        markerEnd: { type: MarkerType.ArrowClosed, color: stroke },
        style: { stroke, strokeWidth: 2 },
        animated: status === 'running',
      };
    });
  }, [steps, live]);

  if (steps.length === 0) {
    return (
      <div className="w-full h-[360px] rounded-xl border border-border bg-panel shadow-card flex items-center justify-center text-sm text-ink-dim">
        No steps recorded for this run yet.
      </div>
    );
  }

  return (
    <div className="w-full h-[360px] rounded-xl border border-brand-200 shadow-card overflow-hidden">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        fitView
        fitViewOptions={{ padding: 0.2, maxZoom: 1.0 }}
        proOptions={{ hideAttribution: true }}
        style={{
          background:
            'linear-gradient(to bottom, #faf8ff, #ffffff), radial-gradient(circle at 8px 8px, #e9d5ff 1px, transparent 1px) 0 0 / 16px 16px',
        }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} color="#e9d5ff" />
        <Controls className="!bg-panel !border-border !shadow-card" showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
