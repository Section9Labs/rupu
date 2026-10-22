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
import { MemoryRouter, useLocation } from 'react-router-dom';
import { Position, type Edge, type Node } from '@xyflow/react';

/** What the stand-in canvas was last handed, and the viewport calls. */
const rf = vi.hoisted(() => ({
  nodes: [] as unknown[],
  edges: [] as unknown[],
  onMoveStart: undefined as ((e: unknown) => void) | undefined,
  instance: {
    viewportInitialized: true,
    setViewport: vi.fn(async () => true),
    getViewport: vi.fn(() => ({ x: 0, y: 0, zoom: 1 })),
    setCenter: vi.fn(async () => true),
    fitView: vi.fn(async () => true),
  },
}));

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
      onMoveStart,
      children,
    }: {
      nodes: Node[];
      edges: Edge[];
      nodeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      edgeTypes: Record<string, ComponentType<Record<string, unknown>>>;
      onNodeClick?: (e: unknown, n: Node) => void;
      onMoveStart?: (e: unknown) => void;
      children?: ReactNode;
    }) => {
      rf.nodes = nodes;
      rf.edges = edges;
      rf.onMoveStart = onMoveStart;
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
    useReactFlow: () => rf.instance,
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
  units: [0, 1].map((index) => ({
    step_id: 'probe',
    index,
    item: `/api/e${index}`,
    run_id: `run_01M5GPROBE${index}`,
    transcript_path: `/home/acme/.rupu/runs/run_01M5GPROBE${index}/transcript.jsonl`,
    output: '',
    success: true,
    finished_at: at(40),
  })),
} as unknown as RunGraphResponse;

/** The fixture flow runs (round 3 is open); the map is mounted with
 *  `running={false}` so nothing streams: every space is read in pages. */
const FLOW: AgentiflowDetail = DETAIL;

let children: RunChild[] = CHILDREN;
/** The flow's messages at ARC_REST_LIMIT (12), so every arc is at rest; the
 *  dropped one (lynx#2 → lynx#1) folds onto round 0's summary anyway. */
const QUIET_FLOW = FLOW_MESSAGES.filter((m) => m.id !== 'msg_004');

