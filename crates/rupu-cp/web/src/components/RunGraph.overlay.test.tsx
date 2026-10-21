// @vitest-environment jsdom
// RunGraph's message and board layer (`overlay`): the run's messages as arcs
// between its step cards, the operator's (and an unknown address's) entry,
// the board as a rail on the right with each post's arc landing on it, and
// the time-driven reveal. The xyflow canvas can't mount under jsdom, so
// `ReactFlow` is a stand-in that renders each node it is handed (not
// `hidden`) with its registered type and records the nodes and edges.
import '@testing-library/jest-dom/vitest';
import type { ComponentType, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { Position, type Edge, type Node } from '@xyflow/react';

const rf = vi.hoisted(() => ({ nodes: [] as Node[], edges: [] as Edge[] }));

vi.mock('@xyflow/react', async () => {
  const actual = await vi.importActual<typeof import('@xyflow/react')>('@xyflow/react');
  return {
    ...actual,
    ReactFlow: ({
      nodes,
      edges,
      nodeTypes,
      edgeTypes,
      children,
    }: {
      nodes: Node[];
      edges: Edge[];
      nodeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      edgeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      children?: ReactNode;
    }) => {
      rf.nodes = nodes;
      rf.edges = edges;
      return (
        <div data-testid="rf">
          {nodes
            .filter((n) => !n.hidden)
            .map((n) => {
              const C = nodeTypes[n.type!];
              return (
                <div key={n.id} data-testid={`node-${n.id}`}>
                  <C id={n.id} type={n.type} data={n.data} selected={false} zIndex={0} isConnectable={false}
                    positionAbsoluteX={0} positionAbsoluteY={0} dragging={false} draggable={false} selectable deletable />
                </div>
              );
            })}
          <svg>
            {edges
              .filter((e) => !e.hidden)
              .map((e) => {
                const E = edgeTypes[e.type!];
                return (
                  <E key={e.id} {...e} sourceX={0} sourceY={0} targetX={100} targetY={50}
                    sourcePosition={Position.Right} targetPosition={Position.Left} />
                );
              })}
          </svg>
          {children}
        </div>
      );
    },
    Controls: () => null,
    MiniMap: () => null,
    Background: () => null,
  };
});

import RunGraph from './RunGraph';
import { buildRunLayer } from '../lib/flow/runLayer';
import {
  RUN_BOARD,
  RUN_EVENTS,
  RUN_MESSAGES,
  RUN_MODEL,
  RUN_PARTICIPANTS,
  RUN_POSITIONS,
  RUN_RECORD,
} from '../lib/flow/runFixtures';

afterEach(() => {
  cleanup();
  rf.nodes = [];
  rf.edges = [];
});

const layer = buildRunLayer(
  RUN_MODEL,
  RUN_EVENTS,
  { participants: RUN_PARTICIPANTS, messages: RUN_MESSAGES },
  RUN_BOARD,
  { record: RUN_RECORD },
);

function overlay(revealed: Set<string> | null = null) {
  return { messages: layer.messages, board: layer.board.view, writers: layer.board.writers, revealed };
}

const arcs = () => rf.edges.filter((e) => e.type === 'message' && !e.hidden);
const arcPairs = () => arcs().map((e) => `${(e.data as { message: { id: string } }).message.id}:${e.source}->${e.target}`);
const shownNodes = () => rf.nodes.filter((n) => !n.hidden).map((n) => n.id);

describe('RunGraph overlay', () => {
  it('no_overlay_is_todays_graph', () => {
    render(<RunGraph model={RUN_MODEL} positions={RUN_POSITIONS} />);
    expect(rf.nodes.map((n) => n.id)).toEqual(['collect', 'probe', 'report']);
    expect(rf.edges.map((e) => e.type)).toEqual(['run', 'run']);
    expect(rf.nodes.every((n) => !n.hidden)).toBe(true);
  });

  it('overlay_draws_arcs_and_rail', () => {
    render(<RunGraph model={RUN_MODEL} positions={RUN_POSITIONS} overlay={overlay()} />);
    // The step graph is still the step graph.
    expect(rf.edges.filter((e) => e.type === 'run').map((e) => e.id)).toEqual(['collect->probe', 'probe->report']);
    // Arcs between the steps' cards: a loop for the units of one step, the
    // operator from its entry, the board post onto its item on the rail, and
    // the dead end off canvas.
    expect(arcPairs()).toEqual(
      expect.arrayContaining([
        'msg_r3:collect->rail',
        'msg_r4:operator->collect',
        'msg_r4:operator->probe',
        'msg_r4:operator->report',
        'msg_r1:probe->probe',
        'msg_r2:probe->report',
        'msg_r5:report->offcanvas',
        'msg_r6:offcanvas->report',
        `claim:package-lock.json:${RUN_BOARD.claims[0].acquired_at}:collect->rail`,
      ]),
    );
    const post = arcs().find((e) => e.id.startsWith('m:run:msg_r3:'))!;
    expect(post.targetHandle).toBe('msg_r3');
    // The rail is on the canvas, to the right of every card.
    const rail = rf.nodes.find((n) => n.id === 'rail')!;
    expect(rail.type).toBe('rail');
    expect(rail.position.x).toBeGreaterThan(640 + 220);
    const railEl = screen.getByTestId('node-rail');
    expect(within(railEl).getByText('claim: package-lock.json')).toBeInTheDocument();
    expect(within(railEl).getByText('package-lock.json')).toBeInTheDocument();
    // The entries sit left of the graph.
    expect(screen.getByTestId('flow-entry-operator')).toBeInTheDocument();
    expect(rf.nodes.find((n) => n.id === 'operator')!.position.x).toBeLessThan(0);
    // The dead end draws dashed.
    expect(screen.getByTestId('msg-arc-msg_r5').closest('g')).toHaveAttribute('data-dead', 'true');
  });

  it('a_selected_message_lights_its_arc', () => {
    const onSelect = vi.fn();
    render(
      <RunGraph model={RUN_MODEL} positions={RUN_POSITIONS} overlay={{ ...overlay(), selected: 'msg_r2', onSelect }} />,
    );
    const lit = arcs().filter((e) => (e.data as { highlighted: boolean }).highlighted);
    expect(lit.map((e) => e.source + '->' + e.target)).toEqual(['probe->report']);
    screen.getByTestId('msg-arc-msg_r1').dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(onSelect).toHaveBeenCalledWith('msg_r1');
  });

  it('reveal_hides_what_has_not_happened_yet', () => {
    // As of minute 4: collect ran, the claim and its post landed.
    const at4 = new Set(['collect', 'msg_r3', `claim:run:package-lock.json:${RUN_BOARD.claims[0].acquired_at}`]);
    render(<RunGraph model={RUN_MODEL} positions={RUN_POSITIONS} overlay={overlay(at4)} />);
    expect(shownNodes()).toEqual(expect.arrayContaining(['collect', 'rail']));
    expect(shownNodes()).not.toContain('probe');
    expect(shownNodes()).not.toContain('report');
    // The step edge into a step not yet started is hidden too.
    expect(rf.edges.find((e) => e.id === 'collect->probe')!.hidden).toBe(true);
    expect(arcPairs().map((p) => p.split(':')[0])).toEqual(expect.arrayContaining(['msg_r3', 'claim']));
    expect(arcPairs().some((p) => p.startsWith('msg_r2'))).toBe(false);
    expect(within(screen.getByTestId('node-rail')).getByText('claim: package-lock.json')).toBeInTheDocument();
  });
});
