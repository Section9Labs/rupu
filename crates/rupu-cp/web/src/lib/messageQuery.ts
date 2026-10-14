// messageQuery — the Messages tab's filter: a `RowField` registry over a
// space's log, evaluated client-side by `lib/query/rows`.
//
// Addresses (`from`, `to`, `involving`) match exactly, or by role word when
// the value has no `#`: `from:pipit` is every `pipit#n`, `from:pipit#2` only
// that one (never `pipit#20`). Everything compares ignoring case.

import { AtSign, Bot, CircleAlert, Hash, ListTree, Send, Shapes, Users } from 'lucide-react';
import { filterRows, rowFacets, rowField, type Facets, type FilterResult, type RowField } from './query/rows';
import type { MessageKind, SpaceMessage, SpaceParticipant } from './api';
import { roleWordOf } from './messages';

export const MESSAGE_KINDS: MessageKind[] = ['post', 'direct', 'broadcast', 'operator', 'directive', 'retract'];
export const MESSAGE_FLAGS = ['urgent', 'undelivered', 'seen', 'unseen', 'reply'] as const;
type Flag = (typeof MESSAGE_FLAGS)[number];

function flags(m: SpaceMessage): Flag[] {
  const out: Flag[] = [];
  if (m.urgent) out.push('urgent');
  if ((m.delivery.undelivered ?? []).length > 0) out.push('undelivered');
  out.push(m.seen.length > 0 ? 'seen' : 'unseen');
  if (m.refs?.reply_to) out.push('reply');
  return out;
}

const recipients = (m: SpaceMessage): string[] => [...(m.to ? [m.to] : []), ...m.delivery.resolved];

/** The query fields over a log whose senders are `participants` (the sender's
 *  agent and step come from its participant row). `kind` and `is` are closed
 *  sets; the rest are free text the facets suggest. */
export function messageFields(participants: SpaceParticipant[]): RowField<SpaceMessage>[] {
  const sender = (m: SpaceMessage) => participants.find((p) => p.address === m.from);
  return [
    rowField({ key: 'kind', aliases: ['type'], values: [...MESSAGE_KINDS], match: 'exact', get: (m) => m.kind, facet: true, label: 'Kind', description: 'kind:direct · kind:directive,retract', icon: Shapes }),
    rowField({ key: 'from', match: 'address', get: (m) => [m.from, m.from_codename], facet: (m) => m.from, label: 'From', description: 'from:pipit (every pipit#n) · from:pipit#2', icon: Send }),
    rowField({ key: 'to', match: 'address', get: recipients, facet: (m) => m.delivery.resolved, label: 'To', description: 'to:heron · to:role:recon', icon: AtSign }),
    rowField({ key: 'involving', aliases: ['who'], match: 'address', get: (m) => [m.from, ...recipients(m)], label: 'Involving', description: 'involving:lynx#1 (sent or reached)', icon: Users }),
    rowField({ key: 'agent', aliases: ['role'], match: 'exact', get: (m) => sender(m)?.agent, facet: true, label: 'Agent', description: "agent:recon (the sender's agent)", icon: Bot }),
    rowField({ key: 'step', match: 'exact', get: (m) => sender(m)?.step_id, facet: true, label: 'Step', description: "step:analyze (the sender's step)", icon: ListTree }),
    rowField({ key: 'is', values: [...MESSAGE_FLAGS], match: 'exact', get: flags, facet: true, label: 'Is', description: 'is:urgent · is:undelivered · -is:seen', icon: CircleAlert }),
    rowField({ key: 'id', match: 'exact', get: (m) => m.id, label: 'Id', description: 'id:msg_…', icon: Hash }),
  ];
}

/** The fields as the grammar sees them (chips, suggestions, shortcuts). */
export const MESSAGE_FIELDS = messageFields([]);

/** Whether `value` names one of `addresses`. */
function matchesAddress(value: string, addresses: (string | undefined)[]): boolean {
  const v = value.toLowerCase();
  const byRole = !v.includes('#');
  return addresses.some((a) => a !== undefined && (a.toLowerCase() === v || (byRole && roleWordOf(a) === v)));
}

/** Values in use per field, with counts: feeds the QueryBar dropdown. Every
 *  participant is an `involving` value, counted over what it sent or reached. */
export function messageFacets(messages: SpaceMessage[], participants: SpaceParticipant[]): Facets {
  return {
    ...rowFacets(messages, messageFields(participants)),
    involving: participants
      .map((p) => ({ value: p.address, count: messages.filter((m) => matchesAddress(p.address, [m.from, ...recipients(m)])).length }))
      .sort((a, b) => b.count - a.count),
  };
}

/** Filter a log by a query string. On a parse error nothing is filtered (the
 *  QueryBar shows the offending chip in red) and the message is returned. Bare
 *  words search the body, the sender and the addressee. */
export function filterMessages(messages: SpaceMessage[], q: string, participants: SpaceParticipant[]): FilterResult<SpaceMessage> {
  return filterRows(messages, q, messageFields(participants), { text: (m) => [m.body, m.from, m.to] });
}
