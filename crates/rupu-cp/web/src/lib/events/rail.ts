// The event rail's pure rules, shared by Live Events and the run and
// agentiflow Events tabs (components/events/*). A StreamCard
// (lib/situationRoom/cards.ts) becomes either a quiet one-line row (agent
// work, lifecycle, panel rounds, completions, warnings) or a raised card (a
// finding, a gate that waits on you, a failure); the cards that need the
// operator are pinned in the Needs-you deck above the stream until they are
// answered or seen. Nothing here touches the DOM.

import type { StreamCard } from '../situationRoom/cards';
import { parseCodename } from '../codename';

/** The face the rail marker (and a card's capsule) shows. */
export type Mark =
  | 'running'
  | 'done'
  | 'failed'
  | 'awaiting'
  | 'activity'
  | 'lifecycle'
  | 'panel'
  | 'warning'
  | 'critical'
  | 'high'
  | 'medium'
  | 'low'
  | 'info';

/** Findings, gates and failures are raised cards; everything else is a line. */
export function isRaised(card: StreamCard): boolean {
  return card.form === 'finding' || card.form === 'await' || card.form === 'error';
}

export function markOf(card: StreamCard): Mark {
  switch (card.form) {
    case 'finding':
      return card.severity ?? 'info';
    case 'await':
      return 'awaiting';
    case 'error':
      return 'failed';
    case 'warning':
      return 'warning';
    case 'panel':
      return 'panel';
    case 'complete':
      return card.accent === 'error' ? 'failed' : 'done';
    case 'lifecycle':
      return 'lifecycle';
    default:
      return 'activity';
  }
}

/** How a quiet row reads: a short kind word, a lowercase headline, and an
 *  optional right-aligned mono meta (duration, tokens). */
export interface RowText {
  kw: string;
  headline: string;
  meta?: string;
  /** Dim words after the headline (a skip reason, a panel's remaining
   *  severity, a step kind). */
  note?: string;
}

export function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 100) / 10}k`;
  return `${Math.round(n / 100_000) / 10}M`;
}

export function fmtDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${Math.max(0, Math.round(ms / 100) / 10)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`;
}

const lower = (s: string) => (s ? s[0].toLowerCase() + s.slice(1) : s);

export function rowText(card: StreamCard): RowText {
  switch (card.form) {
    case 'activity':
      switch (card.badge) {
        case 'Agent':
          return { kw: 'agent', headline: 'agent started' };
        case 'Scanning':
        case 'Step':
          return { kw: 'step', headline: `${card.title} started`, note: card.detail };
        case 'Working':
          return { kw: 'working', headline: card.detail ?? card.title };
        case 'Fan-out':
          return { kw: 'unit', headline: 'unit started' };
        case 'Skipped':
          return { kw: 'skipped', headline: card.title, note: card.detail };
        default:
          return { kw: card.badge.split(' ')[0].toLowerCase(), headline: card.title, note: card.detail };
      }
    case 'lifecycle':
      if (card.badge === 'Run started') {
        return { kw: 'run', headline: 'run started', note: card.detail ? baseName(card.detail) : undefined };
      }
      return { kw: card.badge.toLowerCase(), headline: lower(card.title) };
    case 'complete': {
      const failed = card.accent === 'error';
      const meta =
        card.durationMs != null
          ? fmtDuration(card.durationMs)
          : card.tokensIn != null && card.tokensOut != null
            ? `${fmtTokens(card.tokensIn)} → ${fmtTokens(card.tokensOut)} tok`
            : undefined;
      if (card.badge.startsWith('Run')) return { kw: failed ? 'failed' : 'done', headline: lower(card.title), meta };
      if (card.badge.startsWith('Unit')) {
        return { kw: failed ? 'failed' : 'done', headline: failed ? 'unit failed' : 'unit finished', meta };
      }
      return { kw: failed ? 'failed' : 'done', headline: `${card.title} ${failed ? 'failed' : 'finished'}`, meta };
    }
    case 'panel':
      return {
        kw: 'panel',
        headline: card.round ? `round ${card.round.n}/${card.round.max}` : card.title,
        note: card.detail,
      };
    case 'warning':
      return { kw: 'warning', headline: card.detail ?? card.title };
    default:
      return { kw: card.badge.toLowerCase(), headline: card.title, note: card.detail };
  }
}

