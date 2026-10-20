/**
 * BoardCard — bespoke view for the fleet coordination board (`board.claim`,
 * `board.release`, `board.retract`, `board.read`, and the `board.post` /
 * `board.directive` fallbacks that didn't fold into a MessageBubble).
 *
 * Intent per verb:
 *   • claim / release / retract → a one-line coordination result: the work
 *     unit + the outcome (granted / released / already held …), no JSON.
 *   • read                      → the board log as a readable feed: each post
 *     is a timestamp · author · kind · (→ addressee) · body row, with the
 *     author tinted by the same identity colour the Messages tab uses.
 *   • post / directive          → a note (directive shown as an authority
 *     note); these normally render as a MessageBubble, so this is the fallback
 *     when no `message` event folded in.
 *
 * Parsing (`parseBoard`, `parseBoardLog`) is pure and unit-tested.
 * No `any`. Static Tailwind class strings only.
 */

import { useThemeMode } from '../codename/useThemeMode';
import { identityOf } from '../../lib/messages';
import type { ToolView } from './transcriptView';
import { Chip, VerbPill } from './toolChrome';
import Markdown from './Markdown';

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

export type BoardVerb = 'claim' | 'release' | 'retract' | 'read' | 'post' | 'directive' | 'other';

export interface BoardOutcome {
  label: string;
  tone: 'ok' | 'neutral' | 'warn' | 'err';
}

export interface BoardLogEntry {
  /** Clock time (`HH:MM:SS`) from the entry's ISO timestamp, else the raw head. */
  time: string;
  who: string;
  kind: string;
  to: string | null;
  body: string;
}

export interface BoardView {
  verb: BoardVerb;
  /** claim/release/retract */
  workUnit: string | null;
  outcome: BoardOutcome | null;
  /** read */
  entries: BoardLogEntry[];
  empty: boolean;
  limit: number | null;
  /** post/directive fallback */
  body: string | null;
  postKind: string | null;
  to: string | null;
}

function verbOf(tool: string): BoardVerb {
  const v = tool.startsWith('board.') ? tool.slice('board.'.length) : tool;
  switch (v) {
    case 'claim':
    case 'release':
    case 'retract':
    case 'read':
    case 'post':
    case 'directive':
      return v;
    default:
      return 'other';
  }
}

/** The outcome of a claim / release / retract from its output (or error). */
function outcomeOf(tool: ToolView): BoardOutcome | null {
  if (tool.error) return { label: tool.error, tone: 'err' };
  const out = (tool.output ?? '').trim();
  if (!out) return null;
  if (/^granted\b/i.test(out)) return { label: 'granted', tone: 'ok' };
  if (/^released\b/i.test(out)) return { label: 'released', tone: 'neutral' };
  if (/^retracted\b/i.test(out)) return { label: 'retracted', tone: 'neutral' };
  // Anything else — "already held by …", "denied …" — is a contended outcome.
  return { label: out, tone: 'warn' };
}

const ENTRY_HEAD = /^(\S+)\s+(\S+)\s+\[([^\]]+)\](?:\s+->\s+(\S+?))?:\s*([\s\S]*)$/;

/** `HH:MM:SS` from an ISO timestamp, else the raw head verbatim. */
function clock(ts: string): string {
  const m = ts.match(/T(\d{2}:\d{2}:\d{2})/);
  return m ? m[1] : ts;
}

/** Parse a `board.read` output into entries. Each post starts at a line that
 *  begins with an ISO date; any following non-matching lines continue the
 *  current post's body (a body may contain newlines). */
export function parseBoardLog(output: string): BoardLogEntry[] {
  const entries: BoardLogEntry[] = [];
  for (const line of output.split('\n')) {
    const starts = /^\d{4}-\d{2}-\d{2}T/.test(line);
    if (starts) {
      const m = line.match(ENTRY_HEAD);
      if (m) {
        entries.push({ time: clock(m[1]), who: m[2], kind: m[3], to: m[4] ?? null, body: m[5] });
        continue;
      }
    }
    if (entries.length > 0 && line.length > 0) {
      entries[entries.length - 1].body += `\n${line}`;
    }
  }
  return entries;
}

