// AssetInventory — the read view for engagement assets. Consumes `GET /api/assets`
// and presents each profile's assets as its own relationship graph, not one
// mixed list: `network` as a host → service tree (linked by the service's
// `coords.host`, since the `parent` edge is not populated), `web` as sites/routes,
// `code` as modules/files. Each asset carries its coverage-depth rung
// (discovered → … → tested → exploited). A query bar — the same grammar as the
// findings query (`lib/assetQuery`) — filters the view client-side.
//
// Used by the Security → Assets page and the agentiflow detail's Assets tab
// (the latter via `wsId` / `target` scoping).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Network, Globe, Code, Binary, Boxes, Server, Plug, Route, FileCode, Package, FunctionSquare, RefreshCw, ChevronRight, ChevronDown, TriangleAlert, type LucideIcon } from 'lucide-react';
import { api, apiErrorMessage, type AssetRow } from '../../lib/api';
import { QueryBar } from '../query/QueryBar';
import { assetFields, assetFacets, filterAssets, DEPTH_TONE, depthRank } from '../../lib/assetQuery';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { cn } from '../../lib/cn';

const PROFILE_ICON: Record<string, LucideIcon> = { network: Network, web: Globe, code: Code, binary: Binary };
const KIND_ICON: Record<string, LucideIcon> = { host: Server, service: Plug, site: Globe, route: Route, file: FileCode, module: Package, function: FunctionSquare, package: Package };
const profIcon = (p: string): LucideIcon => PROFILE_ICON[p] ?? Boxes;
const kindIcon = (k: string): LucideIcon => KIND_ICON[k] ?? Boxes;

// Depth-tone → a flat color for the coverage bar segment + legend dot.
const TONE_HEX: Record<string, string> = { neutral: '#94a3b8', violet: '#8b5cf6', green: '#16a34a', amber: '#d97706', red: '#dc2626' };

function coord(a: AssetRow, k: string): unknown {
  return (a.coords as Record<string, unknown>)[k];
}
function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}

/** Secondary coordinates worth showing on a generic (non-host/service) row. */
function coordText(a: AssetRow): string | null {
  const parts: string[] = [];
  const url = str(coord(a, 'url'));
  const route = str(coord(a, 'http_route'));
  const path = str(coord(a, 'path'));
  const res = str(coord(a, 'resource_id'));
  if (url) parts.push(url);
  if (route) parts.push(route);
  if (path) parts.push(path);
  if (res) parts.push(res);
  const joined = parts.join(' ').trim();
  return joined && joined !== a.label ? joined : null;
}

function DepthBadge({ depth }: { depth?: string | null }) {
  if (!depth) return <span className="text-meta text-ink-mute">—</span>;
  return <Badge tone={DEPTH_TONE[depth] ?? 'neutral'}>{depth}</Badge>;
}

/** A thin stacked coverage bar (depth counts, shallow → deep) + legend. */
function DepthBar({ assets }: { assets: AssetRow[] }) {
  const counts = new Map<string, number>();
  for (const a of assets) if (a.depth) counts.set(a.depth, (counts.get(a.depth) ?? 0) + 1);
  const entries = [...counts].sort((x, y) => depthRank(x[0]) - depthRank(y[0]));
  const total = entries.reduce((s, [, n]) => s + n, 0);
  if (total === 0) return null;
  return (
    <div className="ml-auto flex items-center gap-2">
      <div className="flex h-1.5 w-28 overflow-hidden rounded-full">
        {entries.map(([d, n]) => (
          <div key={d} style={{ flex: n, background: TONE_HEX[DEPTH_TONE[d] ?? 'neutral'] }} title={`${d}: ${n}`} />
        ))}
      </div>
      <span className="hidden flex-wrap gap-x-2 text-meta text-ink-mute sm:flex">
        {entries.map(([d, n]) => (
          <span key={d} className="inline-flex items-center gap-1">
            <span className="h-1.5 w-1.5 rounded-sm" style={{ background: TONE_HEX[DEPTH_TONE[d] ?? 'neutral'] }} />
            {d} {n}
          </span>
        ))}
      </span>
    </div>
  );
}

function ipKey(h: AssetRow): string {
  const ip = str(coord(h, 'host')) ?? '';
  const m = ip.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (m) return m.slice(1).map((x) => x.padStart(3, '0')).join('.');
  return (h.label || ip).toLowerCase();
}

