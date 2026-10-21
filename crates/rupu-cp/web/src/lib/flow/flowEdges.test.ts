import { describe, expect, it } from 'vitest';
import { KIND_FILTERS, type KindFilter } from '../messages';
import { buildFlowModel, type FlowModel } from './flowModel';
import {
  RAIL,
  boardAnchorKeys,
  claimMessages,
  makeRouter,
  messageArcs,
  messageCounts,
  type ArcOptions,
} from './flowEdges';
import { CHILDREN, DETAIL, FLOW_BOARD, FLOW_MESSAGES, RUN, at, fixtureInput } from './fixtures';

const ALL: ReadonlySet<KindFilter> = new Set(KIND_FILTERS.map((k) => k.key));

/** Every node the model draws, as the map would. */
function drawn(model: FlowModel): Set<string> {
  const v = new Set<string>(['lead', RAIL]);
  for (const n of model.nodes) {
    v.add(n.id);
    if (n.kind === 'frame' && n.graph) for (const s of n.graph.nodes) v.add(`${n.id}/${s.id}`);
  }
  for (const r of model.rounds) if (r.collapsed) v.add(`summary:${r.round}`);
  return v;
}

const opts = (over: Partial<ArcOptions> = {}): ArcOptions => ({
  kinds: ALL,
  participant: null,
  focus: null,
  hover: null,
  selected: null,
  anchors: new Set(),
  ...over,
});

const model = buildFlowModel(fixtureInput());
const msg = (id: string) => model.messages.find((m) => m.id === id)!;

