// Catalog tab — the effective concern catalog snapshot, as collapsed rows.
import { useEffect, useMemo, useState } from 'react';
import { api, type FlatCatalog } from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import { Spinner } from '../ui/Spinner';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import ExpandCollapse from './ExpandCollapse';
import { ListQueryBar, NoMatches, useFilteredRows, useListQuery } from '../query/ListQueryBar';
import { CATALOG_FIELDS, type CatalogRow } from '../../lib/query/lists/concerns';

export default function CoverageCatalogTab({ target, wsId }: { target: string; wsId?: string }) {
  const [cat, setCat] = useState<FlatCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());

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

  const [q, setQ] = useListQuery();
  const all = useMemo<CatalogRow[]>(
    () => (cat ? cat.concerns.map((c) => ({ ...c, source: cat.sources[c.id] ?? 'inline' })) : []),
    [cat],
  );
  const { rows: concerns, error: queryError, facets } = useFilteredRows(all, q, CATALOG_FIELDS);

  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  if (error) return <p className="mt-4 text-sm text-err">{error}</p>;
  if (!cat) return <div className="mt-4"><Spinner label="Loading…" /></div>;
  if (cat.concerns.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No catalog snapshot for this target.</p>;

  return (
    <section className="mt-6">
      <ListQueryBar
        value={q}
        onChange={setQ}
        fields={CATALOG_FIELDS}
        facets={facets}
        total={all.length}
        shown={concerns.length}
        error={queryError}
        label="Filter concerns"
        placeholder="Filter concerns… e.g. severity>=high tag:owasp-a03 source:inline"
      />
      <SectionHeader
        tone="muted"
        label="Catalog concerns"
        count={concerns.length}
        trailing={
          <ExpandCollapse
            onExpandAll={() => setOpen(new Set(concerns.map((c) => c.id)))}
            onCollapseAll={() => setOpen(new Set())}
          />
        }
      />
      {concerns.length === 0 ? (
        <NoMatches noun="concerns" />
      ) : (
      <ListCard>
        {concerns.map((c) => (
          <CollapsibleRow
            key={c.id}
            open={open.has(c.id)}
            onToggle={() => toggle(c.id)}
            header={
              <span className="flex items-center gap-2 flex-wrap">
                <span className="text-sm font-medium text-ink">{c.name}</span>
                <span className="text-note font-mono text-ink-mute">{c.id}</span>
                <SeverityChip severity={c.severity} />
                <span className="text-meta text-ink-mute">{c.source}</span>
              </span>
            }
          >
            {c.description && <p className="text-xs text-ink-dim leading-snug">{c.description}</p>}
            <p className="mt-1 text-note text-ink-mute font-mono break-all">
              globs: {c.applicable_globs.join(', ')}
            </p>
            <p className="mt-1 text-note text-ink-mute">min strength: {c.min_strength}</p>
            {c.tags.length > 0 && (
              <p className="mt-1 text-note text-ink-mute">tags: {c.tags.join(', ')}</p>
            )}
            {c.references.length > 0 && (
              <ul className="mt-1 space-y-0.5">
                {c.references.map((ref) => (
                  <li key={ref} className="text-note text-ink-mute break-all">
                    {ref}
                  </li>
                ))}
              </ul>
            )}
          </CollapsibleRow>
        ))}
      </ListCard>
      )}
    </section>
  );
}