function ServiceRow({ svc }: { svc: AssetRow }) {
  const port = coord(svc, 'port');
  const proto = str(coord(svc, 'proto'));
  const Icon = kindIcon(svc.sub_kind);
  return (
    <div className="flex items-center gap-2.5 py-1.5 pl-9 pr-1">
      <Icon size={13} className="shrink-0 text-ink-mute" />
      {port != null && (
        <span className="shrink-0 font-mono text-note text-ink">
          :{String(port)}
          {proto && proto !== 'tcp' ? `/${proto}` : ''}
        </span>
      )}
      <span className="truncate text-sm text-ink-dim" title={svc.label}>
        {svc.label}
      </span>
      <span className="ml-auto shrink-0">
        <DepthBadge depth={svc.depth} />
      </span>
    </div>
  );
}

function GenericRow({ asset, indent = false }: { asset: AssetRow; indent?: boolean }) {
  const Icon = kindIcon(asset.sub_kind);
  const coords = coordText(asset);
  return (
    <div className={cn('flex items-center gap-2.5 py-1.5 pr-1', indent ? 'pl-9' : 'pl-3')}>
      <Icon size={13} className="shrink-0 text-ink-mute" />
      <span className="truncate text-sm text-ink" title={asset.label}>
        {asset.label}
      </span>
      {coords && <span className="truncate font-mono text-meta text-ink-mute">{coords}</span>}
      <span className="ml-auto shrink-0">
        <DepthBadge depth={asset.depth} />
      </span>
    </div>
  );
}

function HostNode({ host, services }: { host: AssetRow; services: AssetRow[] }) {
  const [open, setOpen] = useState(true);
  const ip = str(coord(host, 'host')) ?? '';
  const name = host.label && host.label !== ip ? host.label : ip;
  const secondary = name !== ip && ip ? ip : null;
  const hasKids = services.length > 0;
  return (
    <div>
      <button
        type="button"
        onClick={() => hasKids && setOpen((v) => !v)}
        className={cn('flex w-full items-center gap-2.5 py-1.5 pl-2 pr-1 text-left', hasKids && 'hover:bg-surface/50')}
      >
        <span className="shrink-0 text-ink-mute">{hasKids ? open ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : <span className="inline-block w-[13px]" />}</span>
        <Server size={14} className="shrink-0 text-ink-mute" />
        <span className="truncate text-sm font-medium text-ink" title={host.label}>
          {name}
        </span>
        {secondary && <span className="shrink-0 font-mono text-meta text-ink-mute">{secondary}</span>}
        <span className="ml-auto flex shrink-0 items-center gap-2.5">
          {hasKids && <span className="text-meta tabular-nums text-ink-mute">{services.length} svc</span>}
          <DepthBadge depth={host.depth} />
        </span>
      </button>
      {hasKids && open && (
        <div className="border-l border-border/70 ml-[14px]">
          {services.map((s) => (
            <ServiceRow key={s.id} svc={s} />
          ))}
        </div>
      )}
    </div>
  );
}

