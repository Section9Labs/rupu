import { describe, expect, it } from 'vitest';
import type { AgentiflowEvent, RunChild, SpaceMessage, SpaceParticipant } from '../api';
import {
  ARC_REST_LIMIT,
  KEEP_EXPANDED_ROUNDS,
  MAX_CARDS_PER_ROUND,
  buildFlowModel,
  type FlowInput,
  type FlowModel,
} from './flowModel';
import { CHILDREN, DETAIL, FLOW_ID, GANNET_GRAPH, RUN, at, fixtureInput, ms } from './fixtures';

/** Every round of the fixture expanded, so its whole tree is in `nodes`. */
const ALL = new Set(['round:0', 'round:1', 'round:2', 'round:3']);

const node = (m: FlowModel, id: string) => m.nodes.find((n) => n.id === id);
const msg = (m: FlowModel, id: string) => m.messages.find((x) => x.id === id);

describe('flow model', () => {
  it('fixture_rounds_children_and_frames', () => {
    const m = buildFlowModel(fixtureInput(), { expanded: ALL });

    expect(m.rounds.map((r) => [r.round, r.startedAt, r.endedAt])).toEqual([
      [0, ms(1), ms(23)],
      [1, ms(24), ms(84)],
      [2, ms(85), ms(111)],
      // The running round has no event yet.
      [3, null, null],
    ]);
    expect(m.rounds.map((r) => r.childIds)).toEqual([
      [RUN.lynx1, RUN.wren1, RUN.wren2, RUN.lynx2, RUN.lynx3],
      [RUN.gannet1],
      [RUN.kite1, RUN.kite2, RUN.kite3, RUN.wren3],
      [RUN.owl1],
    ]);
    expect(m.rounds.map((r) => r.transcriptPath)).toEqual(
      DETAIL.lead_transcripts.map((t) => t.path),
    );

    // The spine, then each round followed by its tree, depth first.
    expect(m.nodes.map((n) => [n.id, n.kind, n.parentId, n.depth, n.round, n.inferred])).toEqual([
      ['lead', 'lead', '', 0, -1, false],
      ['round:0', 'round', 'lead', 0, 0, false],
      [RUN.lynx1, 'child', 'round:0', 1, 0, false],
      [RUN.wren1, 'child', RUN.lynx1, 2, 0, false],
      [RUN.wren2, 'child', RUN.lynx1, 2, 0, false],
      [RUN.lynx2, 'child', 'round:0', 1, 0, false],
      [RUN.lynx3, 'child', 'round:0', 1, 0, false],
      ['round:1', 'round', 'lead', 0, 1, false],
      [RUN.gannet1, 'frame', 'round:1', 1, 1, false],
      ['round:2', 'round', 'lead', 0, 2, false],
      [RUN.kite1, 'child', 'round:2', 1, 2, false],
      [RUN.kite2, 'child', 'round:2', 1, 2, false],
      [RUN.kite3, 'child', 'round:2', 1, 2, false],
      [RUN.wren3, 'child', RUN.kite3, 2, 2, false],
      ['round:3', 'round', 'lead', 0, 3, false],
      [RUN.owl1, 'child', 'round:3', 1, 3, false],
    ]);
    const frame = node(m, RUN.gannet1)!;
    expect(frame.child?.generated).toBe(true);
    expect(frame.graph).toBe(GANNET_GRAPH);
    expect(node(m, RUN.lynx1)?.child?.name).toBe('surface-mapper');

    // Round 0 and 1 are finished and not among the latest two: by default
    // they collapse, and their children leave the canvas.
    const dflt = buildFlowModel(fixtureInput());
    expect(dflt.rounds.map((r) => r.collapsed)).toEqual([true, true, false, false]);
    expect(dflt.nodes.map((n) => n.id)).toEqual([
      'lead', 'round:0', 'round:1', 'round:2', RUN.kite1, RUN.kite2, RUN.kite3, RUN.wren3, 'round:3', RUN.owl1,
    ]);
    // A collapsed round still names its children (for its summary card).
    expect(dflt.rounds[0].childIds).toHaveLength(5);
    expect(dflt.rounds.map((r) => r.hidden)).toEqual([0, 0, 0, 0]);
  });

  it('round_decision_and_goals_after_come_from_the_next_event', () => {
    const m = buildFlowModel(fixtureInput());
    expect(m.rounds.map((r) => r.decision)).toEqual([
      'Map the surface before testing anything.',
      'Generated auth-sweep to probe every auth endpoint, with a review panel.',
      'Verify the two highest findings; retry the verifier that hit a 529.',
      null,
    ]);
    expect(m.rounds.map((r) => r.spentUsd)).toEqual([8.4, 29.75, 39.05, null]);
    expect(m.rounds.map((r) => r.goalsAfter)).toEqual([
      // Round 1's event: the assessment it started from.
      [{ id: 'verified', met: false, current: 0 }, { id: 'report', met: false, current: 0 }],
      // Round 2's event.
      [{ id: 'verified', met: false, current: 0 }, { id: 'report', met: false, current: 0 }],
      // The last finished round: the record's latest evaluation.
      [{ id: 'verified', met: true, current: 2 }, { id: 'report', met: false, current: 0 }],
      // Still running.
      null,
    ]);

    // An old run: rounds carry no decision, goals or start; the last round
    // still gets the record's goals, the others nothing.
    const old = fixtureInput();
    old.detail = {
      ...DETAIL,
      events: DETAIL.events.map((e) => {
        if (e.kind !== 'round') return e;
        const { decision: _d, goals: _g, started_at: _s, ...rest } = e as Record<string, unknown>;
        return rest as AgentiflowEvent;
      }),
    };
    const om = buildFlowModel(old);
    expect(om.rounds.map((r) => [r.decision, r.startedAt, r.goalsAfter === null])).toEqual([
      [null, null, true],
      [null, null, true],
      [null, null, false],
      [null, null, true],
    ]);
    // Its bands still span the rounds: from the previous end (round 0 from
    // the run's start).
    expect(om.timeline.bands.map((b) => [b.key, b.from, b.to])).toEqual([
      ['round:0', ms(0), ms(23)],
      ['round:1', ms(23), ms(84)],
      ['round:2', ms(84), ms(111)],
      ['round:3', ms(111), null],
    ]);
  });

  it('legacy_children_are_placed_by_time_and_marked_inferred', () => {
    const strip = (c: RunChild): RunChild => {
      const { round: _r, ...rest } = c;
      return rest;
    };
    const withRounds = buildFlowModel(fixtureInput(), { expanded: ALL });
    for (const legacyEvents of [false, true]) {
      const input = fixtureInput();
      input.children = CHILDREN.map(strip);
      if (legacyEvents) {
        // Rounds without `started_at` either: the lower bound is the
        // previous round's end.
        input.detail = {
          ...DETAIL,
          events: DETAIL.events.map((e) => {
            const { started_at: _s, ...rest } = e as Record<string, unknown>;
            return rest as AgentiflowEvent;
          }),
        };
      }
      const m = buildFlowModel(input, { expanded: ALL });
      expect(m.rounds.map((r) => r.childIds)).toEqual(withRounds.rounds.map((r) => r.childIds));
      expect(m.nodes.map((n) => [n.id, n.parentId, n.round])).toEqual(
        withRounds.nodes.map((n) => [n.id, n.parentId, n.round]),
      );
      const kids = m.nodes.filter((n) => n.child);
      expect(kids).toHaveLength(CHILDREN.length);
      expect(kids.every((n) => n.inferred)).toBe(true);
    }

    // A child whose parent isn't listed hangs off its round, inferred.
    const orphan = fixtureInput();
    orphan.children = [
      ...CHILDREN,
      {
        ...CHILDREN.find((c) => c.run_id === RUN.wren3)!,
        run_id: 'sub_01M5ORPHAN',
        parent_run_id: 'run_01M5GONE',
        depth: 3,
      },
    ];
    const om = buildFlowModel(orphan, { expanded: ALL });
    expect(node(om, 'sub_01M5ORPHAN')).toMatchObject({ parentId: 'round:2', depth: 1, round: 2, inferred: true });

    // A stopped flow: work started after its last round event belongs to a
    // round that ended without one; a late sub-agent stays with its parent.
    const stopped = fixtureInput();
    stopped.detail = { ...DETAIL, record: { ...DETAIL.record, status: 'failed', ended_at: at(120) } };
    stopped.children = CHILDREN.filter((c) => c.run_id !== RUN.owl1).map((c) =>
      c.run_id === RUN.wren3 ? { ...strip(c), started_at: at(115) } : c,
    );
    const sm = buildFlowModel(stopped, { expanded: ALL });
    expect(sm.rounds.map((r) => r.round)).toEqual([0, 1, 2]);
    expect(node(sm, RUN.wren3)).toMatchObject({ parentId: RUN.kite3, round: 2, inferred: false });
    stopped.children = CHILDREN.map(strip);
    const tm = buildFlowModel(stopped);
    expect(tm.rounds.map((r) => [r.round, r.startedAt, r.endedAt, r.collapsed])).toEqual([
      [0, ms(1), ms(23), true],
      [1, ms(24), ms(84), true],
      [2, ms(85), ms(111), false],
      [3, null, ms(120), false],
    ]);
    expect(node(tm, RUN.owl1)).toMatchObject({ parentId: 'round:3', round: 3, inferred: true });
  });

  it('unmapped_addresses_still_produce_an_edge', () => {
    const input = fixtureInput();
    const ghost: SpaceMessage[] = [
      {
        id: 'msg_901', ts: at(100), kind: 'directive', from: 'lead', to: 'role:ghost', body: 'nobody here',
        delivery: { resolved: [], undelivered: [{ to: 'role:ghost', reason: 'no_live_participant' }] }, seen: [],
      },
      {
        id: 'msg_902', ts: at(101), kind: 'direct', from: 'left#4', to: 'lead', body: 'a participant that left',
        delivery: { resolved: ['lead'] }, seen: [],
      },
      {
        id: 'msg_903', ts: at(102), kind: 'broadcast', from: 'kite#1', to: 'broadcast', body: 'legacy, no delivery',
        delivery: { resolved: [] }, seen: [],
      },
    ];
    input.spaces = [{ ...input.spaces[0], messages: [...input.spaces[0].messages, ...ghost] }, input.spaces[1]];
    const m = buildFlowModel(input);
    expect(msg(m, 'msg_901')).toMatchObject({
      kind: 'directive',
      from: 'lead',
      to: ['board', 'offcanvas'],
      undelivered: [{ to: 'offcanvas', address: 'role:ghost', reason: 'no_live_participant' }],
    });
    expect(msg(m, 'msg_902')).toMatchObject({ from: 'offcanvas', to: ['lead'] });
    expect(msg(m, 'msg_903')).toMatchObject({ kind: 'broadcast', from: RUN.kite1, to: ['offcanvas'] });

    // The fixture's endpoints: participants to run ids, the board, the
    // operator, a resolved fan-out, an undelivered one on its card.
    expect(msg(m, 'msg_007')).toMatchObject({ kind: 'operator', from: 'operator', to: ['lead'], space: 'flow' });
    expect(msg(m, 'msg_002')).toMatchObject({ kind: 'post', from: RUN.lynx1, to: ['board'] });
    expect(msg(m, 'msg_013')?.to).toEqual(['board', RUN.kite1, RUN.kite2, RUN.kite3]);
    expect(msg(m, 'msg_017')).toMatchObject({
      to: [RUN.owl1],
      undelivered: [{ to: RUN.owl1, address: 'owl#1', reason: 'unknown_participant' }],
    });
    // A workflow child's space resolves to its frame's inner step nodes.
    expect(msg(m, 'msg_009')).toMatchObject({
      space: RUN.gannet1,
      from: `${RUN.gannet1}/probe`,
      to: [`${RUN.gannet1}/probe`],
    });
    expect(msg(m, 'msg_012')?.to).toEqual([`${RUN.gannet1}/review`]);
    expect(msg(m, 'msg_008')).toMatchObject({ space: RUN.gannet1, to: ['board'] });
    // Nothing is dropped, and the list is in time order.
    expect(m.messages).toHaveLength(13 + 5 + 3);
    expect(m.messages.map((x) => x.at)).toEqual([...m.messages.map((x) => x.at)].sort((a, b) => a - b));

    // The boards: the flow's rail and the frame's strip, each with its posts
    // and the cards that wrote its items.
    expect(m.board.view).toMatchObject({ journal: true });
    expect(m.board.posts.map((p) => p.id)).toEqual(['msg_002', 'msg_003', 'msg_016']);
    expect([...m.board.writers]).toEqual([
      ['lynx#1', RUN.lynx1],
      ['kite#3', RUN.kite3],
      ['owl#1', RUN.owl1],
      ['lead', 'lead'],
    ]);
    const strip = m.board.frames.get(RUN.gannet1)!;
    expect(strip.posts.map((p) => p.id)).toEqual(['msg_008']);
    expect([...strip.writers]).toEqual([['probe#6', `${RUN.gannet1}/probe`]]);

    // A step the frame's graph lacks resolves to the frame itself.
    const noStep = fixtureInput();
    noStep.childGraphs = new Map([
      [RUN.gannet1, { ...GANNET_GRAPH, nodeById: (id: string) => (id === 'review' ? undefined : GANNET_GRAPH.nodeById(id)) }],
    ]);
    expect(msg(buildFlowModel(noStep), 'msg_012')?.to).toEqual([RUN.gannet1]);
  });

  it('density_rules_apply_at_scale', () => {
    expect([KEEP_EXPANDED_ROUNDS, MAX_CARDS_PER_ROUND, ARC_REST_LIMIT]).toEqual([2, 8, 40]);
    const input = bigFlow();
    const m = buildFlowModel(input);

    expect(m.rounds).toHaveLength(200);
    // The latest two rounds, plus round 120, which still has a running child.
    expect(m.rounds.filter((r) => !r.collapsed).map((r) => r.round)).toEqual([120, 198, 199]);

    const busy = m.rounds[120];
    expect(busy.childIds).toHaveLength(23);
    // 23 children, 8 cards, plus the failed (#9) and running (#15) ones past
    // the limit: 13 fold into one "+13 more".
    expect(busy.hidden).toBe(13);
    const more = m.nodes.filter((n) => n.kind === 'more');
    expect(more).toEqual([
      { id: 'more:120', kind: 'more', parentId: 'round:120', depth: 1, round: 120, inferred: false },
    ]);
    const shown = m.nodes.filter((n) => n.round === 120 && n.kind === 'child').map((n) => n.id);
    expect(shown).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 9, 15].map((i) => `run_u120_${i}`));

    // Expanding the round shows every card and no "+N more".
    const open = buildFlowModel(input, { expanded: new Set(['round:120']) });
    expect(open.nodes.filter((n) => n.round === 120 && n.kind === 'child')).toHaveLength(23);
    expect(open.nodes.some((n) => n.kind === 'more')).toBe(false);
    expect(open.rounds[120].hidden).toBe(0);
    // Expanding a collapsed round uncollapses it.
    const five = buildFlowModel(input, { expanded: new Set(['round:5']) });
    expect(five.rounds[5].collapsed).toBe(false);
    expect(five.nodes.filter((n) => n.round === 5).map((n) => n.id)).toEqual(['round:5', 'run_r5']);
    // Collapsed rounds keep only their node on the canvas.
    expect(m.nodes.filter((n) => n.round === 5).map((n) => n.id)).toEqual(['round:5']);

    // 500 messages: only operator arcs are drawn at rest.
    expect(m.messages).toHaveLength(500);
    expect(m.messages.filter((x) => x.atRest).map((x) => x.kind)).toEqual(Array(10).fill('operator'));
    // At the limit every arc is at rest.
    const few = bigFlow(ARC_REST_LIMIT);
    expect(buildFlowModel(few).messages.every((x) => x.atRest)).toBe(true);
    expect(buildFlowModel(bigFlow(ARC_REST_LIMIT + 1)).messages.filter((x) => x.atRest)).toHaveLength(1);
  });

  it('timeline_items_cover_rounds_children_messages_board', () => {
    const m = buildFlowModel(fixtureInput());
    const of = (kind: string) => m.timeline.items.filter((i) => i.kind === kind).map((i) => [i.key, i.at, i.sub]);

    expect(of('round')).toEqual([
      ['round:0', ms(1), undefined],
      ['round:1', ms(24), undefined],
      ['round:2', ms(85), undefined],
      // Running: from the previous round's end.
      ['round:3', ms(111), undefined],
    ]);
    expect(of('child')).toEqual([
      [RUN.lynx1, ms(2), undefined],
      [RUN.wren1, ms(5), undefined],
      [RUN.wren2, ms(6), undefined],
      [RUN.lynx2, ms(2), undefined],
      [RUN.lynx3, ms(3), undefined],
      [RUN.gannet1, ms(26), undefined],
      [RUN.kite1, ms(86), undefined],
      [RUN.kite2, ms(86), undefined],
      [RUN.kite3, ms(93), undefined],
      [RUN.wren3, ms(97), undefined],
      [RUN.owl1, ms(114), undefined],
    ]);
    const msgs = of('message');
    expect(msgs).toHaveLength(18);
    expect(msgs.slice(0, 3)).toEqual([
      ['msg_001', ms(2), 'directive'],
      ['msg_002', ms(9), 'post'],
      ['msg_003', ms(12), 'post'],
    ]);
    expect(msgs.find((x) => x[0] === 'msg_007')).toEqual(['msg_007', ms(25), 'operator']);
    expect(of('board')).toEqual([
      ['claim:flow:route map:' + at(9), ms(9), undefined],
      ['claim:flow:reset-token reuse:' + at(95), ms(95), undefined],
      ['claim:flow:engagement report:' + at(114), ms(114), undefined],
      ['directive:flow:msg_001', ms(2), undefined],
      ['directive:flow:msg_013', ms(88), undefined],
      [`claim:${RUN.gannet1}:/api/tokens:` + at(34), ms(34), undefined],
    ]);
    expect(m.timeline.items).toHaveLength(4 + 11 + 18 + 6);

    expect(m.timeline.bands).toEqual([
      { key: 'round:0', label: 'Round 0', from: ms(1), to: ms(23), row: 'round' },
      { key: 'round:1', label: 'Round 1', from: ms(24), to: ms(84), row: 'round' },
      { key: 'round:2', label: 'Round 2', from: ms(85), to: ms(111), row: 'round' },
      { key: 'round:3', label: 'Round 3', from: ms(111), to: null, row: 'round' },
    ]);

    // A board the host couldn't serve adds no items and is passed through.
    const input = fixtureInput();
    input.boards = new Map([['flow', { kind: 'unavailable', reason: 'no board here' }]]);
    const u = buildFlowModel(input);
    expect(u.board.view).toEqual({ kind: 'unavailable', reason: 'no board here' });
    expect(u.board.frames.get(RUN.gannet1)?.view).toBe(null);
    expect(u.timeline.items.filter((i) => i.kind === 'board')).toEqual([]);
  });
});

