import { describe, expect, it } from 'vitest';
import type { SpaceMessage } from './api';
import {
  defaultRecipient,
  EMPTY_LOG,
  foldDirectives,
  parseMsgHash,
  msgHash,
  receiptsText,
  recipientGroups,
  undeliveredText,
  withFrames,
  withMessages,
} from './messages';
import { MESSAGES, PARTICIPANTS } from '../components/messages/fixtures';

const m = (id: string, extra: Partial<SpaceMessage> = {}): SpaceMessage => ({
  id,
  ts: '2026-10-13T14:00:00Z',
  kind: 'post',
  from: 'heron',
  to: null,
  body: id,
  delivery: { resolved: [] },
  seen: [],
  ...extra,
});

describe('the client log', () => {
  it('appends new ids and merges receipts into known ones, never duplicating', () => {
    const a = withMessages(EMPTY_LOG, [m('msg_1'), m('msg_2')]);
    const b = withMessages(a, [m('msg_2', { seen: [{ by: 'otter', turn: 3, at: 't' }] }), m('msg_3')]);
    expect(b.messages.map((x) => x.id)).toEqual(['msg_1', 'msg_2', 'msg_3']);
    expect(b.messages[1].seen).toEqual([{ by: 'otter', turn: 3, at: 't' }]);
    const c = withMessages(b, [m('msg_2', { seen: [{ by: 'otter', turn: 3, at: 't' }] })]);
    expect(c.messages[1].seen).toHaveLength(1);
  });

  it('applies stream frames in order: a message, then its receipt', () => {
    const log = withFrames(EMPTY_LOG, [
      { t: 'msg', ...m('msg_1') },
      { t: 'seen', id: 'msg_1', by: 'lynx#1', turn: 4, at: 't1' },
      { t: 'seen', id: 'msg_404', by: 'lynx#1', turn: 4, at: 't1' },
      { t: 'msg', ...m('msg_2') },
    ]);
    expect(log.messages.map((x) => [x.id, x.seen.map((s) => s.by)])).toEqual([
      ['msg_1', ['lynx#1']],
      ['msg_2', []],
    ]);
  });
});

describe('display text', () => {
  it('says why a recipient got nothing', () => {
    expect(undeliveredText({ to: 'role:crawler', reason: 'no_live_participant' })).toBe('no live participant for role:crawler');
    expect(undeliveredText({ to: 'otter', reason: 'participant_left' })).toBe('otter has left');
    expect(undeliveredText({ to: 'x', reason: 'something_new' })).toBe('x: something new');
  });

  it('lists up to two readers, then counts', () => {
    expect(receiptsText([])).toBeNull();
    expect(receiptsText([{ by: 'heron', turn: 5, at: '' }])?.text).toBe('seen by heron · turn 5');
    expect(receiptsText(MESSAGES[6].seen)?.text).toBe('seen by 3');
  });

  it('folds each directive with the retraction that lifted it', () => {
    const [d] = foldDirectives(MESSAGES);
    expect(d.message.id).toBe('msg_01JQ8MA002');
    expect(d.retractedBy?.id).toBe('msg_01JQ8MA008');
  });

  it('round-trips a #msg- anchor', () => {
    expect(parseMsgHash(msgHash('msg_01J'))).toBe('msg_01J');
    expect(parseMsgHash('#call-x')).toBeNull();
  });
});

describe('recipients', () => {
  it('defaults to the lead for a flow and everyone elsewhere', () => {
    expect(defaultRecipient('agentiflows')).toBe('lead');
    expect(defaultRecipient('runs')).toBe('broadcast');
    expect(defaultRecipient('sessions')).toBe('broadcast');
  });

  it('offers steps only for a workflow, and the lead only for a flow', () => {
    const run = recipientGroups('runs', PARTICIPANTS, true);
    expect(run.map((g) => g.label)).toEqual(['Everyone', 'Participants', 'Roles', 'Steps']);
    expect(run[0].options.map((o) => o.value)).toEqual(['broadcast']);
    const session = recipientGroups('sessions', PARTICIPANTS, false);
    expect(session.map((g) => g.label)).toEqual(['Everyone', 'Participants', 'Roles']);
    const flow = recipientGroups('agentiflows', PARTICIPANTS, false);
    expect(flow[0].options.map((o) => o.value)).toEqual(['lead', 'broadcast']);
  });
});
