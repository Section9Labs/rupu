import { describe, expect, it } from 'vitest';
import { buildRunLayer, RUN_SPACE } from './runLayer';
import { RUN_BOARD, RUN_EVENTS, RUN_MESSAGES, RUN_MODEL, RUN_PARTICIPANTS, RUN_RECORD } from './runFixtures';
import { at, ms } from './fixtures';

const layer = () =>
  buildRunLayer(RUN_MODEL, RUN_EVENTS, { participants: RUN_PARTICIPANTS, messages: RUN_MESSAGES }, RUN_BOARD, {
    record: RUN_RECORD,
  });

const byId = (id: string) => layer().messages.find((m) => m.id === id)!;

describe('buildRunLayer', () => {
  it('run_messages_map_to_step_nodes', () => {
    // Units of one step: a loop on the step's card.
    expect(byId('msg_r1')).toMatchObject({ from: 'probe', to: ['probe'], space: RUN_SPACE, kind: 'direct' });
    // A unit to another step's agent.
    expect(byId('msg_r2')).toMatchObject({ from: 'probe', to: ['report'] });
    // A participant with no step id, found by its agent run (agent_started).
    expect(byId('msg_r3')).toMatchObject({ from: 'collect', to: ['board'], kind: 'post' });
    expect(byId('msg_r3').at).toBe(ms(3));
    // The claim's owner is the card its arc leaves from.
    expect(layer().board.writers.get('collect')).toBe('collect');
    expect(layer().board.posts.map((p) => p.id)).toEqual(['msg_r3']);
  });

  it('operator_broadcast_enters_offcanvas', () => {
    // The operator enters from its own off-canvas entry, reaching each step.
    expect(byId('msg_r4')).toMatchObject({ from: 'operator', to: ['collect', 'probe', 'report'], kind: 'operator' });
    // An address no step answers to goes off canvas, as its dead end.
    const dead = byId('msg_r5');
    expect(dead.to).toEqual(['offcanvas']);
    expect(dead.undelivered).toEqual([{ to: 'offcanvas', address: 'ghost#9', reason: 'unknown_participant' }]);
    // A workflow run has no lead.
    expect(byId('msg_r6')).toMatchObject({ from: 'offcanvas', to: ['report'] });
  });

  it('steps_band_from_the_event_order', () => {
    const { items, bands } = layer().timeline;
    // A step starts when the last known time before its start event: the
    // run's start, then each finished step's or unit's recorded end.
    expect(bands).toEqual([
      { key: 'collect', label: 'collect', from: ms(0), to: ms(5), row: 'step' },
      { key: 'probe', label: 'probe', from: ms(5), to: ms(15), row: 'step' },
      { key: 'report', label: 'report', from: ms(15), to: ms(20), row: 'step' },
    ]);
    const item = (key: string) => items.find((i) => i.key === key);
    expect(item('collect')).toEqual({ key: 'collect', at: ms(0), kind: 'step' });
    expect(item('unit:probe:0')).toEqual({ key: 'unit:probe:0', at: ms(5), kind: 'unit' });
    // Unit 1 started after unit 0 ended (at 12).
    expect(item('unit:probe:1')).toEqual({ key: 'unit:probe:1', at: ms(12), kind: 'unit' });
    expect(item('msg_r4')).toEqual({ key: 'msg_r4', at: ms(10), kind: 'message', sub: 'operator' });
    expect(item(`claim:${RUN_SPACE}:package-lock.json:${at(2)}`)).toEqual({
      key: `claim:${RUN_SPACE}:package-lock.json:${at(2)}`,
      at: ms(2),
      kind: 'board',
    });
    // Nothing without a time is guessed into the timeline.
    expect(layer().untimed).toEqual([]);
  });

  it('a_step_without_its_start_appears_at_its_end', () => {
    // No events (a window that dropped them): a step appears when its
    // record says it ended, with no band; nothing is placed by guess.
    const l = buildRunLayer(RUN_MODEL, [], { participants: [], messages: [] }, null, { record: RUN_RECORD });
    expect(l.timeline.bands).toEqual([]);
    expect(l.timeline.items).toEqual([
      { key: 'collect', at: ms(5), kind: 'step' },
      { key: 'probe', at: ms(15), kind: 'step' },
      { key: 'report', at: ms(20), kind: 'step' },
    ]);
    // With no record either, a step that ran is always shown; a pending one
    // only once the replay is over.
    const bare = buildRunLayer(
      { ...RUN_MODEL, nodes: RUN_MODEL.nodes.map((n) => (n.id === 'report' ? { ...n, state: 'pending' as const } : n)) },
      [],
      { participants: [], messages: [] },
      null,
    );
    expect(bare.untimed).toEqual(['collect', 'probe']);
  });
});