export function parseBoard(tool: ToolView): BoardView {
  const verb = verbOf(tool.tool);
  const input = isRecord(tool.input) ? tool.input : {};
  const view: BoardView = {
    verb,
    workUnit: null,
    outcome: null,
    entries: [],
    empty: false,
    limit: typeof input.limit === 'number' ? input.limit : null,
    body: typeof input.body === 'string' ? input.body : null,
    postKind: typeof input.kind === 'string' ? input.kind : null,
    to:
      typeof input.addressed_to === 'string'
        ? input.addressed_to
        : typeof input.to === 'string'
          ? input.to
          : null,
  };

  if (verb === 'claim' || verb === 'release' || verb === 'retract') {
    view.workUnit = typeof input.work_unit === 'string' ? input.work_unit : null;
    view.outcome = outcomeOf(tool);
  } else if (verb === 'read') {
    const out = (tool.output ?? '').trim();
    if (!out || /^\(no posts\)$/i.test(out)) view.empty = true;
    else view.entries = parseBoardLog(tool.output ?? '');
  }
  return view;
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

function excerpt(s: string, n = 72): string {
  const one = s.replace(/\s+/g, ' ').trim();
  return one.length > n ? one.slice(0, n - 1) + '…' : one;
}

/** Collapsed-row summary per verb. */
export function BoardSummary({ view }: { view: BoardView }) {
  if (view.verb === 'claim' || view.verb === 'release' || view.verb === 'retract') {
    return (
      <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {view.workUnit && (
          <Chip tone="neutral" mono>
            {view.workUnit}
          </Chip>
        )}
        {view.outcome && (
          <>
            <span className="text-ink-mute text-meta">→</span>
            <Chip tone={view.outcome.tone} title={view.outcome.label}>
              {excerpt(view.outcome.label, 40)}
            </Chip>
          </>
        )}
      </span>
    );
  }
  if (view.verb === 'read') {
    const n = view.entries.length;
    return (
      <span className="flex min-w-0 flex-1 items-center gap-1.5 text-note text-ink-dim">
        board log
        <Chip tone="neutral">{view.empty ? 'empty' : `${n} post${n === 1 ? '' : 's'}`}</Chip>
      </span>
    );
  }
  // post / directive fallback
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1.5">
      {view.verb === 'directive' ? (
        <VerbPill tone="warn">directive</VerbPill>
      ) : (
        view.postKind && <Chip tone="neutral">{view.postKind}</Chip>
      )}
      {view.to && <Chip tone="neutral" mono>→ {view.to}</Chip>}
      {view.body && <span className="truncate font-sans text-note text-ink-dim">{excerpt(view.body)}</span>}
    </span>
  );
}

function LogRow({ entry }: { entry: BoardLogEntry }) {
  const mode = useThemeMode();
  const color = identityOf(entry.who, mode).color;
  return (
    <div className="flex gap-2 border-t border-border py-1 first:border-t-0">
      <span className="shrink-0 font-mono text-meta text-ink-mute tabular-nums">{entry.time}</span>
      <span className="shrink-0 font-mono text-note font-semibold" style={{ color }}>
        {entry.who}
      </span>
      <span className="shrink-0 rounded bg-surface px-1 text-[9px] uppercase tracking-wide text-ink-mute leading-5">
        {entry.kind}
      </span>
      {entry.to && <span className="shrink-0 text-meta text-ink-mute">→ {entry.to}</span>}
      <span className="min-w-0 flex-1 whitespace-pre-wrap text-note text-ink-dim leading-snug">{entry.body}</span>
    </div>
  );
}

/** Expanded body per verb. */
export function BoardBody({ view }: { view: BoardView }) {
  if (view.verb === 'read') {
    if (view.empty) return <p className="px-3 py-2 text-note text-ink-mute">No posts on the board.</p>;
    return (
      <div className="px-3 py-1.5">
        {view.entries.map((e, i) => (
          <LogRow key={i} entry={e} />
        ))}
      </div>
    );
  }
  if (view.verb === 'post' || view.verb === 'directive') {
    return (
      <div className="px-3 py-2">
        {view.body ? (
          <div className="text-ui text-ink-dim">
            <Markdown text={view.body} />
          </div>
        ) : (
          <p className="text-note text-ink-mute">(no body)</p>
        )}
      </div>
    );
  }
  // claim / release / retract — the summary carries it; show the raw outcome quietly.
  if (view.outcome) {
    return (
      <p className="px-3 py-2 font-mono text-meta text-ink-mute break-all">
        {view.workUnit ? `${view.workUnit} · ` : ''}
        {view.outcome.label}
      </p>
    );
  }
  return null;
}

/** A directive gets a left authority stripe in the card shell; exported so
 *  ToolCard can apply it. `border-l-warn` colours ONLY the left edge (plain
 *  `border-warn` would tint all four borders). */
export function boardAccent(view: BoardView): string | null {
  return view.verb === 'directive' ? 'border-l-2 border-l-warn' : null;
}
