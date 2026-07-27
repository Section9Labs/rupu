// Build › Autoflows — autoflow-enabled workflow definitions.
// Shows the static list of workflows that have autoflow triggers configured.

import { useCallback, useEffect, useState } from 'react';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type AutoflowDefRow } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

const TRIGGER_CLS: Record<string, string> = {
  cron:  'bg-violet-50 text-violet-700 ring-violet-200',
  event: 'bg-sky-50 text-sky-700 ring-sky-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CLS[trigger] ?? 'bg-surface text-ink ring-border';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {trigger}
    </span>
  );
}

// Autoflows are workflows with `autoflow.enabled`, so they reuse the workflow
// detail page — keyed by file stem (`slug`), not the parsed display name.
//
// Column order follows the definition-table canonical standard
// (`docs/superpowers/plans/2026-07-24-rupu-cp-table-standardization.md`
// Task 5) applied to the fields `AutoflowDefRow` actually carries: Name
// (the one flexible/truncating `subject` column) → Scope → Trigger. There is
// no Runs/Tokens/Cost/Last-run data on this row type — those columns are not
// fabricated (see the plan's "Out of scope" note).
const DEF_COLUMNS: Column<AutoflowDefRow>[] = [
  {
    key: 'name',
    header: 'Name',
    subject: true,
    sortable: true,
    sortValue: (d) => d.name,
    titleValue: (d) => d.name,
    render: (d) => <span className="text-sm font-medium text-ink">{d.name}</span>,
  },
  {
    key: 'scope',
    header: 'Scope',
    fit: true,
    sortable: true,
    sortValue: (d) => d.scope,
    render: (d) => <ScopeChip scope={d.scope} />,
  },
  {
    key: 'trigger',
    header: 'Trigger',
    fit: true,
    sortable: true,
    sortValue: (d) => d.trigger,
    render: (d) => <TriggerChip trigger={d.trigger} />,
  },
];

export default function AutoflowsDefs() {
  const [defs, setDefs] = useState<AutoflowDefRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [visible, setVisible] = useState(STEP);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getAutoflowDefs();
      setDefs(data);
      setVisible(STEP);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load autoflow definitions');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const shown = (defs ?? []).slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < (defs?.length ?? 0),
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflows</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflows with autoflow triggers configured.</p>
        </div>
        <Button variant="secondary" onClick={() => void load()} className="gap-1.5">
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {defs === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading autoflow definitions…" />
        </div>
      ) : defs.length === 0 ? (
        <EmptyState
          icon={<Inbox size={20} />}
          title="No autoflow-enabled workflows"
          hint="Workflows with autoflow triggers configured will appear here."
        />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Autoflow Workflows" count={defs.length} />
          <SortableTable<AutoflowDefRow>
            columns={DEF_COLUMNS}
            rows={shown}
            rowKey={(d) => d.slug}
            rowHref={(d) => `/workflows/${encodeURIComponent(d.slug)}`}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
          {defs.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}
    </div>
  );
}
