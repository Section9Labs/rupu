// Catalog tab — the effective concern catalog snapshot for a target.
import { useEffect, useState } from 'react';
import { api, type FlatCatalog } from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageCatalogTab({ target, wsId }: { target: string; wsId?: string }) {
  const [cat, setCat] = useState<FlatCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setCat(null);
    setError(null);
    api
      .getCoverageCatalog(target, wsId)
      .then((d) => {
        if (!cancelled) setCat(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load catalog');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!cat) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (cat.concerns.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No catalog snapshot for this target.</p>;

  return (
    <section className="mt-6">
      <SectionHeader tone="muted" label="Catalog concerns" count={cat.concerns.length} />
      <ListCard>
        {cat.concerns.map((c) => (
          <div key={c.id} className="px-4 py-3">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium text-ink">{c.name}</span>
              <span className="text-[11px] font-mono text-ink-mute">{c.id}</span>
              <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
                {c.severity}
              </span>
              <span className="text-[10px] text-ink-mute">{cat.sources[c.id] ?? 'inline'}</span>
            </div>
            {c.description && (
              <p className="mt-1 text-xs text-ink-dim leading-snug">{c.description}</p>
            )}
            <p className="mt-1 text-[11px] text-ink-mute font-mono break-all">
              {c.applicable_globs.join(', ')}
            </p>
          </div>
        ))}
      </ListCard>
    </section>
  );
}