/** A 200-round agentiflow, running in round 199: one child per round, except
 *  round 120 with 23 (child 9 failed, child 15 still running), and
 *  `messageCount` messages in the flow's space, every 50th from the
 *  operator. */
function bigFlow(messageCount = 500): FlowInput {
  const MIN = 60_000;
  const t0 = Date.UTC(2026, 9, 1);
  const iso = (t: number) => new Date(t).toISOString();
  const events: AgentiflowEvent[] = [{ kind: 'run_started', ts: iso(t0) }];
  for (let r = 0; r < 199; r++) {
    events.push({
      kind: 'round', round: r, started_at: iso(t0 + r * 10 * MIN), ts: iso(t0 + (r * 10 + 9) * MIN),
      goals_met: 0, goals_total: 1, spent_usd: r, spent_tokens: r * 1000, outcome: 'yielded',
    });
  }
  const children: RunChild[] = [];
  const participants: SpaceParticipant[] = [
    { address: 'lead', agent: 'lead', run_id: FLOW_ID, kind: 'flow_lead', live: true },
  ];
  const add = (run_id: string, round: number, start: number, status: RunChild['status']) => {
    const address = `${run_id}#1`;
    children.push({
      handle: address, kind: 'agent', name: 'worker', run_id, parent_run_id: FLOW_ID, depth: 1, codename: null,
      address, status, success: null, error: null, tokens: null, duration_ms: null, started_at: iso(start),
      finished_at: null, transcript_path: null, legacy: false, round,
    });
    participants.push({ address, agent: 'worker', run_id, kind: 'flow_unit', live: status === 'running' });
  };
  for (let r = 0; r < 200; r++) {
    if (r === 120) {
      for (let i = 0; i < 23; i++) {
        add(`run_u120_${i}`, r, t0 + (r * 10 + 1) * MIN + i * 1000, i === 9 ? 'failed' : i === 15 ? 'running' : 'done');
      }
    } else {
      add(`run_r${r}`, r, t0 + (r * 10 + 1) * MIN, r === 199 ? 'running' : 'done');
    }
  }
  const messages: SpaceMessage[] = [];
  for (let i = 0; i < messageCount; i++) {
    const op = i % 50 === 0;
    const to = `run_r${i % 199}#1`;
    messages.push({
      id: `msg_${String(i).padStart(4, '0')}`, ts: iso(t0 + i * 1000), kind: op ? 'operator' : 'direct',
      from: op ? 'operator' : 'lead', to, body: `m${i}`, delivery: { resolved: [to] }, seen: [],
    });
  }
  return {
    detail: {
      ...DETAIL,
      record: { ...DETAIL.record, started_at: iso(t0) },
      events,
      lead_transcripts: [],
    },
    children,
    spaces: [{ id: 'flow', participants, messages }],
    boards: new Map(),
    childGraphs: new Map(),
  };
}
