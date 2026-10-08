// assetQuery — the engagement-asset query, built on the SAME grammar as the
// findings query (`findingQuery/grammar`, generic over a field registry). The
// field set is derived from the loaded assets (so no present value is ever
// rejected), and — because `/api/assets` has no server-side `?q=` — the parsed
// terms are evaluated client-side against `AssetRow` here. Reusable anywhere
// assets are shown (the Security → Assets page, the agentiflow Assets tab).
//
// `depth` is an ordered rung ladder, so it is declared with the grammar's
// ordered `severity` kind to unlock `depth>=tested`; everything else is `eq`.

import { Layers, Shapes, Gauge, Server, EthernetPort, Waypoints, FileCode, Type, Hash, type LucideIcon } from 'lucide-react';
import { parseQuery, type Term } from './findingQuery/grammar';
import type { QueryField } from './findingQuery/fields';
import type { AssetRow } from './api';
import type { BadgeTone } from '../components/ui/Badge';

/** Coverage-depth rungs, shallow → deep, across the built-in profiles. Drives
 *  both the `depth>=` comparison and the deepest-first ordering of a section. */
export const DEPTH_ORDER = ['discovered', 'mapped', 'unreviewed', 'enumerated', 'crawled', 'reviewed', 'tested', 'confirmed', 'exploited'];

/** Depth rung → badge tone. Deeper = hotter; unknown rungs stay neutral. */
export const DEPTH_TONE: Record<string, BadgeTone> = {
  discovered: 'neutral',
  mapped: 'neutral',
  unreviewed: 'neutral',
  enumerated: 'violet',
  crawled: 'violet',
  reviewed: 'green',
  tested: 'amber',
  confirmed: 'amber',
  exploited: 'red',
};

export function depthRank(d?: string | null): number {
  const i = d ? DEPTH_ORDER.indexOf(d.toLowerCase()) : -1;
  return i < 0 ? -1 : i;
}

const BASE_PROTOS = ['tcp', 'udp', 'icmp', 'sctp'];

function uniq(xs: (string | undefined | null)[]): string[] {
  return [...new Set(xs.filter((x): x is string => typeof x === 'string' && x !== ''))];
}
function coord(a: AssetRow, k: string): unknown {
  return (a.coords as Record<string, unknown>)[k];
}
function coordStr(a: AssetRow): string {
  return Object.values(a.coords as Record<string, unknown>)
    .map((v) => `${v}`)
    .join(' ');
}

/** The query fields, derived from the loaded assets. `icon`/`label`/`description`
 *  are what the `QueryBar` shows; `values` double as the suggestion/validation
 *  set for the closed (enum) fields. */
export function assetFields(assets: AssetRow[]): QueryField[] {
  const profiles = uniq(assets.map((a) => a.profile)).sort();
  const kinds = uniq(assets.map((a) => a.sub_kind)).sort();
  const present = uniq(assets.map((a) => a.depth));
  const depths = DEPTH_ORDER.filter((d) => present.includes(d)).concat(present.filter((d) => !DEPTH_ORDER.includes(d)));
  const protos = uniq([...BASE_PROTOS, ...assets.map((a) => (typeof coord(a, 'proto') === 'string' ? (coord(a, 'proto') as string) : undefined))]);
  const field = (key: string, kind: QueryField['kind'], values: string[], label: string, description: string, icon: LucideIcon, aliases: string[] = []): QueryField => ({ key, aliases, kind, values, label, description, icon });
  return [
    field('profile', 'enum', profiles, 'Profile', 'profile:network', Layers),
    field('kind', 'enum', kinds, 'Kind', 'kind:service', Shapes, ['type']),
    field('depth', 'severity', depths, 'Depth', 'depth:exploited · depth>=tested', Gauge),
    field('host', 'text', [], 'Host', 'host:38.104. (contains)', Server),
    field('port', 'text', [], 'Port', 'port:3389', EthernetPort),
    field('proto', 'enum', protos, 'Proto', 'proto:tcp', Waypoints),
    field('path', 'text', [], 'Path', 'path:gateway/auth', FileCode),
    field('label', 'text', [], 'Label', 'label:router', Type),
    field('id', 'text', [], 'Id', 'id:…', Hash),
  ];
}

/** Values in use per field, with counts — feeds the QueryBar dropdown. */
export function assetFacets(assets: AssetRow[]): Record<string, { value: string; count: number }[]> {
  const tally = (vals: (string | undefined | null)[]) => {
    const m = new Map<string, number>();
    for (const v of vals) if (typeof v === 'string' && v !== '') m.set(v, (m.get(v) ?? 0) + 1);
    return [...m].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
  };
  return {
    profile: tally(assets.map((a) => a.profile)),
    kind: tally(assets.map((a) => a.sub_kind)),
    depth: tally(assets.map((a) => a.depth)),
    proto: tally(assets.map((a) => (typeof coord(a, 'proto') === 'string' ? (coord(a, 'proto') as string) : undefined))),
    host: tally(assets.map((a) => (typeof coord(a, 'host') === 'string' ? (coord(a, 'host') as string) : undefined))),
  };
}

// Fields compared by exact (case-insensitive) equality; the rest are substring.
const EXACT = new Set(['profile', 'kind', 'depth', 'proto', 'port', 'id']);

function valuesFor(a: AssetRow, key: string): string[] {
  switch (key) {
    case 'profile':
      return [a.profile];
    case 'kind':
      return [a.sub_kind, a.kind];
    case 'depth':
      return a.depth ? [a.depth] : [];
    case 'host':
      return typeof coord(a, 'host') === 'string' ? [coord(a, 'host') as string] : [];
    case 'port':
      return coord(a, 'port') != null ? [String(coord(a, 'port'))] : [];
    case 'proto':
      return typeof coord(a, 'proto') === 'string' ? [coord(a, 'proto') as string] : [];
    case 'path':
      return typeof coord(a, 'path') === 'string' ? [coord(a, 'path') as string] : [];
    case 'label':
      return [a.label];
    case 'id':
      return [a.id];
    default:
      return [a.label, a.sub_kind, a.profile, a.id, coordStr(a)];
  }
}

function cmp(ai: number, ti: number, op: Term['op']): boolean {
  switch (op) {
    case 'gt':
      return ai > ti;
    case 'ge':
      return ai >= ti;
    case 'lt':
      return ai < ti;
    case 'le':
      return ai <= ti;
    default:
      return ai === ti;
  }
}

function termMatches(a: AssetRow, t: Term): boolean {
  let hit: boolean;
  if (t.key === 'depth' && t.op !== 'eq') {
    const ai = depthRank(a.depth);
    const ti = depthRank(t.values[0]);
    hit = ai >= 0 && ti >= 0 && cmp(ai, ti, t.op);
  } else {
    const vals = valuesFor(a, t.key).map((v) => v.toLowerCase());
    const exact = EXACT.has(t.key);
    hit = t.values.some((raw) => {
      const v = raw.toLowerCase();
      return vals.some((x) => (exact ? x === v : x.includes(v)));
    });
  }
  return t.neg ? !hit : hit;
}

/** Filter assets by a query string. On a parse error, nothing is filtered (the
 *  QueryBar shows the offending chip in red) and the message is returned. */
export function filterAssets(assets: AssetRow[], q: string, fields: QueryField[]): { rows: AssetRow[]; error: string | null } {
  if (!q.trim()) return { rows: assets, error: null };
  const p = parseQuery(q, fields);
  if (!p.ok) return { rows: assets, error: p.error.message };
  return { rows: assets.filter((a) => p.terms.every((t) => termMatches(a, t))), error: null };
}
