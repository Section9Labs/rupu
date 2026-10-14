// The Network explorer's query: its fields, how a query becomes the
// netflow reads' params (`from` / `to` and the dimension lists, which the
// server applies), the window the explorer opens on when the query names
// none, and the graph clicks that write terms into it.
import { CalendarClock, Globe, History, Network, Route, Server, Workflow } from 'lucide-react';
import type { QueryField } from '../../findingQuery/fields';
import { parseQuery, quoteValue, tokenize, type Term } from '../../findingQuery/grammar';
import { spanWindow, type LatestRun, type NetflowFilters, type NetflowRange } from '../../netflow';
import { parseTimeValue, type TimeValue } from '../time';

export const NETFLOW_FIELDS: QueryField[] = [
  { key: 'since', aliases: ['from'], kind: 'time', values: [], label: 'Since', description: 'since:30m · since:2026-10-01 (a UTC day) · an RFC 3339 time', icon: History },
  { key: 'until', aliases: ['to'], kind: 'time', values: [], label: 'Until', description: 'until:10m (older than) · until:2026-10-01', icon: CalendarClock },
  { key: 'window', aliases: [], kind: 'enum', values: ['all'], label: 'Window', description: 'window:all (every recorded flow, not the default window)', icon: Route },
  { key: 'workflow', aliases: [], kind: 'text', values: [], label: 'Workflow', description: 'workflow:nightly · workflow:unknown (no workflow)', icon: Workflow },
  { key: 'origin', aliases: [], kind: 'text', values: [], label: 'Origin', description: 'origin:provider:anthropic · origin:subprocess:curl', icon: Network },
  { key: 'org', aliases: [], kind: 'text', values: [], label: 'Network', description: 'org:as15169 · -org:unknown', icon: Globe },
  { key: 'host', aliases: [], kind: 'text', values: [], label: 'Endpoint', description: 'host:api.github.com:443 (host:port)', icon: Server },
];

/** The dimension keys and the filter lists they fill. */
export const NETFLOW_DIMS = {
  workflow: ['workflows', 'not_workflows'],
  origin: ['origins', 'not_origins'],
  org: ['orgs', 'not_orgs'],
  host: ['hosts', 'not_hosts'],
} as const satisfies Record<string, readonly [keyof NetflowFilters, keyof NetflowFilters]>;

export type NetflowDim = keyof typeof NETFLOW_DIMS;

/** What a query asks the netflow reads for. `window: 'default'` means it
 *  names no window: the caller supplies the default. */
export type NetflowRequest =
  | { ok: true; window: 'default' | 'all' | 'query'; range: NetflowRange; filters: NetflowFilters }
  | { ok: false; error: string };

const DAY_MS = 86_400_000;

function lowerBound(t: TimeValue, now: number): number {
  return t.kind === 'ago' ? now - t.ms : t.kind === 'day' ? t.start : t.ms;
}

/** `until:` is inclusive: a day runs through its last millisecond. */
function upperBound(t: TimeValue, now: number): number {
  return t.kind === 'ago' ? now - t.ms : t.kind === 'day' ? t.start + DAY_MS - 1 : t.ms;
}

/** A time term's bound: its values OR, so the widest one. */
function termBound(t: Term, bound: (v: TimeValue) => number, widest: (a: number, b: number) => number): number {
  return t.values.map((v) => bound(parseTimeValue(v)!)).reduce((a, b) => widest(a, b));
}

/**
 * The reads' params for `q`, its spans resolved against `now`. Terms AND: a
 * second `since:` narrows. A dimension's values OR in one term (`origin:a,b`)
 * and a negated term leaves its values out; two positive terms on one
 * dimension can't be asked of the server, nor can free text or a negated
 * `since:` / `until:` / `window:`.
 */
export function netflowRequest(q: string, now: number): NetflowRequest {
  const p = parseQuery(q, NETFLOW_FIELDS);
  if (!p.ok) return { ok: false, error: p.error.message };
  const filters: NetflowFilters = {
    workflows: [], origins: [], orgs: [], hosts: [],
    not_workflows: [], not_origins: [], not_orgs: [], not_hosts: [],
  };
  let from: number | undefined;
  let to: number | undefined;
  let all = false;
  const positive = new Set<string>();
  for (const t of p.terms) {
    if (t.key === 'text') {
      return { ok: false, error: `\`${t.values.join(',')}\`: the network view takes keyed terms only (since:, until:, workflow:, origin:, org:, host:)` };
    }
    if (t.neg && (t.key === 'since' || t.key === 'until' || t.key === 'window')) {
      return { ok: false, error: `\`-${t.key}:\` can't be negated${t.key === 'window' ? '' : `; use ${t.key === 'since' ? 'until:' : 'since:'}`}` };
    }
    if (t.key === 'since') {
      const b = termBound(t, (v) => lowerBound(v, now), Math.min);
      from = from === undefined ? b : Math.max(from, b);
    } else if (t.key === 'until') {
      const b = termBound(t, (v) => upperBound(v, now), Math.max);
      to = to === undefined ? b : Math.min(to, b);
    } else if (t.key === 'window') {
      all = true;
    } else {
      const [inc, exc] = NETFLOW_DIMS[t.key as NetflowDim];
      if (t.neg) {
        filters[exc] = [...new Set([...filters[exc], ...t.values])];
      } else {
        if (positive.has(t.key)) {
          return { ok: false, error: `one \`${t.key}:\` term at a time; list several values with commas (${t.key}:a,b)` };
        }
        positive.add(t.key);
        filters[inc] = [...t.values];
      }
    }
  }
  const range: NetflowRange = {};
  if (from !== undefined) range.from = new Date(from).toISOString();
  if (to !== undefined) range.to = new Date(to).toISOString();
  const window = from !== undefined || to !== undefined ? 'query' : all ? 'all' : 'default';
  return { ok: true, window, range, filters };
}

