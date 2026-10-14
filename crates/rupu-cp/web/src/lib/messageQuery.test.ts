import { describe, expect, it } from 'vitest';
import type { SpaceMessage, SpaceParticipant } from './api';
import { filterMessages, messageFacets } from './messageQuery';
import { MESSAGES, PARTICIPANTS } from '../components/messages/fixtures';

const ids = (q: string, messages = MESSAGES, participants = PARTICIPANTS) => {
  const r = filterMessages(messages, q, participants);
  expect(r.error).toBeNull();
  return r.rows.map((m) => m.id.replace('msg_01JQ8MA', ''));
};

const m = (id: string, from: string, extra: Partial<SpaceMessage> = {}): SpaceMessage => ({
  id,
  ts: '2026-10-13T14:00:00Z',
  kind: 'post',
  from,
  to: null,
  body: id,
  delivery: { resolved: [] },
  seen: [],
  ...extra,
});

const PIPITS = [
  m('p1', 'pipit#1'),
  m('p2', 'pipit#2'),
  m('p20', 'pipit#20'),
  m('h', 'heron', { kind: 'direct', to: 'pipit#2', delivery: { resolved: ['pipit#2'] } }),
];
const pids = (q: string) => filterMessages(PIPITS, q, []).rows.map((x) => x.id);

describe('filterMessages', () => {
  it('kind (and its alias type)', () => {
    expect(ids('kind:direct')).toEqual(['003', '006']);
    expect(ids('type:directive,retract')).toEqual(['002', '008']);
  });

  it('from matches the sender or its codename', () => {
    expect(ids('from:lynx#1')).toEqual(['004', '005', '006']);
    expect(ids('from:cobalt-harbor/heron')).toEqual(['001']);
  });

  it('to matches the addressee or a resolved recipient', () => {
    expect(ids('to:role:crawler')).toEqual(['006']);
    expect(ids('to:heron')).toEqual(['004', '005', '007', '009']);
  });

  it('involving (alias who) is sender or recipient', () => {
    expect(ids('involving:lynx#1')).toEqual(['001', '002', '003', '004', '005', '006', '007', '008', '009']);
    expect(ids('who:otter')).toEqual(['001', '002', '004', '005', '007', '008', '009']);
  });

  it("agent (alias role) and step are the sender's", () => {
    expect(ids('agent:service-analyst')).toEqual(['004', '005', '006']);
    expect(ids('role:RECON')).toEqual(['001', '002', '003', '008']);
    expect(ids('step:verify')).toEqual(['009']);
  });

  it('is: urgent, undelivered, seen, unseen, reply', () => {
    expect(ids('is:urgent')).toEqual(['007']);
    expect(ids('is:undelivered')).toEqual(['006']);
    expect(ids('is:unseen')).toEqual(['002', '006', '008']);
    expect(ids('is:seen')).toEqual(['001', '003', '004', '005', '007', '009']);
    const reply = m('r', 'heron', { refs: { reply_to: 'msg_01JQ8MA009' } });
    expect(filterMessages([...MESSAGES, reply], 'is:reply', PARTICIPANTS).rows.map((x) => x.id)).toEqual(['r']);
  });

  it('id is exact', () => {
    expect(ids('id:msg_01JQ8MA004')).toEqual(['004']);
    expect(ids('id:msg_01JQ8MA00')).toEqual([]);
  });

  it('bare text searches body, sender and addressee', () => {
    expect(ids('FTP')).toEqual(['001', '002', '009']);
    expect(ids('crawler')).toEqual(['006']);
  });

  it('negates a term, ORs a term’s values and ANDs terms', () => {
    expect(ids('-kind:post')).toEqual(['002', '003', '005', '006', '007', '008']);
    expect(ids('kind:direct,broadcast')).toEqual(['003', '005', '006']);
    expect(ids('from:heron kind:direct')).toEqual(['003']);
    expect(ids('-is:seen -kind:directive,retract')).toEqual(['006']);
  });

  it('a value without # names a role word; with # only that address', () => {
    expect(pids('from:pipit')).toEqual(['p1', 'p2', 'p20']);
    expect(pids('from:pipit#2')).toEqual(['p2']);
    expect(pids('from:Pipit#20')).toEqual(['p20']);
    expect(pids('involving:pipit#2')).toEqual(['p2', 'h']);
    expect(pids('to:pipit')).toEqual(['h']);
  });

  it('a parse error filters nothing and says why', () => {
    const r = filterMessages(MESSAGES, 'colour:red', PARTICIPANTS);
    expect(r.rows).toHaveLength(MESSAGES.length);
    expect(r.error).toBe('unknown key `colour` (quote the text to search for it)');
    expect(filterMessages(MESSAGES, 'is:loud', PARTICIPANTS).error).toBe('`loud` is not a value of `is:` (one of urgent, undelivered, seen, unseen, reply)');
  });
});

describe('messageFacets', () => {
  it('counts the values in use', () => {
    const extra: SpaceParticipant = { address: 'kite', agent: 'recon', run_id: 'r', kind: 'workflow_step', live: true };
    const f = messageFacets(MESSAGES, [...PARTICIPANTS, extra]);
    expect(f.kind).toEqual([
      { value: 'post', count: 3 },
      { value: 'direct', count: 2 },
      { value: 'directive', count: 1 },
      { value: 'broadcast', count: 1 },
      { value: 'operator', count: 1 },
      { value: 'retract', count: 1 },
    ]);
    expect(f.from).toEqual([
      { value: 'heron', count: 4 },
      { value: 'lynx#1', count: 3 },
      { value: 'operator', count: 1 },
      { value: 'otter', count: 1 },
    ]);
    expect(f.involving).toEqual([
      { value: 'lynx#1', count: 9 },
      { value: 'heron', count: 8 },
      { value: 'otter', count: 7 },
      { value: 'kite', count: 0 },
    ]);
    expect(f.agent).toEqual([
      { value: 'recon', count: 4 },
      { value: 'service-analyst', count: 3 },
      { value: 'exploit-verifier', count: 1 },
    ]);
    expect(f.step).toEqual([
      { value: 'recon', count: 4 },
      { value: 'analyze', count: 3 },
      { value: 'verify', count: 1 },
    ]);
    expect(f.is).toEqual([
      { value: 'seen', count: 6 },
      { value: 'unseen', count: 3 },
      { value: 'undelivered', count: 1 },
      { value: 'urgent', count: 1 },
    ]);
  });
});