describe('flowEdges', () => {
  it('anchors_fall_back_to_the_summary_the_more_card_or_the_round', () => {
    const router = makeRouter(model, drawn(model));
    // Round 0 is collapsed: its children land on its summary card.
    expect(router.anchorOf(RUN.lynx1, msg('msg_004'))).toEqual({ node: 'summary:0' });
    // The lead lands on the round it was in at the message's time.
    expect(router.anchorOf('lead', msg('msg_001'))).toEqual({ node: 'round:0' });
    expect(router.anchorOf('lead', msg('msg_015'))).toEqual({ node: 'round:2' });
    // An operator message enters at the operator; a board write at its item.
    expect(router.anchorOf('operator', msg('msg_007'))).toEqual({ node: 'operator' });
    expect(router.anchorOf('board', msg('msg_016'))).toEqual({ node: RAIL, handle: 'msg_016' });
    expect(router.anchorOf(`${RUN.gannet1}/probe`, msg('msg_009'))).toEqual({ node: `${RUN.gannet1}/probe` });
    // A frame not drawn: its round; a "+N more" round: its more card.
    const noFrame = drawn(model);
    noFrame.delete(RUN.gannet1);
    noFrame.delete(`${RUN.gannet1}/probe`);
    expect(makeRouter(model, noFrame).anchorOf(`${RUN.gannet1}/probe`, msg('msg_009'))).toEqual({ node: 'round:1' });
    // Nothing drawn for it at all: no anchor.
    noFrame.delete('round:1');
    expect(makeRouter(model, noFrame).anchorOf(`${RUN.gannet1}/probe`, msg('msg_009'))).toBeNull();
    const fake = { ...model, rounds: model.rounds.map((r) => (r.round === 2 ? { ...r, hidden: 1 } : r)) };
    const v = drawn(model);
    v.delete(RUN.kite2);
    v.add('more:2');
    expect(makeRouter(fake, v).anchorOf(RUN.kite2, msg('msg_014'))).toEqual({ node: 'more:2' });
  });

  it('folded_ends_drop_their_loop_but_a_true_self_arc_keeps_it', () => {
    const router = makeRouter(model, drawn(model));
    const arcs = messageArcs(model.messages, router, opts());
    // lynx#2 → lynx#1: both on round 0's summary card: no arc.
    expect(arcs.find((a) => a.message.id === 'msg_004')).toBeUndefined();
    // probe#6 → probe#5: two units of one step, a real loop on it.
    const loop = arcs.find((a) => a.message.id === 'msg_009')!;
    expect(loop.source).toBe(`${RUN.gannet1}/probe`);
    expect(loop.target).toBe(`${RUN.gannet1}/probe`);
    // The undelivered message keeps its dead-end target.
    expect(arcs.some((a) => a.message.id === 'msg_017')).toBe(true);
  });

  it('focus_hover_and_selection_draw_and_light_arcs_off_rest', () => {
    const offRest = { ...model, messages: model.messages.map((m) => ({ ...m, atRest: false })) };
    const router = makeRouter(offRest, drawn(offRest));
    expect(messageArcs(offRest.messages, router, opts())).toEqual([]);
    const kite3 = messageArcs(offRest.messages, router, opts({ focus: RUN.kite3 }));
    expect(new Set(kite3.map((a) => a.message.id))).toEqual(new Set(['msg_013', 'msg_015', 'msg_017', 'msg_018']));
    expect(kite3.every((a) => a.highlighted)).toBe(true);
    const hovered = messageArcs(offRest.messages, router, opts({ hover: RUN.kite2 }));
    expect(new Set(hovered.map((a) => a.message.id))).toEqual(new Set(['msg_013', 'msg_014']));
    // A focused round draws every arc touching it, unlit.
    const round2 = messageArcs(offRest.messages, router, opts({ focus: 'round:2' }));
    expect(round2.some((a) => a.message.id === 'msg_016' && !a.highlighted)).toBe(true);
    expect(round2.some((a) => a.message.id === 'msg_005')).toBe(false);
    const sel = messageArcs(offRest.messages, router, opts({ selected: 'msg_005' }));
    expect(sel).toEqual([]); // both ends folded onto the summary
    expect(messageArcs(offRest.messages, router, opts({ selected: 'msg_014' })).map((a) => a.message.id)).toEqual([
      'msg_014',
    ]);
  });

  it('filters_and_board_handles', () => {
    const router = makeRouter(model, drawn(model));
    const onlyDirect = messageArcs(model.messages, router, opts({ kinds: new Set(['direct']) }));
    expect(onlyDirect.every((a) => a.message.kind === 'direct')).toBe(true);
    const owl = messageArcs(model.messages, router, opts({ participant: 'owl#1' }));
    expect(new Set(owl.map((a) => a.message.id))).toEqual(new Set(['msg_017', 'msg_018']));
    // A board write lands on its item only when the board shows it.
    const anchors = new Set(boardAnchorKeys(RAIL, FLOW_BOARD, model.board.posts));
    const post = messageArcs(model.messages, router, opts({ anchors })).find((a) => a.message.id === 'msg_016')!;
    expect(post).toMatchObject({ target: RAIL, targetHandle: 'msg_016' });
    const bare = messageArcs(model.messages, router, opts()).find((a) => a.message.id === 'msg_016')!;
    expect(bare.targetHandle).toBeUndefined();
  });

  it('claims_arc_from_their_holder_and_counts_skip_the_rail', () => {
    const claims = claimMessages('flow', FLOW_BOARD, model.board.writers, true);
    expect(claims.map((c) => [c.from, c.id])).toEqual([
      [RUN.lynx1, `claim:route map:${FLOW_BOARD.claims[0].acquired_at}`],
      [RUN.kite3, `claim:reset-token reuse:${FLOW_BOARD.claims[1].acquired_at}`],
      [RUN.owl1, `claim:engagement report:${FLOW_BOARD.claims[2].acquired_at}`],
    ]);
    expect(claimMessages('flow', { kind: 'unavailable', reason: 'x' }, model.board.writers, true)).toEqual([]);
    const counts = messageCounts(model.messages, makeRouter(model, drawn(model)));
    expect(counts.get(RUN.owl1)).toBe(2);
    expect(counts.has(RAIL)).toBe(false);
  });

  it('the_lead_lands_where_the_model_places_work_when_a_round_end_is_unreadable', () => {
    // Round 1's end can't be parsed: it never ends, so anything after round
    // 0 belongs to it, for the model and the router alike.
    const events = DETAIL.events.map((e) => (e.kind === 'round' && e.round === 1 ? { ...e, ts: 'not a time' } : e));
    const late = { ...CHILDREN[4], run_id: 'run_01M5LATE', handle: 'kite#9', address: 'kite#9', round: undefined, started_at: at(150) };
    const lead = { id: 'msg_900', ts: at(150), kind: 'direct' as const, from: 'lead', to: 'kite#9', body: 'late', delivery: { resolved: [] }, seen: [] };
    const input = fixtureInput();
    const m = buildFlowModel({
      ...input,
      detail: { ...DETAIL, events },
      children: [...CHILDREN, late],
      spaces: [{ ...input.spaces[0], messages: [...FLOW_MESSAGES, lead] }, input.spaces[1]],
    });
    const placed = m.rounds.find((r) => r.childIds.includes(late.run_id))!.round;
    expect(placed).toBe(1);
    const router = makeRouter(m, drawn(m));
    expect(router.anchorOf('lead', m.messages.find((x) => x.id === 'msg_900')!)).toEqual({ node: `round:${placed}` });
  });
});