export type DefaultWindow =
  | { kind: 'hour'; range: NetflowRange }
  | { kind: 'run'; range: NetflowRange; run: LatestRun }
  | { kind: 'span'; range: NetflowRange }
  | { kind: 'all'; range: NetflowRange };

const HOUR_MS = 3_600_000;

/**
 * The window the explorer opens on when the query names none. A run's own
 * Network tab (`span`, the run's span) opens on that span. Elsewhere it is
 * the shorter of the last hour and the latest run's span (start to finish,
 * or to now while it runs), so the first load reads little.
 */
export function defaultWindow(now: number, opts: { span?: NetflowRange; latest?: LatestRun | null }): DefaultWindow {
  if (opts.span) return opts.span.from || opts.span.to ? { kind: 'span', range: opts.span } : { kind: 'all', range: {} };
  const hour: DefaultWindow = { kind: 'hour', range: { from: new Date(now - HOUR_MS).toISOString() } };
  const run = opts.latest;
  if (!run) return hour;
  const start = Date.parse(run.started_at);
  const end = run.finished_at ? Date.parse(run.finished_at) : now;
  if (Number.isNaN(start) || Number.isNaN(end) || end - start >= HOUR_MS) return hour;
  return { kind: 'run', range: spanWindow(run.started_at, run.finished_at), run };
}

function minutes(ms: number): string {
  const m = Math.max(1, Math.round(ms / 60_000));
  return m < 60 ? `${m} min` : `${Math.round(m / 6) / 10} h`;
}

/** The quiet line under the query bar that says which default is in effect. */
export function defaultWindowText(w: DefaultWindow, now: number): string {
  const how = 'since: or until: picks another window; window:all shows every flow.';
  switch (w.kind) {
    case 'hour':
      return `Showing the last hour (the default). ${how}`;
    case 'span':
      return `Showing this run's span (the default). ${how}`;
    case 'all':
      return 'Showing every recorded flow.';
    case 'run': {
      const end = w.run.finished_at ? Date.parse(w.run.finished_at) : now;
      const who = w.run.run_id ? `${w.run.workflow ?? w.run.run_id}, ${w.run.run_id}` : 'a run with no record';
      const live = w.run.finished_at ? '' : ', still running';
      return `Showing the latest run's span (${who}: ${minutes(end - Date.parse(w.run.started_at))}${live}), the default: it is shorter than the last hour. ${how}`;
    }
  }
}

function tokensOf(q: string): string[] {
  const t = tokenize(q);
  return t.ok ? t.tokens.map((x) => x.text) : q.trim() ? [q.trim()] : [];
}

function termOf(raw: string): Term | null {
  const p = parseQuery(raw, NETFLOW_FIELDS);
  return p.ok && p.terms.length === 1 ? p.terms[0] : null;
}

/**
 * A graph click on `value` of `key`: drop it from the query's positive term
 * when it's there, else keep it (adding it to that term, and taking it out
 * of a negated one). Every other token stays as written.
 */
export function toggleNetflowValue(q: string, key: NetflowDim, value: string): string {
  const pos: string[] = [];
  const neg: string[] = [];
  const kept: string[] = [];
  for (const raw of tokensOf(q)) {
    const t = termOf(raw);
    if (t && t.key === key) (t.neg ? neg : pos).push(...t.values);
    else kept.push(raw);
  }
  const has = pos.includes(value);
  const nextPos = has ? pos.filter((v) => v !== value) : [...pos, value];
  const nextNeg = has ? neg : neg.filter((v) => v !== value);
  const spell = (vs: string[]) => vs.map(quoteValue).join(',');
  return [
    ...kept,
    ...(nextPos.length ? [`${key}:${spell(nextPos)}`] : []),
    ...(nextNeg.length ? [`-${key}:${spell(nextNeg)}`] : []),
  ].join(' ');
}

/** The query with its window replaced by `[from, to]` (a zoom). */
export function withNetflowWindow(q: string, from: string, to: string): string {
  const kept = tokensOf(q).filter((raw) => {
    const t = termOf(raw);
    return !(t && (t.key === 'since' || t.key === 'until' || t.key === 'window'));
  });
  return [...kept, `since:${from}`, `until:${to}`].join(' ');
}
