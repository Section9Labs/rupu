// messageQuery — the Messages tab's filter, on the SAME grammar as the
// findings query (`findingQuery/grammar`), evaluated client-side against a
// space's log as `assetQuery` does for assets.
//
// Addresses (`from`, `to`, `involving`) match exactly, or by role word when
// the value has no `#`: `from:pipit` is every `pipit#n`, `from:pipit#2` only
// that one (never `pipit#20`). Everything compares ignoring case.

import { AtSign, Bot, CircleAlert, Hash, ListTree, Send, Shapes, Users } from 'lucide-react';
import { parseQuery, type Term } from './findingQuery/grammar';
import type { QueryField } from './findingQuery/fields';
import type { MessageKind, SpaceMessage, SpaceParticipant } from './api';
import { roleWordOf } from './messages';

export const MESSAGE_KINDS: MessageKind[] = ['post', 'direct', 'broadcast', 'operator', 'directive', 'retract'];
export const MESSAGE_FLAGS = ['urgent', 'undelivered', 'seen', 'unseen', 'reply'] as const;
type Flag = (typeof MESSAGE_FLAGS)[number];

type Facets = Record<string, { value: string; count: number }[]>;

/** The query fields. Nothing in them depends on the loaded log: `kind` and
 *  `is` are closed sets, the rest are free text the facets suggest. */
export const MESSAGE_FIELDS: QueryField[] = [
  { key: 'kind', aliases: ['type'], kind: 'enum', values: [...MESSAGE_KINDS], label: 'Kind', description: 'kind:direct · kind:directive,retract', icon: Shapes },
  { key: 'from', aliases: [], kind: 'text', values: [], label: 'From', description: 'from:pipit (every pipit#n) · from:pipit#2', icon: Send },
  { key: 'to', aliases: [], kind: 'text', values: [], label: 'To', description: 'to:heron · to:role:recon', icon: AtSign },
  { key: 'involving', aliases: ['who'], kind: 'text', values: [], label: 'Involving', description: 'involving:lynx#1 (sent or reached)', icon: Users },
  { key: 'agent', aliases: ['role'], kind: 'text', values: [], label: 'Agent', description: "agent:recon (the sender's agent)", icon: Bot },
  { key: 'step', aliases: [], kind: 'text', values: [], label: 'Step', description: "step:analyze (the sender's step)", icon: ListTree },
  { key: 'is', aliases: [], kind: 'enum', values: [...MESSAGE_FLAGS], label: 'Is', description: 'is:urgent · is:undelivered · -is:seen', icon: CircleAlert },
  { key: 'id', aliases: [], kind: 'text', values: [], label: 'Id', description: 'id:msg_…', icon: Hash },
];

function flags(m: SpaceMessage): Flag[] {
  const out: Flag[] = [];
  if (m.urgent) out.push('urgent');
  if ((m.delivery.undelivered ?? []).length > 0) out.push('undelivered');
  out.push(m.seen.length > 0 ? 'seen' : 'unseen');
  if (m.refs?.reply_to) out.push('reply');
  return out;
}

function sender(m: SpaceMessage, participants: SpaceParticipant[]): SpaceParticipant | undefined {
  return participants.find((p) => p.address === m.from);
}

const recipients = (m: SpaceMessage): string[] => [...(m.to ? [m.to] : []), ...m.delivery.resolved];

function tally(vals: (string | undefined | null)[]): { value: string; count: number }[] {
  const m = new Map<string, number>();
  for (const v of vals) if (typeof v === 'string' && v !== '') m.set(v, (m.get(v) ?? 0) + 1);
  return [...m].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
}

/** Values in use per field, with counts: feeds the QueryBar dropdown. */
export function messageFacets(messages: SpaceMessage[], participants: SpaceParticipant[]): Facets {
  const senders = messages.map((m) => sender(m, participants));
  return {
    kind: tally(messages.map((m) => m.kind)),
    from: tally(messages.map((m) => m.from)),
    to: tally(messages.flatMap((m) => m.delivery.resolved)),
    involving: participants
      .map((p) => ({ value: p.address, count: messages.filter((m) => matchesAddress(p.address, [m.from, ...recipients(m)])).length }))
      .sort((a, b) => b.count - a.count),
    agent: tally(senders.map((p) => p?.agent)),
    step: tally(senders.map((p) => p?.step_id)),
    is: tally(messages.flatMap(flags)),
  };
}

/** Whether `value` names one of `addresses`. */
function matchesAddress(value: string, addresses: (string | undefined)[]): boolean {
  const v = value.toLowerCase();
  const byRole = !v.includes('#');
  return addresses.some((a) => a !== undefined && (a.toLowerCase() === v || (byRole && roleWordOf(a) === v)));
}

const eqCi = (value: string, x: string | undefined) => x !== undefined && x.toLowerCase() === value.toLowerCase();

function valueMatches(m: SpaceMessage, key: string, value: string, participants: SpaceParticipant[]): boolean {
  switch (key) {
    case 'kind':
      return m.kind === value;
    case 'from':
      return matchesAddress(value, [m.from, m.from_codename]);
    case 'to':
      return matchesAddress(value, recipients(m));
    case 'involving':
      return matchesAddress(value, [m.from, ...recipients(m)]);
    case 'agent':
      return eqCi(value, sender(m, participants)?.agent);
    case 'step':
      return eqCi(value, sender(m, participants)?.step_id);
    case 'is':
      return flags(m).includes(value as Flag);
    case 'id':
      return eqCi(value, m.id);
    default: {
      const v = value.toLowerCase();
      return [m.body, m.from, m.to ?? ''].some((x) => x.toLowerCase().includes(v));
    }
  }
}

function termMatches(m: SpaceMessage, t: Term, participants: SpaceParticipant[]): boolean {
  const hit = t.values.some((v) => valueMatches(m, t.key, v, participants));
  return t.neg ? !hit : hit;
}

/** Filter a log by a query string. On a parse error nothing is filtered (the
 *  QueryBar shows the offending chip in red) and the message is returned. */
export function filterMessages(messages: SpaceMessage[], q: string, participants: SpaceParticipant[]): { rows: SpaceMessage[]; error: string | null } {
  if (!q.trim()) return { rows: messages, error: null };
  const p = parseQuery(q, MESSAGE_FIELDS);
  if (!p.ok) return { rows: messages, error: p.error.message };
  return { rows: messages.filter((m) => p.terms.every((t) => termMatches(m, t, participants))), error: null };
}
