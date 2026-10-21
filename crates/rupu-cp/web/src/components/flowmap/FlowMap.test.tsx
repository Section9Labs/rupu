// @vitest-environment jsdom
// FlowMap — the agentiflow's unfolding map, over the invented `acme-shop-auth`
// fixture through a mocked api. The real xyflow canvas can't mount under
// jsdom (no ResizeObserver / layout), so `ReactFlow` is a thin stand-in that
// renders each node it is handed (not `hidden`) with the registered node
// type, and each edge with its edge type: the map's own node components and
// the run graph's step components render for real.
import '@testing-library/jest-dom/vitest';
import type { ComponentType, ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { Position, type Edge, type Node } from '@xyflow/react';

vi.mock('@xyflow/react', async () => {
  const actual = await vi.importActual<typeof import('@xyflow/react')>('@xyflow/react');
  return {
    ...actual,
    ReactFlow: ({
      nodes,
      edges,
      nodeTypes,
      edgeTypes,
      onNodeClick,
      children,
    }: {
      nodes: Node[];
      edges: Edge[];
      nodeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      edgeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      onNodeClick?: (e: unknown, n: Node) => void;
      children?: ReactNode;
    }) => {
      const shown = new Set(nodes.filter((n) => !n.hidden).map((n) => n.id));
      return (
        <div data-testid="rf">
          {nodes
            .filter((n) => !n.hidden)
            .map((n) => {
              const C = nodeTypes[n.type!];
              return (
                <div key={n.id} data-testid={`node-${n.id}`} onClick={(e) => onNodeClick?.(e, n)}>
                  <C id={n.id} type={n.type} data={n.data} selected={false} zIndex={0} isConnectable={false}
                    positionAbsoluteX={0} positionAbsoluteY={0} dragging={false} draggable={false} selectable deletable />
                </div>
              );
            })}
          <svg>
            {edges
              .filter((e) => shown.has(e.source) && shown.has(e.target))
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

import FlowMap from './FlowMap';
import { ThemeProvider } from '../theme/ThemeProvider';
import { api, type AgentiflowDetail, type RunChild, type RunGraphResponse } from '../../lib/api';
import {
  CHILDREN,
  DETAIL,
  FLOW_BOARD,
  FLOW_ID,
  FLOW_MESSAGES,
  FLOW_PARTICIPANTS,
  GANNET_BOARD,
  GANNET_MESSAGES,
  GANNET_PARTICIPANTS,
  RUN,
  at,
} from '../../lib/flow/fixtures';

/** gannet#1's run graph as `GET /api/runs/:id/graph` serves it. */
const GANNET_RESPONSE = {
  run: {},
  workflow: {
    steps: [
      { id: 'collect', kind: 'step', agent: 'endpoint-collector' },
      { id: 'probe', kind: 'for_each', agent: 'auth-prober', for_each: '{{ endpoints }}' },
      { id: 'report', kind: 'step', agent: 'finding-writer' },
    ],
    edges: [
      { from: 'collect', to: 'probe' },
      { from: 'probe', to: 'report' },
    ],
  },
  step_results: [
    { step_id: 'collect', success: true },
    { step_id: 'probe', success: true },
    { step_id: 'report', success: true },
  ],
  units: [],
} as unknown as RunGraphResponse;

/** The fixture flow runs (round 3 is open); the map is mounted with
 *  `running={false}` so nothing streams: every space is read in pages. */
const FLOW: AgentiflowDetail = DETAIL;

let children: RunChild[] = CHILDREN;

beforeEach(() => {
  children = CHILDREN;
  vi.spyOn(api, 'getAgentiflowChildren').mockImplementation(async () => children);
  vi.spyOn(api, 'getAgentiflowBoard').mockResolvedValue({ kind: 'ok', board: FLOW_BOARD });
  vi.spyOn(api, 'getRunBoard').mockResolvedValue({ kind: 'ok', board: GANNET_BOARD });
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(GANNET_RESPONSE);
  vi.spyOn(api, 'subscribeRunLog').mockReturnValue(() => {});
  vi.spyOn(api, 'subscribeMessages').mockReturnValue(() => {});
  vi.spyOn(api, 'getMessages').mockImplementation(async (route) => ({
    kind: 'ok',
    view:
      route === 'agentiflows'
        ? { participants: FLOW_PARTICIPANTS, messages: FLOW_MESSAGES, legacy: false, space: 'present' }
        : { participants: GANNET_PARTICIPANTS, messages: GANNET_MESSAGES, legacy: false, space: 'present' },
  }));
  vi.spyOn(api, 'getAgentiflowGenerated').mockResolvedValue({
    handle: 'gannet#1',
    name: 'auth-sweep',
    file_name: 'auth-sweep.yaml',
    yaml: 'name: auth-sweep\nsteps:\n  - id: collect\n',
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function mount(detail: AgentiflowDetail = FLOW) {
  const ui = (d: AgentiflowDetail) => (
    <MemoryRouter>
      <ThemeProvider>
        <FlowMap detail={d} running={false} />
      </ThemeProvider>
    </MemoryRouter>
  );
  const r = render(ui(detail));
  return { ...r, rerenderWith: (d: AgentiflowDetail) => r.rerender(ui(d)) };
}

const node = (id: string) => screen.queryByTestId(`node-${id}`);
const track = () => screen.getByTestId('time-track');
const tNow = () => Number(track().getAttribute('aria-valuenow'));

describe('FlowMap', () => {
  it('renders_every_round_child_and_frame', async () => {
    mount();
    await waitFor(() => expect(node(RUN.kite1)).not.toBeNull());
    for (const r of [0, 1, 2, 3]) expect(node(`round:${r}`)).not.toBeNull();
    expect(within(node('round:1')!).getByText(/Generated auth-sweep/)).toBeInTheDocument();
    // Round 0 is finished and old: one summary card stands for its children.
    expect(within(node('summary:0')!).getByText(/5 children · 5 done/)).toBeInTheDocument();
    expect(node(RUN.lynx1)).toBeNull();
    for (const id of [RUN.gannet1, RUN.kite1, RUN.kite2, RUN.kite3, RUN.wren3, RUN.owl1]) expect(node(id)).not.toBeNull();
    expect(within(node(RUN.kite2)!).getByText('provider overloaded (529) after 3 retries')).toBeInTheDocument();
    expect(within(node(RUN.kite3)!).getByText('Reset-token reuse confirmed; tokens valid 24h after use.')).toBeInTheDocument();
    expect(within(node(RUN.wren3)!).getByText('sub-agent')).toBeInTheDocument();
    expect(within(node(RUN.gannet1)!).getByText(/written by the lead in round 1/)).toBeInTheDocument();
    // The summary expands its round.
    fireEvent.click(within(node('summary:0')!).getByRole('button'));
    for (const id of [RUN.lynx1, RUN.lynx2, RUN.lynx3, RUN.wren1, RUN.wren2]) expect(node(id)).not.toBeNull();
    expect(node('summary:0')).toBeNull();
    // The flow's board is the rail; arcs are drawn between the cards.
    expect(within(node('rail')!).getByText('engagement report')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_015')).toBeInTheDocument());
    expect(node('operator')).not.toBeNull();
  });

  it('frame_loads_its_graph_on_first_view', async () => {
    const { rerenderWith } = mount();
    await waitFor(() => expect(node(`${RUN.gannet1}/collect`)).not.toBeNull());
    expect(api.getRunGraph).toHaveBeenCalledTimes(1);
    expect(api.getRunGraph).toHaveBeenCalledWith(RUN.gannet1);
    // The run graph's own components: a step card, the fan-out.
    expect(within(node(`${RUN.gannet1}/collect`)!).getByTestId('rg-node')).toBeInTheDocument();
    expect(within(node(`${RUN.gannet1}/probe`)!).getByText('for each')).toBeInTheDocument();
    // Its own space's arcs and board strip.
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_011')).toBeInTheDocument());
    expect(within(node(RUN.gannet1)!).getByText('/api/tokens')).toBeInTheDocument();
    rerenderWith({ ...FLOW });
    await waitFor(() => expect(api.getAgentiflowChildren).toHaveBeenCalledTimes(2));
    expect(api.getRunGraph).toHaveBeenCalledTimes(1);
  });

  it('generated_frame_opens_its_definition', async () => {
    mount();
    await waitFor(() => expect(node(RUN.gannet1)).not.toBeNull());
    fireEvent.click(within(node(RUN.gannet1)!).getByRole('button', { name: /definition/ }));
    expect(api.getAgentiflowGenerated).toHaveBeenCalledWith(FLOW_ID, 'gannet#1');
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(within(dialog).getByText(/name: auth-sweep/)).toBeInTheDocument());
  });

  it('a_poll_keeps_pan_expansion_and_replay', async () => {
    const { rerenderWith } = mount();
    await waitFor(() => expect(node('summary:0')).not.toBeNull());
    fireEvent.click(within(node('summary:0')!).getByRole('button'));
    expect(node(RUN.lynx1)).not.toBeNull();
    // Mid-replay: scrubbed to about minute 13.
    fireEvent.keyDown(track(), { key: 'Home' });
    fireEvent.keyDown(track(), { key: 'ArrowRight', shiftKey: true });
    const t = tNow();
    expect(screen.getByTestId('time-scrubber')).toBeInTheDocument();

    // A poll brings a child launched in round 0 at minute 4.
    const late: RunChild = {
      ...CHILDREN[0],
      handle: 'lynx#4',
      address: 'lynx#4',
      codename: 'cobalt-harbor/heron>lynx#4',
      run_id: 'run_01M5LYNX4',
      name: 'header-checker',
      started_at: at(4),
      finished_at: at(8),
      output_excerpt: 'No security headers on /admin.',
    };
    children = [...CHILDREN, late];
    rerenderWith({ ...FLOW });
    await waitFor(() => expect(node(late.run_id)).not.toBeNull());
    expect(tNow()).toBe(t);
    expect(screen.getByTestId('time-scrubber')).toBeInTheDocument();
    expect(node(RUN.lynx1)).not.toBeNull();
    expect(node('summary:0')).toBeNull();
  });

  it('replay_hides_then_reveals_in_time_order', async () => {
    mount();
    await waitFor(() => expect(node(RUN.kite1)).not.toBeNull());
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_015')).toBeInTheDocument());
    // The start: only round 0 has begun.
    fireEvent.keyDown(track(), { key: 'Home' });
    expect(node('round:0')).not.toBeNull();
    for (const id of ['round:1', 'round:2', RUN.gannet1, RUN.kite1]) expect(node(id)).toBeNull();
    expect(screen.queryByTestId('msg-arc-msg_015')).toBeNull();
    // Forward: round 1 and its frame arrive before round 2 does.
    for (let i = 0; i < 3; i++) fireEvent.keyDown(track(), { key: 'ArrowRight', shiftKey: true });
    expect(node('round:1')).not.toBeNull();
    expect(node(RUN.gannet1)).not.toBeNull();
    expect(node('round:2')).toBeNull();
    expect(node(RUN.kite1)).toBeNull();
    // To the end: everything, the late message's arc too.
    fireEvent.keyDown(track(), { key: 'End' });
    expect(node('round:2')).not.toBeNull();
    expect(node(RUN.kite1)).not.toBeNull();
    expect(screen.getByTestId('msg-arc-msg_015')).toBeInTheDocument();
  });
});
