// Settings → Models: the discovered model-limits catalog plus a manual
// refetch (spec docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md §8.3).
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, apiErrorMessage, type CatalogModel, type CatalogProvider } from '../../lib/api';
import { relativeTime } from '../../lib/time';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyTabState } from '../ConfigEditor';

const fmt = (n: number | null) => (n == null ? '—' : n.toLocaleString('en-US'));

const COLUMNS: Column<CatalogModel>[] = [
  {
    key: 'id',
    header: 'Model',
    subject: true,
    sortable: true,
    sortValue: (m) => m.id,
    // The subject column truncates; the tooltip is how a long id stays readable.
    titleValue: (m) => m.id,
    render: (m) => <span className="font-mono">{m.id}</span>,
  },
  { key: 'input', header: 'Input limit', align: 'right', fit: true, sortable: true, sortValue: (m) => m.input_tokens, render: (m) => fmt(m.input_tokens) },
  { key: 'output', header: 'Output cap', align: 'right', fit: true, sortable: true, sortValue: (m) => m.output_tokens, render: (m) => fmt(m.output_tokens) },
  { key: 'source', header: 'Source', fit: true, render: (m) => m.source },
];

export function ModelsTab() {
  const [catalog, setCatalog] = useState<CatalogProvider[] | null>(null);
  // A catalog LOAD failure (first load, or a reload after a refetch). Retry re-runs it.
  const [loadError, setLoadError] = useState<string | null>(null);
  // A thrown Refetch all (the refresh itself failed): a load Retry would not
  // redo it, so it gets a plain banner with no Retry.
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});

  // Latest-wins: every load takes a sequence number, and only the newest one
  // may write state, so a slow earlier response can't clobber a later one.
  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    const seq = ++loadSeq.current;
    try {
      const next = await api.getModelCatalog();
      if (seq !== loadSeq.current) return;
      setCatalog(next);
      setLoadError(null);
    } catch (e) {
      if (seq !== loadSeq.current) return;
      if (e instanceof ApiError && e.status === 501) setUnavailable(true);
      else setLoadError(apiErrorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const retry = async () => {
    setRetrying(true);
    try {
      await load();
    } finally {
      setRetrying(false);
    }
  };

  const refetch = async (provider?: string) => {
    setBusy(provider ?? '*');
    // A failed Refetch all's banner is about that attempt; starting another
    // refetch retires it (nothing else clears it — it isn't a load error).
    setRefreshError(null);
    // The inline provider errors this refetch produced. They are applied in the
    // `finally`, in the same React batch that clears `busy`, so an alert is
    // never inserted into a section that is still aria-busy="true" (assistive
    // tech may not announce an alert added to a busy region).
    let applyErrors: ((prev: Record<string, string>) => Record<string, string>) | null = null;
    try {
      const outcomes = await api.refreshModels(provider);
      applyErrors = (prev) => {
        const next = { ...prev };
        for (const o of outcomes) {
          if (o.ok) delete next[o.provider];
          else next[o.provider] = o.error ?? 'refresh failed';
        }
        return next;
      };
      await load();
    } catch (e) {
      // A single-provider refetch that throws (400 unknown provider, 500,
      // network) lands inline under that provider (spec §8.3); only a thrown
      // Refetch all falls back to the page-level banner.
      if (provider) {
        const message = apiErrorMessage(e);
        applyErrors = (prev) => ({ ...prev, [provider]: message });
      } else setRefreshError(apiErrorMessage(e));
    } finally {
      if (applyErrors) setErrors(applyErrors);
      setBusy(null);
    }
  };

  if (unavailable) return <EmptyTabState text="The model catalog requires `rupu cp serve`." />;
  // The tab is single-flight: a catalog load (Retry) and a refresh (Refetch)
  // never overlap, so each Retry/Refetch control is blocked while the other
  // kind of work, or its own, is in flight.
  const working = retrying || busy !== null;

  // The load-failure banner, with a Retry that is blocked while anything is in flight.
  const loadErrorBanner = loadError ? (
    <ErrorBanner>
      <div className="flex items-center justify-between gap-3">
        <span>{loadError}</span>
        <Button variant="secondary" size="sm" disabled={working} aria-busy={retrying} onClick={() => void retry()}>
          Retry
        </Button>
      </div>
    </ErrorBanner>
  ) : null;

  if (!catalog) {
    // First load failed (non-501): nothing to show yet, so give the operator a
    // way back in without leaving the Settings page.
    return loadErrorBanner ?? <Spinner label="Loading models…" />;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <p className="text-sm">
          Limits come from each provider's model list, cached for an hour. Agent frontmatter and
          <code className="mx-1">[[providers.&lt;name&gt;.models]]</code>
          override them.
        </p>
        <Button variant="secondary" size="sm" disabled={working} aria-busy={busy === '*'} onClick={() => void refetch()}>
          Refetch all
        </Button>
      </div>
      {loadErrorBanner}
      {refreshError && <ErrorBanner>{refreshError}</ErrorBanner>}
      {catalog.map((p) => {
        // A provider is refreshing during its own Refetch and during Refetch all.
        const refreshing = busy === '*' || busy === p.provider;
        return (
          <section key={p.provider} className="space-y-2" aria-busy={refreshing}>
            <div className="flex items-center gap-3">
              <h3 className="font-medium">{p.provider}</h3>
              <span className="text-sm">{p.fetched_at ? `fetched ${relativeTime(p.fetched_at)}` : 'never fetched'}</span>
              {p.stale && <Badge tone="amber">stale</Badge>}
              <div className="ml-auto">
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={working}
                  onClick={() => void refetch(p.provider)}
                  // The explicit label keeps the name stable ("Refetch <provider>")
                  // while the visible text flips to "Refetching…"; aria-busy is what
                  // tells assistive tech the refresh is in flight.
                  aria-label={`Refetch ${p.provider}`}
                  aria-busy={refreshing}
                >
                  {busy === p.provider ? 'Refetching…' : 'Refetch'}
                </Button>
              </div>
            </div>
            {errors[p.provider] && <ErrorBanner>{`${p.provider}: ${errors[p.provider]}`}</ErrorBanner>}
            {p.models.length > 0 ? (
              <SortableTable<CatalogModel>
                columns={COLUMNS}
                rows={p.models}
                rowKey={(m) => m.id}
                initialSort={{ key: 'id', dir: 'asc' }}
              />
            ) : (
              <p className="text-sm">No models listed.</p>
            )}
          </section>
        );
      })}
    </div>
  );
}
