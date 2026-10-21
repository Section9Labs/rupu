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
/** `<message>:<source>-><target>`; a stub as `<message>:<stub>@<card>`. */
const arcPairs = () =>
  arcs().map((e) => {
    const d = e.data as { message: { id: string }; stub?: string };
    return d.stub ? `${d.message.id}:${d.stub}@${e.source}` : `${d.message.id}:${e.source}->${e.target}`;
  });
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
    // board post onto its item on the rail; the operator's (and an off-map
    // sender's) as a short entry at each card it reached, the message that
    // reached nobody as a dead end beside its sender.
    expect(arcPairs()).toEqual(
      expect.arrayContaining([
        'msg_r3:collect->rail',
        'msg_r4:enter@collect',
        'msg_r4:enter@probe',
        'msg_r4:enter@report',
        'msg_r1:probe->probe',
        'msg_r2:probe->report',
        'msg_r5:dead@report',
        'msg_r6:enter@report',
        `claim:package-lock.json:${RUN_BOARD.claims[0].acquired_at}:collect->rail`,
      ]),
    );
    expect(arcs().find((e) => e.id.startsWith('m:run:msg_r6:'))!.data).toMatchObject({ label: 'off map' });
    const post = arcs().find((e) => e.id.startsWith('m:run:msg_r3:'))!;
    expect(post.targetHandle).toBe('msg_r3');
    // The rail is on the canvas, to the right of every card.
    const rail = rf.nodes.find((n) => n.id === 'rail')!;
    expect(rail.type).toBe('rail');
    expect(rail.position.x).toBeGreaterThan(640 + 220);
    const railEl = screen.getByTestId('node-rail');
    expect(within(railEl).getByText('claim: package-lock.json')).toBeInTheDocument();
    expect(within(railEl).getByText('package-lock.json')).toBeInTheDocument();
    // No entry nodes: nothing but the steps and the rail is on the canvas.
    expect(rf.nodes.map((n) => n.id)).toEqual(['collect', 'probe', 'report', 'rail']);
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

  it('a_busy_run_badges_its_cards_and_counts_its_messages', () => {
    // Past ARC_REST_LIMIT only the operator's arcs are drawn at rest: the
    // cards still say how many messages each sent or received, and the
    // summary chip how many there are.
    const busy = [
      ...RUN_MESSAGES,
      ...Array.from({ length: 10 }, (_, i) => ({ ...RUN_MESSAGES[5], id: `msg_x${i}` })),
    ];
    const l = buildRunLayer(RUN_MODEL, RUN_EVENTS, { participants: RUN_PARTICIPANTS, messages: busy }, RUN_BOARD, {
      record: RUN_RECORD,
    });
    render(
      <RunGraph
        model={RUN_MODEL}
        positions={RUN_POSITIONS}
        overlay={{ messages: l.messages, board: l.board.view, writers: l.board.writers, revealed: null }}
      />,
    );
    expect(busy.length).toBeGreaterThan(12);
    expect(arcPairs().every((p) => p.startsWith('msg_r4:'))).toBe(true);
    const badge = (step: string) => within(screen.getByTestId(`node-${step}`)).queryByTestId('flow-mcount');
    // report: msg_r2, msg_r4, msg_r5, msg_r6 and the ten copies of msg_r6.
    expect(badge('report')).toHaveTextContent('14');
    expect(badge('probe')).toHaveTextContent('3');
    expect(badge('collect')).toHaveTextContent('3');
    expect(screen.getByTestId('rg-summary-messages')).toHaveTextContent(`${busy.length} messages`);
  });

  it('a_selected_message_lights_both_its_cards', () => {
    render(<RunGraph model={RUN_MODEL} positions={RUN_POSITIONS} overlay={{ ...overlay(), selected: 'msg_r2' }} />);
    const lit = rf.nodes.filter((n) => (n.data as { lit?: boolean }).lit).map((n) => n.id);
    expect(lit).toEqual(['probe', 'report']);
    expect(screen.getByTestId('node-report').querySelector('[data-lit]')).not.toBeNull();
    expect(screen.getByTestId('node-collect').querySelector('[data-lit]')).toBeNull();
    // Its body shows without hover.
    expect(screen.getByText('Token reuse confirmed on /api/tokens.', { selector: '.afx-msg-tip-body' })).toBeInTheDocument();
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
    // The summary counts the steps shown so far.
    expect(screen.getByTestId('rg-summary')).toHaveTextContent('1 done');
  });

  it('a_claim_writer_off_canvas_enters_at_its_item', () => {
    // A claim's owner no step answers to: a short entry into its item.
    const board = { ...RUN_BOARD, claims: [{ ...RUN_BOARD.claims[0], owner: 'ghost#2' }] };
    render(
      <RunGraph
        model={RUN_MODEL}
        positions={RUN_POSITIONS}
        overlay={{ messages: [], board, writers: new Map([['ghost#2', 'offcanvas']]), revealed: null }}
      />,
    );
    expect(rf.nodes.map((n) => n.id)).not.toContain('offcanvas');
    expect(arcPairs()).toEqual([`claim:package-lock.json:${RUN_BOARD.claims[0].acquired_at}:enter@rail`]);
    expect(arcs()[0].targetHandle).toBe(`claim:package-lock.json:${RUN_BOARD.claims[0].acquired_at}`);
  });
});