function baseName(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** One piece of a row's context line. */
export interface CtxPart {
  text: string;
  /** Mono (codenames, units). */
  mono?: boolean;
  /** A codename: `crew` dim, `leaf` strong; rendered by the row. */
  codename?: { crew?: string; leaf: string; derived?: boolean };
  href?: string;
}

/** Where a row sits: on Live Events every row names its project and
 *  workflow and links its run; on a run's tab those are the page itself. */
export type RailScope = 'live' | 'run';

export interface Resolved {
  project?: string;
  workflow?: string;
}

export function ctxOf(card: StreamCard, scope: RailScope, resolved: Resolved = {}): CtxPart[] {
  const out: CtxPart[] = [];
  if (scope === 'live') {
    const project = resolved.project ?? card.projectName;
    const workflow = resolved.workflow ?? card.workflow;
    if (project) out.push({ text: project });
    if (workflow) out.push({ text: workflow });
  }
  const showStep = card.stepId && !(card.form === 'activity' && (card.badge === 'Scanning' || card.badge === 'Step'))
    && !(card.form === 'complete' && !card.badge.startsWith('Unit') && !card.badge.startsWith('Run'))
    && !(card.form === 'error' && card.title.startsWith(card.stepId));
  if (card.stepId && showStep) {
    const kind = card.stepKind && card.stepKind !== 'linear' ? `${card.stepKind} ` : '';
    out.push({ text: `${kind}${card.stepId}${card.unitKey ? ` [${card.unitKey}]` : ''}` });
  } else if (card.unitKey) {
    out.push({ text: `[${card.unitKey}]`, mono: true });
  }
  if (card.round && card.form !== 'panel') out.push({ text: `round ${card.round.n}/${card.round.max}` });
  if (card.agent) out.push({ text: card.agent });
  const runHref = scope === 'live' && card.runId ? `/runs/${card.runId}` : undefined;
  if (card.codename) {
    const p = parseCodename(card.codename);
    out.push({
      text: card.codename,
      codename: { crew: scope === 'live' ? p.crew : undefined, leaf: p.leaf, derived: card.codenameDerived },
      href: runHref,
    });
  } else if (scope === 'live' && card.runId) {
    out.push({ text: card.crew ?? card.runId.slice(0, 8), mono: true, href: runHref });
  }
  if (card.model && card.form === 'activity' && card.badge === 'Agent') {
    out.push({ text: [card.provider, card.model].filter(Boolean).join('/'), mono: true });
  }
  return out;
}

// ── gates: still waiting, or answered ──

export type GateOutcome = 'open' | 'approved' | 'rejected';

/**
 * What became of each gate card in a newest-first list: a gate is answered
 * once its run says anything newer about the same step or about the whole
 * run (a rejected run ends `rejected`; anything else moved on past it). A
 * run the caller knows has ended closes its gates too.
 */
export function gateOutcomes(
  cards: readonly StreamCard[],
  terminal?: (runId: string) => string | undefined,
): Map<string, GateOutcome> {
  const out = new Map<string, GateOutcome>();
  // The list is newest first, so by the time a gate comes up, `later` holds
  // everything its run said after it.
  const later = new Map<string, StreamCard[]>();
  for (const c of cards) {
    if (!c.runId) continue;
    if (c.form === 'await') {
      const newer = later.get(c.runId) ?? [];
      const moved = newer.filter((n) => !n.stepId || n.stepId === c.stepId);
      let outcome: GateOutcome = 'open';
      if (moved.length > 0) {
        outcome = moved.some((n) => n.form === 'complete' && /rejected/i.test(n.title)) ? 'rejected' : 'approved';
      } else {
        const status = terminal?.(c.runId);
        if (status) outcome = status === 'rejected' ? 'rejected' : 'approved';
      }
      out.set(c.key, outcome);
    }
    const list = later.get(c.runId);
    if (list) list.push(c);
    else later.set(c.runId, [c]);
  }
  return out;
}

// ── the Needs-you deck ──

/** How long an unseen failure or severe finding stays pinned. */
export const NEEDS_YOU_WINDOW_MS = 24 * 60 * 60 * 1000;

export function deckRank(card: StreamCard): number {
  if (card.form === 'await') return 0;
  if (card.form === 'error') return 1;
  return card.severity === 'critical' ? 2 : 3;
}

export interface DeckInputs {
  now: number;
  isSeen: (key: string) => boolean;
  gates: ReadonlyMap<string, GateOutcome>;
  /** Gates this viewer already answered (pending-state, until the run moves). */
  answered?: (card: StreamCard) => boolean;
  windowMs?: number;
}

/** A gate still waiting, an unseen failure, an unseen critical or high finding. */
export function needsYou(card: StreamCard, o: DeckInputs): boolean {
  if (card.form === 'await') return o.gates.get(card.key) === 'open' && !o.answered?.(card);
  const recent = !card.ts || o.now - card.ts <= (o.windowMs ?? NEEDS_YOU_WINDOW_MS);
  if (card.form === 'error') return recent && !o.isSeen(card.key);
  if (card.form === 'finding') {
    return recent && (card.severity === 'critical' || card.severity === 'high') && !o.isSeen(card.key);
  }
  return false;
}

/** The deck, most urgent first: gates, failures, critical, high; newest first within each. */
export function deckOf(cards: readonly StreamCard[], o: DeckInputs): StreamCard[] {
  const order = new Map(cards.map((c, i) => [c.key, i]));
  return cards
    .filter((c) => needsYou(c, o))
    .sort((a, b) => deckRank(a) - deckRank(b) || (order.get(a.key) ?? 0) - (order.get(b.key) ?? 0));
}

/** `2 gates · 1 failure · 3 findings`. */
export function deckBrief(items: readonly StreamCard[]): string {
  const n = { await: 0, error: 0, finding: 0 };
  for (const c of items) if (c.form === 'await' || c.form === 'error' || c.form === 'finding') n[c.form]++;
  const parts: string[] = [];
  if (n.await) parts.push(`${n.await} gate${n.await === 1 ? '' : 's'}`);
  if (n.error) parts.push(`${n.error} failure${n.error === 1 ? '' : 's'}`);
  if (n.finding) parts.push(`${n.finding} finding${n.finding === 1 ? '' : 's'}`);
  return parts.join(' · ');
}

// ── time: five-minute groups ──

export const BUCKET_MS = 5 * 60 * 1000;

export interface RailGroup {
  /** Bucket start (ms), or null for rows that carry no time. */
  bucket: number | null;
  /** The newest time in the group (for its relative age). */
  newest: number;
  cards: StreamCard[];
}

/** Consecutive cards of one five-minute bucket, newest first. Cards without a
 *  time (a run's own events carry none) join the group around them. */
export function groupByBucket(cards: readonly StreamCard[]): RailGroup[] {
  const out: RailGroup[] = [];
  for (const c of cards) {
    const b = c.ts ? Math.floor(c.ts / BUCKET_MS) * BUCKET_MS : null;
    const last = out[out.length - 1];
    if (last && (b === null || last.bucket === b || last.bucket === null)) {
      if (last.bucket === null && b !== null) last.bucket = b;
      if (c.ts > last.newest) last.newest = c.ts;
      last.cards.push(c);
    } else {
      out.push({ bucket: b, newest: c.ts, cards: [c] });
    }
  }
  return out;
}

/** "just now", "4 min ago", "2 h ago", "3 d ago". */
export function relAge(ms: number): string {
  if (ms < 45_000) return 'just now';
  if (ms < 3_600_000) return `${Math.max(1, Math.round(ms / 60_000))} min ago`;
  if (ms < 86_400_000) return `${Math.round(ms / 3_600_000)} h ago`;
  return `${Math.round(ms / 86_400_000)} d ago`;
}

/** The key a thread follows: rows of one run are linked on the rail. */
export function threadKey(card: StreamCard): string | undefined {
  return card.runId ?? card.crew;
}