beforeEach(() => {
  children = CHILDREN;
  rf.instance.setViewport.mockClear();
  rf.instance.setCenter.mockClear();
  rf.instance.getViewport.mockImplementation(() => ({ x: 0, y: 0, zoom: 1 }));
  vi.spyOn(api, 'getAgentiflowChildTree').mockImplementation(async () => ({ children, truncated: false }));
  vi.spyOn(api, 'getAgentiflowBoard').mockResolvedValue({ kind: 'ok', board: FLOW_BOARD });
  vi.spyOn(api, 'getRunBoard').mockResolvedValue({ kind: 'ok', board: GANNET_BOARD });
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(GANNET_RESPONSE);
  vi.spyOn(api, 'subscribeRunLog').mockReturnValue(() => {});
  vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});
  vi.spyOn(api, 'readRunLog').mockResolvedValue([]);
  vi.spyOn(api, 'subscribeMessages').mockReturnValue(() => {});
  vi.spyOn(api, 'getMessages').mockImplementation(async (route) => ({
    kind: 'ok',
    view:
      route === 'agentiflows'
        ? { participants: FLOW_PARTICIPANTS, messages: QUIET_FLOW, legacy: false, space: 'present' }
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

function Location() {
  const l = useLocation();
  return <div data-testid="location">{l.pathname + l.search}</div>;
}

function mount(detail: AgentiflowDetail = FLOW, running = false) {
  const ui = (d: AgentiflowDetail) => (
    <MemoryRouter>
      <ThemeProvider>
        <FlowMap detail={d} running={running} />
        <Location />
      </ThemeProvider>
    </MemoryRouter>
  );
  const r = render(ui(detail));
  return { ...r, rerenderWith: (d: AgentiflowDetail) => r.rerender(ui(d)) };
}

const node = (id: string) => screen.queryByTestId(`node-${id}`);

/** Gives the canvas's pane a size (jsdom lays nothing out), so the map
 *  places its viewport. */
function sizedPane(): () => void {
  const w = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
  const h = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 800 });
  return () => {
    if (w) Object.defineProperty(HTMLElement.prototype, 'clientWidth', w);
    if (h) Object.defineProperty(HTMLElement.prototype, 'clientHeight', h);
  };
}
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
    // The operator is no node: its message enters the card it reached.
    expect(node('operator')).toBeNull();
    expect(screen.getByTestId('msg-arc-msg_007').closest('g')).toHaveTextContent('operator');
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
    // Each inner step carries its own message count (the frame's head counts
    // only the flow's space): probe's units wrote all five of the frame's.
    expect(within(node(`${RUN.gannet1}/probe`)!).getByTestId('flow-mcount')).toHaveTextContent('5');
    expect(within(node(`${RUN.gannet1}/collect`)!).queryByTestId('flow-mcount')).toBeNull();
    // A poll that changed the flow (a round ended) re-reads the children,
    // never the loaded frame's graph.
    rerenderWith({ ...FLOW, record: { ...FLOW.record, status: 'completed' } });
    await waitFor(() => expect(api.getAgentiflowChildTree).toHaveBeenCalledTimes(2));
    expect(api.getRunGraph).toHaveBeenCalledTimes(1);
  });

  it('a_failed_lead_round_is_drawn_failed_with_its_reason', async () => {
    const events = FLOW.events.map((e) =>
      e.kind === 'round' && e.round === 2
        ? { ...e, outcome: 'error', error: 'provider overloaded (529); recovery exhausted', decision: undefined }
        : e,
    );
    mount({ ...FLOW, events });
    await waitFor(() => expect(node('round:2')).not.toBeNull());
    const card = node('round:2')!;
    expect(within(card).getByTestId('flow-round-error')).toHaveTextContent('provider overloaded (529); recovery exhausted');
    expect(card.querySelector('.gx-node')).toHaveAttribute('data-state', 'failed');
    expect(node('round:1')!.querySelector('.gx-node')).toHaveAttribute('data-state', 'done');
  });

  it('selecting_a_message_lights_both_its_cards_and_shows_its_body', async () => {
    mount();
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_015')).toBeInTheDocument());
    expect(document.querySelectorAll('[data-lit]')).toHaveLength(0);
    expect(document.querySelector('.afx-msg-tip')).toBeNull();
    // A row picked in the Messages list: the lead's round and kite#3.
    fireEvent.click(screen.getByTestId('msg-row-msg_015'));
    expect(node('round:2')!.querySelector('[data-lit]')).not.toBeNull();
    expect(node(RUN.kite3)!.querySelector('[data-lit]')).not.toBeNull();
    expect(document.querySelectorAll('[data-lit]')).toHaveLength(2);
    expect(screen.getByText('Take reset-token reuse, kite#2 dropped it.', { selector: '.afx-msg-tip-body' })).toBeInTheDocument();
    // Picking the row again clears it.
    fireEvent.click(screen.getByTestId('msg-row-msg_015'));
    expect(document.querySelectorAll('[data-lit]')).toHaveLength(0);
    expect(document.querySelector('.afx-msg-tip')).toBeNull();
  });

  it('a_frames_messages_between_units_end_on_their_squares', async () => {
    // gannet#1's probe with eight units: probe#5..#7 are squares on its card.
    const units = Array.from({ length: 8 }, (_, index) => ({
      ...(GANNET_RESPONSE.units[0] as object),
      index,
      item: `/api/e${index}`,
      run_id: `run_01M5GPROBE${index}`,
    }));
    vi.mocked(api.getRunGraph).mockResolvedValue({ ...GANNET_RESPONSE, units } as unknown as RunGraphResponse);
    mount();
    await waitFor(() => expect(node(`${RUN.gannet1}/probe`)!.querySelectorAll('button.gx-cell')).toHaveLength(8));
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_009')).toBeInTheDocument());
    const arc = (rf.edges as Edge[]).find((e) => e.id.startsWith(`m:${RUN.gannet1}:msg_009:`))!;
    expect(arc).toMatchObject({
      source: `${RUN.gannet1}/probe`,
      target: `${RUN.gannet1}/probe`,
      sourceHandle: 'unit:6',
      targetHandle: 'unit:5',
    });
  });

  it('a_launch_tree_past_the_walks_cap_says_so_in_place', async () => {
    vi.mocked(api.getAgentiflowChildTree).mockImplementation(async () => ({ children, truncated: true }));
    mount();
    expect(await screen.findByTestId('flow-children-truncated')).toHaveTextContent(/sub-agents beyond that are not drawn/);
  });

  it('generated_frame_opens_its_definition', async () => {
    mount();
    await waitFor(() => expect(node(RUN.gannet1)).not.toBeNull());
    fireEvent.click(within(node(RUN.gannet1)!).getByRole('button', { name: /definition/ }));
    expect(api.getAgentiflowGenerated).toHaveBeenCalledWith(FLOW_ID, 'gannet#1');
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(within(dialog).getByText(/name: auth-sweep/)).toBeInTheDocument());
  });

  it('a_workflow_a_deeper_child_generated_is_labelled_by_it_without_a_definition', async () => {
    const lynx1 = CHILDREN.find((c) => c.run_id === RUN.lynx1)!;
    children = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, parent_run_id: RUN.lynx1, depth: 2 } : c));
    mount();
    await waitFor(() => expect(node(RUN.gannet1)).not.toBeNull());
    const frame = node(RUN.gannet1)!;
    expect(within(frame).getByTestId('flow-frame-pen')).toHaveTextContent(`written by ${lynx1.address}`);
    expect(within(frame).queryByRole('button', { name: /definition/ })).toBeNull();
  });

  it('a_poll_keeps_pan_expansion_and_replay', async () => {
    const restore = sizedPane();
    try {
      const { rerenderWith } = mount();
      await waitFor(() => expect(node('summary:0')).not.toBeNull());
      // Placed (and re-fitted as the frame's graph widened the map), then
      // the operator pans.
      await waitFor(() => expect(node(`${RUN.gannet1}/collect`)).not.toBeNull());
      expect(rf.instance.setViewport).toHaveBeenCalled();
      const placed = rf.instance.setViewport.mock.calls.length;
      rf.onMoveStart?.({ type: 'mousedown' });
      fireEvent.click(within(node('summary:0')!).getByRole('button'));
      expect(node(RUN.lynx1)).not.toBeNull();
      // Round 1 is selected.
      fireEvent.click(node('round:1')!);
      // Mid-replay: scrubbed to about minute 13.
      fireEvent.keyDown(track(), { key: 'Home' });
      fireEvent.keyDown(track(), { key: 'ArrowRight', shiftKey: true });
      const t = tNow();
      expect(screen.getByTestId('time-scrubber')).toBeInTheDocument();
      // A scrub starts a replay, which follows its newest card again.
      const scrubbed = rf.instance.setViewport.mock.calls.length;
      expect(scrubbed).toBeGreaterThanOrEqual(placed);

      // A poll brings a child launched in round 0 at minute 4, and round 3's
      // end (round 4 opens), which would fold round 1 were it not selected.
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
      const round3 = { kind: 'round', ts: at(117), round: 3, started_at: at(112), goals_met: 1, goals_total: 2, spent_usd: 41, spent_tokens: 6_000_000, outcome: 'yielded' };
      rerenderWith({ ...FLOW, events: [...FLOW.events, round3] });
      await waitFor(() => expect(node(late.run_id)).not.toBeNull());
      const ids = () => (rf.nodes as { id: string }[]).map((n) => n.id);
      expect(ids()).toContain('round:4');
      expect(tNow()).toBe(t);
      expect(screen.getByTestId('time-scrubber')).toBeInTheDocument();
      expect(node(RUN.lynx1)).not.toBeNull();
      expect(node('summary:0')).toBeNull();
      expect(ids()).toContain(RUN.gannet1);
      expect(ids()).not.toContain('summary:1');
      // The poll moved nothing.
      expect(rf.instance.setViewport).toHaveBeenCalledTimes(scrubbed);
      // Unselected, round 1 folds as the newer rounds push it out.
      fireEvent.keyDown(track(), { key: 'End' });
      fireEvent.click(node('round:1')!);
      expect(ids()).toContain('summary:1');
    } finally {
      restore();
    }
  });

  it('a_running_map_follows_its_newest_round_until_panned', async () => {
    const restore = sizedPane();
    try {
      const { rerenderWith } = mount(FLOW, true);
      await waitFor(() => expect(node(`${RUN.gannet1}/collect`)).not.toBeNull());
      expect(rf.instance.setViewport).toHaveBeenCalled();
      const placed = rf.instance.setViewport.mock.calls.length;
      const ev = (round: number) => ({ kind: 'round', ts: at(117 + round), round, started_at: at(112 + round), goals_met: 1, goals_total: 2, spent_usd: 41, spent_tokens: 6_000_000, outcome: 'yielded' });
      rerenderWith({ ...FLOW, events: [...FLOW.events, ev(3)] });
      // A new newest round: the map follows it.
      await waitFor(() => expect(rf.instance.setViewport).toHaveBeenCalledTimes(placed + 1));
      rf.onMoveStart?.({ type: 'wheel' });
      rerenderWith({ ...FLOW, events: [...FLOW.events, ev(3), ev(4)] });
      await waitFor(() => expect((rf.nodes as { id: string }[]).some((n) => n.id === 'round:5')).toBe(true));
      // Panned away: the next one doesn't move it.
      expect(rf.instance.setViewport).toHaveBeenCalledTimes(placed + 1);
    } finally {
      restore();
    }
  });

  it('advancing_t_without_a_new_item_keeps_the_nodes', async () => {
    mount();
    await waitFor(() => expect(node(`${RUN.gannet1}/collect`)).not.toBeNull());
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_011')).toBeInTheDocument());
    // Into the quiet between minute 66 and round 2 at minute 85.
    fireEvent.keyDown(track(), { key: 'Home' });
    for (let i = 0; i < 6; i++) fireEvent.keyDown(track(), { key: 'ArrowRight', shiftKey: true });
    const nodes = rf.nodes;
    const edges = rf.edges;
    const t = tNow();
    fireEvent.keyDown(track(), { key: 'ArrowRight' });
    expect(tNow()).toBeGreaterThan(t);
    expect(rf.nodes).toBe(nodes);
    expect(rf.edges).toBe(edges);
    // A step that reveals round 2 does change them.
    for (let i = 0; i < 2; i++) fireEvent.keyDown(track(), { key: 'ArrowRight', shiftKey: true });
    expect(rf.nodes).not.toBe(nodes);
  });

  it('a_frames_fanout_unit_opens_its_transcript', async () => {
    mount();
    await waitFor(() => expect(node(`${RUN.gannet1}/probe`)).not.toBeNull());
    const cells = node(`${RUN.gannet1}/probe`)!.querySelectorAll('button.gx-cell');
    expect(cells).toHaveLength(2);
    fireEvent.click(cells[1]);
    expect(screen.getByTestId('location')).toHaveTextContent(
      `/transcript?path=${encodeURIComponent('/home/acme/.rupu/runs/run_01M5GPROBE1/transcript.jsonl')}&live=0`,
    );
  });

  it('a_frame_log_read_in_two_pages_sends_no_packets_on_open', async () => {
    vi.mocked(api.getMessages).mockImplementation(async (route, _id, opts) => {
      if (route === 'agentiflows') {
        return { kind: 'ok', view: { participants: FLOW_PARTICIPANTS, messages: QUIET_FLOW, legacy: false, space: 'present' } };
      }
      // gannet#1's log, in two pages; the second comes a little later.
      const second = opts?.after === 'page-2';
      if (second) await new Promise((r) => setTimeout(r, 30));
      return {
        kind: 'ok',
        view: {
          participants: GANNET_PARTICIPANTS,
          messages: second ? GANNET_MESSAGES.slice(2) : GANNET_MESSAGES.slice(0, 2),
          legacy: false,
          space: 'present',
          next: second || opts?.limit === 1 ? null : 'page-2',
        },
      };
    });
    mount();
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_009')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_011')).toBeInTheDocument());
    expect(document.querySelector('.afx-msg-packet')).toBeNull();
  });

  it('packets_travel_for_new_messages_not_on_open', async () => {
    mount();
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_015')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByTestId('msg-arc-msg_011')).toBeInTheDocument());
    expect(document.querySelector('.afx-msg-packet')).toBeNull();
    // A replay reveals them again: each travels once as it lands.
    fireEvent.keyDown(track(), { key: 'Home' });
    fireEvent.keyDown(track(), { key: 'End' });
    await waitFor(() => expect(document.querySelectorAll('.afx-msg-packet').length).toBeGreaterThan(0));
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

  it('rounds_are_reached_by_navigator_stepper_and_keys', async () => {
    const restore = sizedPane();
    try {
      mount();
      await waitFor(() => expect(node('round:3')).not.toBeNull());
      expect(screen.getByTestId('round-nav')).toBeInTheDocument();
      expect(screen.getByRole('group', { name: 'Rounds' })).toHaveTextContent(/round 0 \/ 3/);
      rf.instance.setViewport.mockClear();
      // A button move is smooth (or a jump under reduced motion).
      fireEvent.click(screen.getByRole('button', { name: 'Next round' }));
      expect(rf.instance.setViewport).toHaveBeenCalledTimes(1);
      // j / k on the focused map jump, with no animation.
      const canvas = screen.getByLabelText(/Flow map/);
      fireEvent.keyDown(canvas, { key: 'j' });
      expect(rf.instance.setViewport).toHaveBeenCalledTimes(2);
      expect(rf.instance.setViewport.mock.calls[1]).toHaveLength(1);
      fireEvent.keyDown(canvas, { key: 'x' });
      expect(rf.instance.setViewport).toHaveBeenCalledTimes(2);
    } finally {
      restore();
    }
  });

  it('a_replay_follows_until_the_operator_moves_the_camera', async () => {
    const restore = sizedPane();
    try {
      mount();
      await waitFor(() => expect(node('round:3')).not.toBeNull());
      fireEvent.keyDown(track(), { key: 'Home' });
      expect(screen.queryByRole('button', { name: 'Follow' })).toBeNull();
      // The operator pans mid-replay: following stops, the pill offers it back.
      rf.onMoveStart?.({ type: 'mousedown' });
      const pill = await screen.findByRole('button', { name: 'Follow' });
      fireEvent.click(pill);
      expect(screen.queryByRole('button', { name: 'Follow' })).toBeNull();
      // Out of the replay: no pill.
      rf.onMoveStart?.({ type: 'mousedown' });
      await screen.findByRole('button', { name: 'Follow' });
      fireEvent.keyDown(track(), { key: 'End' });
      fireEvent.click(screen.getByRole('button', { name: 'Show all' }));
      expect(screen.queryByRole('button', { name: 'Follow' })).toBeNull();
    } finally {
      restore();
    }
  });
});
