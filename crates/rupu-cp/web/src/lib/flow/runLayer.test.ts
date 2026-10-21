import { describe, expect, it } from 'vitest';
import { buildRunLayer, RUN_SPACE } from './runLayer';
import { RUN_BOARD, RUN_EVENTS, RUN_MESSAGES, RUN_MODEL, RUN_PARTICIPANTS, RUN_RECORD } from './runFixtures';
import type { RunEvent } from '../api';
import type { RunGraphModel } from '../runGraphModel';
import { at, ms } from './fixtures';

const space = { participants: RUN_PARTICIPANTS, messages: RUN_MESSAGES };
const layer = (events: RunEvent[] = RUN_EVENTS) => buildRunLayer(RUN_MODEL, events, space, RUN_BOARD, { record: RUN_RECORD });

const byId = (id: string, events?: RunEvent[]) => layer(events).messages.find((m) => m.id === id)!;

/** The page's capped event window: everything before `step_started <step>` scrolled out. */
const windowFrom = (step: string) => RUN_EVENTS.slice(RUN_EVENTS.findIndex((e) => e.type === 'step_started' && e.step_id === step));

describe('buildRunLayer', () => {
  it('run_messages_map_to_step_nodes', () => {
    // Units of one step: a loop on the step's card.
    expect(byId('msg_r1')).toMatchObject({ from: 'probe', to: ['probe'], space: RUN_SPACE, kind: 'direct' });
    // A unit to a participant found by its agent run (agent_started).
    expect(byId('msg_r2')).toMatchObject({ from: 'probe', to: ['report'] });
    // A participant with no step id, found by its codename in the recorded
    // step identities.
    expect(byId('msg_r3')).toMatchObject({ from: 'collect', to: ['board'], kind: 'post' });
    expect(byId('msg_r3').at).toBe(ms(3));
    // A sub-agent, through the server's fold of its codename: its step's card.
    expect(byId('msg_r7')).toMatchObject({ from: 'collect', to: ['collect'] });
    // The claim's owner is the card its arc leaves from.
    expect(layer().board.writers.get('collect')).toBe('collect');
    expect(layer().board.posts.map((p) => p.id)).toEqual(['msg_r3']);
  });

  it('identities_survive_a_truncated_event_window', () => {
    // collect's and probe's events scrolled out: the server's folds still
    // place collect and its sub-agent; report's agent run is in the window.
    const events = windowFrom('report');
    expect(byId('msg_r3', events)).toMatchObject({ from: 'collect' });
    expect(byId('msg_r7', events)).toMatchObject({ from: 'collect', to: ['collect'] });
    expect(byId('msg_r2', events)).toMatchObject({ to: ['report'] });
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

  it('steps_band_from_their_recorded_times', () => {
    const { items, bands } = layer().timeline;
    // A completed step: its record's end minus its completion's duration.
    expect(bands).toEqual([
      { key: 'collect', label: 'collect', from: ms(0), to: ms(5), row: 'step' },
      { key: 'probe', label: 'probe', from: ms(5), to: ms(15), row: 'step' },
      { key: 'report', label: 'report', from: ms(15), to: ms(20), row: 'step' },
    ]);
    const item = (key: string) => items.find((i) => i.key === key);
    expect(item('collect')).toEqual({ key: 'collect', at: ms(0), kind: 'step' });
    // A unit's arrival is a lower bound: its step's start, then each earlier
    // unit's end (unit 1 started after unit 0 ended, at 12).
    expect(item('unit:probe:0')).toEqual({ key: 'unit:probe:0', at: ms(5), kind: 'unit' });
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

  it('a_running_step_has_an_inferred_start', () => {
    // report is running: no record, no duration. Its band starts at the last
    // known time before its start event (probe's end) and is marked inferred.
    const nodes = RUN_MODEL.nodes.map((n) => (n.id === 'report' ? { ...n, state: 'running' as const } : n));
    const model: RunGraphModel = { ...RUN_MODEL, nodes, nodeById: (id) => nodes.find((n) => n.id === id) };
    const events = RUN_EVENTS.slice(0, RUN_EVENTS.findIndex((e) => e.type === 'step_completed' && e.step_id === 'report'));
    const record = { ...RUN_RECORD, step_results: RUN_RECORD.step_results.filter((r) => r.step_id !== 'report') };
    const l = buildRunLayer(model, events, space, RUN_BOARD, { record });
    expect(l.timeline.bands.find((b) => b.key === 'report')).toEqual({
      key: 'report', label: 'report', from: ms(15), to: null, row: 'step', inferred: true,
    });
    expect(l.timeline.bands.find((b) => b.key === 'probe')!.inferred).toBeUndefined();
  });

  it('a_truncated_event_window_keeps_exact_bands', () => {
    // collect's events scrolled out of the window: it appears at its
    // recorded end with no band; probe's and report's completions are in the
    // window, so their bands stay exact.
    const l = layer(windowFrom('probe'));
    expect(l.timeline.bands).toEqual([
      { key: 'probe', label: 'probe', from: ms(5), to: ms(15), row: 'step' },
      { key: 'report', label: 'report', from: ms(15), to: ms(20), row: 'step' },
    ]);
    expect(l.timeline.items.find((i) => i.key === 'collect')).toEqual({ key: 'collect', at: ms(5), kind: 'step' });
    // Its units' lower bound starts from probe's exact start, not the run's.
    expect(l.timeline.items.find((i) => i.key === 'unit:probe:0')!.at).toBe(ms(5));
  });

  it('a_step_without_its_start_appears_at_its_end', () => {
    // No events at all: a step appears when its record says it ended, with
    // no band; nothing is placed by guess.
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
