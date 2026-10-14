// Pure helpers over a message space's log (spec F1c): how each log kind is
// shown, what an undelivered reason means to the operator, receipts, the
// directive fold, the composer's recipient groups and the feed's kind pills.
// No React; the components in `components/messages/` render what this says.

import { roleBadge } from './codename';
import type { BadgeShape } from './codenamePalette.gen';
import type { MessageKind, MessageRoute, MessageSeen, MessageStreamFrame, SpaceMessage, SpaceParticipant } from './api';

export type Mode = 'light' | 'dark';

/** How a log line is shown in the feed. A post with `to` is an @mention in
 *  the channel; directives and retractions are compact markers there (the
 *  directives panel shows them in full). */
export type RowKind = 'post' | 'mention' | 'direct' | 'broadcast' | 'operator' | 'directive' | 'retract';

export function rowKind(m: SpaceMessage): RowKind {
  switch (m.kind) {
    case 'post':
      return m.to ? 'mention' : 'post';
    default:
      return m.kind;
  }
}

export const OPERATOR = 'operator';
export const LEAD = 'lead';

/** The role word an address names: the last hop, before its `#n`
 *  (`heron>otter#1` → `otter`, `role:recon` → `recon`). */
export function roleWordOf(address: string): string {
  const bare = address.replace(/^(role|step):/, '');
  const last = bare.split('>').pop() ?? bare;
  return last.split(/[#.]/)[0].trim().toLowerCase();
}

export interface Identity {
  color: string;
  shape: BadgeShape | null;
}

const FALLBACK: Record<Mode, string[]> = {
  light: ['#7c3aed', '#2563eb', '#d97706', '#db2777', '#0d9488', '#ea580c'],
  dark: ['#a78bfa', '#60a5fa', '#fbbf24', '#f472b6', '#2dd4bf', '#fb923c'],
};

const OPERATOR_TINT: Record<Mode, string> = { light: '#334155', dark: '#cbd5e1' };
const LEAD_TINT: Record<Mode, string> = { light: '#6d28d9', dark: '#c4b5fd' };

/** A participant's colour and badge shape: its codename role's badge from
 *  the generated palette (one crew shares one tint, so the role word is what
 *  tells participants apart), a hashed fallback for a word the palette lacks. */
export function identityOf(address: string, mode: Mode): Identity {
  if (address === OPERATOR) return { color: OPERATOR_TINT[mode], shape: null };
  if (address === LEAD) return { color: LEAD_TINT[mode], shape: 'star' };
  const word = roleWordOf(address);
  const badge = roleBadge(word, mode);
  if (badge) return { color: badge.color, shape: badge.shape };
  let h = 0;
  for (let i = 0; i < word.length; i++) h = (h * 31 + word.charCodeAt(i)) >>> 0;
  return { color: FALLBACK[mode][h % FALLBACK[mode].length], shape: null };
}

export function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(full, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

/** An undelivered recipient, in the operator's words. */
export function undeliveredText(u: { to: string; reason: string }): string {
  switch (u.reason) {
    case 'unknown_participant':
      return `no participant ${u.to}`;
    case 'no_live_participant':
      return `no live participant for ${u.to}`;
    case 'no_parent':
      return 'no parent to send to';
    case 'no_lead':
      return 'no lead to send to';
    case 'participant_left':
      return `${u.to} has left`;
    case 'inbox_full':
      return `${u.to}'s inbox is full`;
    case 'broadcast_full':
      return 'the broadcast log is full';
    default:
      return `${u.to}: ${u.reason.replace(/_/g, ' ')}`;
  }
}

/** The quiet receipts line: each reader with its turn, or a count past two. */
export function receiptsText(seen: MessageSeen[]): { text: string; title: string } | null {
  if (seen.length === 0) return null;
  const title = seen.map((s) => `${s.by} · turn ${s.turn}`).join('\n');
  if (seen.length > 2) return { text: `seen by ${seen.length}`, title };
  return { text: `seen by ${seen.map((s) => `${s.by} · turn ${s.turn}`).join(', ')}`, title };
}

export interface Directive {
  message: SpaceMessage;
  retractedBy: SpaceMessage | null;
}

/** Every directive, each with the retraction that lifted it; standing ones
 *  first, then by time. */
export function foldDirectives(messages: SpaceMessage[]): Directive[] {
  const retractions = new Map<string, SpaceMessage>();
  for (const m of messages) if (m.kind === 'retract' && m.refs?.retracts) retractions.set(m.refs.retracts, m);
  return messages
    .filter((m) => m.kind === 'directive')
    .map((message) => ({ message, retractedBy: retractions.get(message.id) ?? null }))
    .sort((a, b) => Number(!!a.retractedBy) - Number(!!b.retractedBy) || a.message.ts.localeCompare(b.message.ts));
}

// --- recipients ------------------------------------------------------------

export interface RecipientOption {
  /** The address sent as `to`. */
  value: string;
  label: string;
  hint?: string;
  /** A participant that has left: still addressable, shown greyed. */
  dim?: boolean;
  /** For a participant: its address, for the avatar. */
  avatar?: string;
}

export interface RecipientGroup {
  label: 'Everyone' | 'Participants' | 'Roles' | 'Steps';
  options: RecipientOption[];
}

/** The composer's grouped recipients: everyone (and the lead for a flow),
 *  participants live first, roles and (workflows only) steps with live
 *  counts. The operator is never a recipient. */
export function recipientGroups(route: MessageRoute, participants: SpaceParticipant[], workflow: boolean): RecipientGroup[] {
  const people = participants.filter((p) => p.address !== OPERATOR);
  const everyone: RecipientOption[] = [{ value: 'broadcast', label: 'everyone', hint: 'broadcast' }];
  if (route === 'agentiflows') everyone.unshift({ value: LEAD, label: LEAD, hint: 'the lead', avatar: LEAD });
  const groups: RecipientGroup[] = [{ label: 'Everyone', options: everyone }];

  const sorted = [...people].sort((a, b) => Number(b.live) - Number(a.live) || a.address.localeCompare(b.address));
  const direct = sorted.filter((p) => !(route === 'agentiflows' && p.address === LEAD));
  if (direct.length > 0) {
    groups.push({
      label: 'Participants',
      options: direct.map((p) => ({ value: p.address, label: p.address, hint: p.live ? p.agent : `${p.agent} · left`, dim: !p.live, avatar: p.address })),
    });
  }

  const count = (key: (p: SpaceParticipant) => string | undefined) => {
    const m = new Map<string, number>();
    for (const p of people) {
      const k = key(p);
      if (k) m.set(k, (m.get(k) ?? 0) + (p.live ? 1 : 0));
    }
    return [...m.entries()].sort(([a], [b]) => a.localeCompare(b));
  };
  const roles = count((p) => p.agent);
  if (roles.length > 0) {
    groups.push({ label: 'Roles', options: roles.map(([r, n]) => ({ value: `role:${r}`, label: `role:${r}`, hint: `${n} live`, dim: n === 0 })) });
  }
  if (workflow) {
    const steps = count((p) => p.step_id);
    if (steps.length > 0) {
      groups.push({ label: 'Steps', options: steps.map(([s, n]) => ({ value: `step:${s}`, label: `step:${s}`, hint: `${n} live`, dim: n === 0 })) });
    }
  }
  return groups;
}

/** An operator send between its POST and its line reaching the log. */
export interface PendingSend {
  key: string;
  to: string;
  body: string;
  urgent: boolean;
  /** The log id the 202 named; the entry clears when that line arrives. */
  id?: string;
}

/** F1b's default: an agentiflow's lead, everyone elsewhere. */
export function defaultRecipient(route: MessageRoute): string {
  return route === 'agentiflows' ? LEAD : 'broadcast';
}

// --- filters ---------------------------------------------------------------

/** The feed's kind pills; a directive and its retraction share one. */
export type KindFilter = 'post' | 'direct' | 'broadcast' | 'operator' | 'directive';

export const KIND_FILTERS: { key: KindFilter; label: string; kinds: MessageKind[] }[] = [
  { key: 'post', label: 'Channel', kinds: ['post'] },
  { key: 'direct', label: 'Direct', kinds: ['direct'] },
  { key: 'broadcast', label: 'Broadcast', kinds: ['broadcast'] },
  { key: 'operator', label: 'Operator', kinds: ['operator'] },
  { key: 'directive', label: 'Directives', kinds: ['directive', 'retract'] },
];

// --- the log, as the client holds it --------------------------------------

/** A space's messages in log order, each with its receipts. */
export interface SpaceLog {
  messages: SpaceMessage[];
  index: ReadonlyMap<string, number>;
}

export const EMPTY_LOG: SpaceLog = { messages: [], index: new Map() };

const seenKey = (s: MessageSeen) => `${s.by}#${s.turn}`;

function mergeSeen(into: MessageSeen[], more: MessageSeen[]): MessageSeen[] {
  if (more.length === 0) return into;
  const have = new Set(into.map(seenKey));
  const add = more.filter((s) => !have.has(seenKey(s)));
  return add.length === 0 ? into : [...into, ...add];
}

/** `log` with `msgs` merged in: a new id is appended, a known one gains any
 *  receipts it lacked (a GET page and the stream can overlap). */
export function withMessages(log: SpaceLog, msgs: SpaceMessage[]): SpaceLog {
  if (msgs.length === 0) return log;
  const messages = [...log.messages];
  const index = new Map(log.index);
  for (const m of msgs) {
    const at = index.get(m.id);
    if (at === undefined) {
      index.set(m.id, messages.length);
      messages.push({ ...m, seen: m.seen ?? [] });
    } else {
      const seen = mergeSeen(messages[at].seen, m.seen ?? []);
      if (seen !== messages[at].seen) messages[at] = { ...messages[at], seen };
    }
  }
  return { messages, index };
}

/** `log` with a batch of stream frames applied, in order. A receipt for a
 *  message not yet in the log is dropped: the log writes a message before
 *  any receipt for it. */
export function withFrames(log: SpaceLog, frames: MessageStreamFrame[]): SpaceLog {
  let out = log;
  const pending: SpaceMessage[] = [];
  const flush = () => {
    out = withMessages(out, pending.splice(0));
  };
  for (const f of frames) {
    if (f.t === 'msg') {
      const { t: _t, ...m } = f;
      pending.push({ ...m, seen: m.seen ?? [] });
      continue;
    }
    flush();
    const at = out.index.get(f.id);
    if (at === undefined) continue;
    const seen = mergeSeen(out.messages[at].seen, [{ by: f.by, turn: f.turn, at: f.at }]);
    if (seen === out.messages[at].seen) continue;
    const messages = [...out.messages];
    messages[at] = { ...messages[at], seen };
    out = { messages, index: out.index };
  }
  flush();
  return out;
}

// --- anchors ---------------------------------------------------------------

const MSG_HASH = '#msg-';

/** The message id a `#msg-<id>` hash names, else null. */
export function parseMsgHash(hash: string): string | null {
  if (!hash.startsWith(MSG_HASH)) return null;
  try {
    const id = decodeURIComponent(hash.slice(MSG_HASH.length));
    return id === '' ? null : id;
  } catch {
    return null;
  }
}

export function msgHash(id: string): string {
  return `${MSG_HASH}${encodeURIComponent(id)}`;
}
