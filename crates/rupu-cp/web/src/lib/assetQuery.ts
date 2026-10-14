// assetQuery — the engagement-asset query: a `RowField` registry over
// `AssetRow`, evaluated client-side by `lib/query/rows` (`/api/assets` has no
// server-side `?q=`). The field set is derived from the loaded assets, so no
// present value is ever rejected. Reusable anywhere assets are shown (the
// Security → Assets page, the agentiflow Assets tab).
//
// `depth` is an ordered rung ladder (`depth>=tested`); everything else is `:`.

import { Layers, Shapes, Gauge, Server, EthernetPort, Waypoints, FileCode, Type, Hash, type LucideIcon } from 'lucide-react';
import { filterRows, rowFacets, rowField, type Facets, type FilterResult, type RowField } from './query/rows';
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

const strCoord = (a: AssetRow, k: string): string | undefined => (typeof coord(a, k) === 'string' ? (coord(a, k) as string) : undefined);

/** The query fields, derived from the loaded assets. `values` double as the
 *  suggestion/validation set for the closed (enum) fields. */
export function assetFields(assets: AssetRow[]): RowField<AssetRow>[] {
  const profiles = uniq(assets.map((a) => a.profile)).sort();
  const kinds = uniq(assets.map((a) => a.sub_kind)).sort();
  const present = uniq(assets.map((a) => a.depth));
  const depths = DEPTH_ORDER.filter((d) => present.includes(d)).concat(present.filter((d) => !DEPTH_ORDER.includes(d)));
  const protos = uniq([...BASE_PROTOS, ...assets.map((a) => strCoord(a, 'proto'))]);
  const field = (
    key: string,
    match: RowField<AssetRow>['match'],
    get: RowField<AssetRow>['get'],
    values: string[],
    label: string,
    description: string,
    icon: LucideIcon,
    extra: Partial<RowField<AssetRow>> = {},
  ) => rowField<AssetRow>({ key, match, get, values, label, description, icon, ...extra });
  return [
    field('profile', 'exact', (a) => a.profile, profiles, 'Profile', 'profile:network', Layers, { facet: true }),
    field('kind', 'exact', (a) => [a.sub_kind, a.kind], kinds, 'Kind', 'kind:service', Shapes, { aliases: ['type'], facet: (a) => a.sub_kind }),
    field('depth', 'ordered', (a) => a.depth, depths, 'Depth', 'depth:exploited · depth>=tested', Gauge, { facet: true }),
    field('host', 'contains', (a) => strCoord(a, 'host'), [], 'Host', 'host:38.104. (contains)', Server, { facet: true }),
    field('port', 'exact', (a) => (coord(a, 'port') != null ? String(coord(a, 'port')) : undefined), [], 'Port', 'port:3389', EthernetPort),
    field('proto', 'exact', (a) => strCoord(a, 'proto'), protos, 'Proto', 'proto:tcp', Waypoints, { facet: true }),
    field('path', 'contains', (a) => strCoord(a, 'path'), [], 'Path', 'path:gateway/auth', FileCode),
    field('label', 'contains', (a) => a.label, [], 'Label', 'label:router', Type),
    field('id', 'exact', (a) => a.id, [], 'Id', 'id:…', Hash),
  ];
}

/** Values in use per field, with counts — feeds the QueryBar dropdown. */
export function assetFacets(assets: AssetRow[]): Facets {
  return rowFacets(assets, assetFields(assets));
}

/** Bare words search the label, kind, profile, id and every coordinate. */
const assetText = (a: AssetRow) => [a.label, a.sub_kind, a.profile, a.id, coordStr(a)];

/** Filter assets by a query string. On a parse error, nothing is filtered (the
 *  QueryBar shows the offending chip in red) and the message is returned. */
export function filterAssets(assets: AssetRow[], q: string, fields: RowField<AssetRow>[]): FilterResult<AssetRow> {
  return filterRows(assets, q, fields, { text: assetText });
}
