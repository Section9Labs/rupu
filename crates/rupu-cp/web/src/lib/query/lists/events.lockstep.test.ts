// The Live Events query against the server's: the registry spec and the
// card group of every event type, as `crates/rupu-cp/tests/fixtures/
// list_query/` records them. Fix the page, never the fixture.
import { describe, expect, it } from 'vitest';
import type { RunEvent } from '../../api';
import { cardFromEvent } from '../../situationRoom/cards';
import { EVENT_FIELDS } from './events';
import fields from '../../../../../tests/fixtures/list_query/events.fields.json';
import groups from '../../../../../tests/fixtures/list_query/events.groups.json';

describe('the Live Events query matches the server', () => {
  it('has the same fields', () => {
    const spec = EVENT_FIELDS.map(({ key, aliases, kind, values, match, text, facet }) => ({
      aliases,
      facet: !!facet,
      key,
      kind,
      match,
      text: !!text,
      values,
    }));
    expect(spec).toEqual(fields);
  });

  it.each(Object.entries(groups))('a %s card is in group %s', (type, group) => {
    const ev = {
      type,
      run_id: 'run_1',
      step_id: 'scan',
      note: 'working',
      kind: 'linear',
      reason: 'r',
      error: 'e',
      message: 'm',
      unit_key: 'u',
      index: 0,
      agent: 'a',
      provider: 'p',
      model: 'm',
      success: true,
      duration_ms: 1,
      status: 'completed',
      round: 1,
      max_iterations: 2,
    } as unknown as RunEvent;
    expect(cardFromEvent(ev, 0, 'k')?.group).toBe(group);
  });
});