function ProfileSection({ profile, assets }: { profile: string; assets: AssetRow[] }) {
  const Icon = profIcon(profile);
  const kindCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const a of assets) m.set(a.sub_kind, (m.get(a.sub_kind) ?? 0) + 1);
    return [...m].sort((a, b) => b[1] - a[1]);
  }, [assets]);

  // network-style host → service tree, linked by the service's `coords.host`.
  const { hosts, unlinked, flat } = useMemo(() => {
    const hostAssets = assets.filter((a) => a.sub_kind === 'host');
    if (hostAssets.length === 0) return { hosts: [] as { host: AssetRow; services: AssetRow[] }[], unlinked: [] as AssetRow[], flat: assets };
    const byIp = new Map<string, AssetRow>();
    for (const h of hostAssets) {
      const ip = str(coord(h, 'host'));
      if (ip && !byIp.has(ip)) byIp.set(ip, h);
    }
    const children = new Map<string, AssetRow[]>();
    const orphans: AssetRow[] = [];
    for (const a of assets) {
      if (a.sub_kind === 'host') continue;
      const ip = str(coord(a, 'host'));
      if (ip && byIp.has(ip)) (children.get(ip) ?? children.set(ip, []).get(ip)!).push(a);
      else orphans.push(a);
    }
    const hostList = [...byIp.values()]
      .sort((x, y) => ipKey(x).localeCompare(ipKey(y)))
      .map((h) => ({ host: h, services: (children.get(str(coord(h, 'host'))!) ?? []).sort((a, b) => Number(coord(a, 'port') ?? 0) - Number(coord(b, 'port') ?? 0)) }));
    return { hosts: hostList, unlinked: orphans, flat: [] as AssetRow[] };
  }, [assets]);

  const isTree = hosts.length > 0;

  return (
    <section className="rounded-xl border border-border bg-panel shadow-card">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-2.5">
        <Icon size={15} className="shrink-0 text-ink-dim" />
        <span className="text-sm font-semibold capitalize text-ink">{profile}</span>
        <span className="flex flex-wrap items-center gap-x-2.5 text-meta text-ink-mute">
          {kindCounts.map(([k, n]) => (
            <span key={k} className="tabular-nums">
              {n} {k}
              {n === 1 ? '' : 's'}
            </span>
          ))}
        </span>
        <DepthBar assets={assets} />
      </div>
      <div className="px-2 py-1">
        {isTree ? (
          <div className="divide-y divide-border">
            {hosts.map(({ host, services }) => (
              <HostNode key={host.id} host={host} services={services} />
            ))}
            {unlinked.length > 0 && (
              <div className="pt-1">
                <div className="flex items-center gap-2 py-1.5 pl-2 text-meta text-ink-mute">
                  <TriangleAlert size={13} className="shrink-0 text-warn" />
                  Unlinked · {unlinked.length} with no recorded host
                </div>
                {unlinked.sort((a, b) => a.label.localeCompare(b.label)).map((a) => (
                  <GenericRow key={a.id} asset={a} indent />
                ))}
              </div>
            )}
          </div>
        ) : (
          <div className="divide-y divide-border">
            {[...flat].sort((a, b) => a.sub_kind.localeCompare(b.sub_kind) || a.label.localeCompare(b.label)).map((a) => (
              <GenericRow key={a.id} asset={a} />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

export default function AssetInventory({ wsId, target, emptyHint }: { wsId?: string; target?: string; emptyHint?: React.ReactNode }) {
  const [assets, setAssets] = useState<AssetRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState('');
  const seq = useRef(0);

  const load = useCallback(() => {
    const mine = ++seq.current;
    const ctl = new AbortController();
    setLoading(true);
    api
      .getAssets({ ws_id: wsId, target, signal: ctl.signal })
      .then((rows) => {
        if (seq.current !== mine) return;
        setAssets(rows);
        setError(null);
      })
      .catch((e: unknown) => {
        if (seq.current !== mine || ctl.signal.aborted) return;
        setError(apiErrorMessage(e));
      })
      .finally(() => {
        if (seq.current === mine) setLoading(false);
      });
    return () => ctl.abort();
  }, [wsId, target]);

  useEffect(() => {
    const abort = load();
    return () => {
      seq.current++;
      abort();
    };
  }, [load]);

  // An asset id is globally unique (kind + locator), so the same asset served
  // from two registry entries that point at one path is a true duplicate —
  // collapse by id before display.
  const unique = useMemo(() => Array.from(new Map((assets ?? []).map((a) => [a.id, a])).values()), [assets]);
  const fields = useMemo(() => assetFields(unique), [unique]);
  const facets = useMemo(() => assetFacets(unique), [unique]);
  const { rows } = useMemo(() => filterAssets(unique, q, fields), [unique, q, fields]);

  const byProfile = useMemo(() => {
    const m = new Map<string, AssetRow[]>();
    for (const a of rows) (m.get(a.profile) ?? m.set(a.profile, []).get(a.profile)!).push(a);
    return [...m.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [rows]);

  if (assets === null && loading) {
    return (
      <div className="py-12 flex items-center justify-center">
        <Spinner label="Loading assets…" />
      </div>
    );
  }
  if (error) return <ErrorBanner>{error}</ErrorBanner>;
  if (unique.length === 0) {
    return (
      <EmptyState
        title="No assets recorded yet"
        hint={emptyHint ?? (<>Assets appear here once an engagement records them with the <span className="font-mono">asset_mark</span> tool (hosts/services for network, sites/routes for web, files for code).</>)}
      />
    );
  }

  return (
    <div className="space-y-3">
      <QueryBar
        value={q}
        onChange={setQ}
        fields={fields}
        facets={facets}
        label="Filter assets"
        placeholder="Filter assets… e.g. kind:service depth>=tested host:38.104."
      />
      <div className="flex items-center justify-between">
        <span className="text-note tabular-nums text-ink-dim">
          {q.trim() ? `${rows.length} of ${unique.length}` : unique.length} assets · {byProfile.length} {byProfile.length === 1 ? 'profile' : 'profiles'}
        </span>
        <Button variant="secondary" onClick={() => load()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </div>
      {byProfile.map(([profile, profAssets]) => (
        <ProfileSection key={profile} profile={profile} assets={profAssets} />
      ))}
      {rows.length === 0 && <p className="rounded-xl border border-border bg-panel px-4 py-8 text-center text-sm text-ink-dim">No assets match the filter.</p>}
    </div>
  );
}
