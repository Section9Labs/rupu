// AssetInventory — the read view for non-code (and code) engagement assets.
// Consumes `GET /api/assets` and renders each registered workspace's assets as
// the profile's own graph: hosts → services for `network`, sites → routes for
// `web`, files for `code`. Each asset carries a coverage-depth badge (the rung
// of its profile's ladder it reached: discovered → enumerated → tested →
// exploited, etc.). Used by the Security → Assets page and the agentiflow
// detail's Assets tab (the latter via `wsId` / `target` scoping).

import { useCallback, useEffect, useRef, useState } from 'react';
import { Server, Network, Globe, Route, FileCode, Boxes, RefreshCw } from 'lucide-react';
import { api, apiErrorMessage, type AssetRow } from '../../lib/api';
import { Badge, type BadgeTone } from '../ui/Badge';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { cn } from '../../lib/cn';

/** Icon per bare kind. */
function KindIcon({ sub, className }: { sub: string; className?: string }) {
  const map: Record<string, typeof Server> = {
    host: Server,
    service: Network,
    site: Globe,
    route: Route,
    file: FileCode,
    function: FileCode,
  };
  const Icon = map[sub] ?? Boxes;
  return <Icon size={14} className={className} />;
}

/** Depth rung → badge tone. Deeper = hotter; unknown rungs stay neutral. */
const DEPTH_TONE: Record<string, BadgeTone> = {
  discovered: 'neutral',
  mapped: 'neutral',
  unreviewed: 'neutral',
  enumerated: 'violet',
  crawled: 'violet',
  reviewed: 'green',
  tested: 'amber',
  exploited: 'red',
};

function coordText(a: AssetRow): string | null {
  const c = a.coords as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof c.host === 'string') parts.push(c.host);
  if (typeof c.port === 'number') parts.push(`:${c.port}${typeof c.proto === 'string' && c.proto !== 'tcp' ? `/${c.proto}` : ''}`);
  if (typeof c.url === 'string') parts.push(c.url);
  if (typeof c.http_route === 'string') parts.push(c.http_route);
  if (typeof c.path === 'string') parts.push(c.path);
  if (typeof c.resource_id === 'string') parts.push(c.resource_id);
  const joined = parts.join('').trim();
  // Only show coords when they add something the label doesn't already carry.
  return joined && joined !== a.label ? joined : null;
}

function AssetLine({ asset, depthPx }: { asset: AssetRow; depthPx: number }) {
  const coords = coordText(asset);
  const tone = asset.depth ? DEPTH_TONE[asset.depth] ?? 'neutral' : undefined;
  return (
    <div className="flex items-center gap-2.5 py-1.5" style={{ paddingLeft: depthPx }}>
      <KindIcon sub={asset.sub_kind} className="shrink-0 text-ink-mute" />
      <span className="truncate text-sm text-ink" title={asset.label}>
        {asset.label}
      </span>
      <span className="shrink-0 font-mono text-meta text-ink-mute">{asset.sub_kind}</span>
      {coords && <span className="truncate font-mono text-meta text-ink-mute">{coords}</span>}
      <span className="ml-auto shrink-0">
        {asset.depth ? <Badge tone={tone!}>{asset.depth}</Badge> : <span className="text-meta text-ink-mute">—</span>}
      </span>
    </div>
  );
}

/** One project's (workspace's) assets as a parent→child tree. */
function ProjectAssets({ project, assets }: { project: string; assets: AssetRow[] }) {
  const byId = new Map(assets.map((a) => [a.id, a]));
  const children = new Map<string, AssetRow[]>();
  const roots: AssetRow[] = [];
  for (const a of assets) {
    if (a.parent && byId.has(a.parent)) {
      (children.get(a.parent) ?? children.set(a.parent, []).get(a.parent)!).push(a);
    } else {
      roots.push(a);
    }
  }
  const sortKey = (a: AssetRow) => a.label.toLowerCase();
  roots.sort((x, y) => sortKey(x).localeCompare(sortKey(y)));

  // Per-kind + per-depth tallies for the section header.
  const kinds = new Map<string, number>();
  const depths = new Map<string, number>();
  for (const a of assets) {
    kinds.set(a.sub_kind, (kinds.get(a.sub_kind) ?? 0) + 1);
    if (a.depth) depths.set(a.depth, (depths.get(a.depth) ?? 0) + 1);
  }

  const renderNode = (a: AssetRow, depth: number): React.ReactNode => {
    const kids = (children.get(a.id) ?? []).sort((x, y) => sortKey(x).localeCompare(sortKey(y)));
    return (
      <div key={a.id}>
        <AssetLine asset={a} depthPx={12 + depth * 20} />
        {kids.map((k) => renderNode(k, depth + 1))}
      </div>
    );
  };

  return (
    <section className="rounded-xl border border-border bg-panel shadow-card">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-2.5">
        <span className="font-mono text-sm font-medium text-ink">{project}</span>
        <span className="text-note tabular-nums text-ink-dim">{assets.length} assets</span>
        <span className="ml-auto flex flex-wrap items-center gap-1.5">
          {[...kinds].map(([k, n]) => (
            <span key={k} className="inline-flex items-center gap-1 text-meta text-ink-mute">
              <KindIcon sub={k} />
              {n} {k}
            </span>
          ))}
        </span>
      </div>
      <div className="divide-y divide-border px-2 py-1">{roots.map((r) => renderNode(r, 0))}</div>
    </section>
  );
}

export default function AssetInventory({
  wsId,
  target,
  emptyHint,
}: {
  wsId?: string;
  target?: string;
  emptyHint?: React.ReactNode;
}) {
  const [assets, setAssets] = useState<AssetRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
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

  if (assets === null && loading) {
    return (
      <div className="py-12 flex items-center justify-center">
        <Spinner label="Loading assets…" />
      </div>
    );
  }
  if (error) return <ErrorBanner>{error}</ErrorBanner>;
  if (!assets || assets.length === 0) {
    return (
      <EmptyState
        title="No assets recorded yet"
        hint={
          emptyHint ?? (
            <>Assets appear here once an engagement records them with the <span className="font-mono">assets.mark</span> tool (hosts/services for network, sites/routes for web, files for code).</>
          )
        }
      />
    );
  }

  // An asset id is globally unique (kind + locator), so the same asset served
  // from two registry entries that point at one path (the duplicate-workspace
  // bug) is a true duplicate — collapse by id before display.
  const unique = Array.from(new Map(assets.map((a) => [a.id, a])).values());

  // Group by project (workspace) for display.
  const byProject = new Map<string, AssetRow[]>();
  for (const a of unique) {
    const key = a.project || a.ws_id;
    (byProject.get(key) ?? byProject.set(key, []).get(key)!).push(a);
  }
  const projects = [...byProject.entries()].sort((a, b) => b[1].length - a[1].length);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <span className="text-note tabular-nums text-ink-dim">
          {unique.length} assets across {projects.length} {projects.length === 1 ? 'project' : 'projects'}
        </span>
        <Button variant="secondary" onClick={() => load()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </div>
      {projects.map(([project, rows]) => (
        <ProjectAssets key={project} project={project} assets={rows} />
      ))}
    </div>
  );
}
