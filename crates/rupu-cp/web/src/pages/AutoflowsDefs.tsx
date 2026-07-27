// Build › Autoflows — autoflow-enabled workflow definitions.
// Shows the static list of workflows that have autoflow triggers configured.

import { useCallback, useEffect, useState } from 'react';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, scopeSelectorFor, type AutoflowDefRow } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const ENABLED_CLS = 'bg-ok-bg text-ok ring-ok/30';
const DISABLED_CLS = 'bg-surface text-ink-mute ring-border';

/** Exported so `ProjectDefinitions.tsx`'s autoflow sub-table can render the
 *  same read-only enabled/disabled chip without a toggle (its rows are
 *  project-scoped by construction — see that file's `AUTOFLOW_COLUMNS`). */
export function EnabledChip({ enabled }: { enabled: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5',
        enabled ? ENABLED_CLS : DISABLED_CLS,
      )}
    >
      {enabled ? 'Enabled' : 'Disabled'}
    </span>
  );
}

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

// Autoflows are workflows with an `autoflow:` block, so they reuse the
// workflow detail page — keyed by file stem (`slug`), not the parsed display
// name. `scan_autoflow_defs` (rupu-cp) now lists BOTH enabled and disabled
// defs (previously disabled ones were silently dropped, which made a
// Enable/Disable toggle incoherent), so the Enabled column + toggle button
// below are meaningful in both directions.
//
// Column order follows the definition-table canonical standard
// (`docs/superpowers/plans/2026-07-24-rupu-cp-table-standardization.md`
// Task 5) applied to the fields `AutoflowDefRow` actually carries: Name
// (the one flexible/truncating `subject` column) → Scope → Trigger → Enabled,
// then the trailing action column (the Enable/Disable toggle). There is no
// Runs/Tokens/Cost/Last-run data on this row type — those columns are not
// fabricated (see the plan's "Out of scope" note).
function defColumns(onToggle: (d: AutoflowDefRow) => void): Column<AutoflowDefRow>[] {
  return [
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
    {
      key: 'enabled',
      header: 'Enabled',
      fit: true,
      sortable: true,
      sortValue: (d) => (d.enabled ? 1 : 0),
      render: (d) => <EnabledChip enabled={d.enabled} />,
    },
    {
      key: 'action',
      header: '',
      align: 'right',
      fit: true,
      // Its own real button (Enable/Disable) — keep it independently
      // focusable/announced (I7) rather than swallowed by the row link.
      //
      // Renders for every row regardless of scope: `POST
      // /api/autoflows/:name/enable|disable` resolves `:name` via
      // `resolve_workflow_path`, which now uses the SAME representative-
      // worktree selection (`distinct_repo_workspaces`) this list itself
      // uses to build its rows — so the toggle always flips the exact file
      // the row displays, never a different worktree's copy or the global
      // layer. See `set_autoflow_enabled`'s doc comment in
      // `rupu-cp/src/api/autoflows.rs`.
      interactive: true,
      render: (d) => (
        <div
          className="flex items-center justify-end"
          onClick={(e) => {
            // The row is link-wrapped (rowHref) — without both of these,
            // this click either soft- or hard-navigates to the workflow
            // instead of toggling it (stopPropagation alone does not block
            // the enclosing <a>'s native default navigation action).
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          {d.enabled ? (
            <Button
              variant="ring-danger"
              onClick={() => onToggle(d)}
              aria-label={`Disable ${d.name}`}
            >
              Disable
            </Button>
          ) : (
            <Button variant="ring" onClick={() => onToggle(d)} aria-label={`Enable ${d.name}`}>
              Enable
            </Button>
          )}
        </div>
      ),
    },
  ];
}

export default function AutoflowsDefs() {
  const [defs, setDefs] = useState<AutoflowDefRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [visible, setVisible] = useState(STEP);
  // Toggle failures — kept separate from the list-fetch error above, but
  // shown in the same banner (mirrors Sessions.tsx / WorkflowRuns.tsx's
  // `actionError`).
  const [actionError, setActionError] = useState<string | null>(null);

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

  // Row action: flip `autoflow.enabled` — keyed by `slug` (file stem), the
  // same identifier `resolve_workflow_path` resolves `POST
  // /api/autoflows/:name/enable|disable` against server-side.
  // `scopeSelectorFor(d)` threads the row's exact `scope_kind`/`scope_id` so
  // the toggle targets THIS row's file even when another repo defines the
  // same name.
  async function handleToggle(d: AutoflowDefRow) {
    try {
      await api.setAutoflowEnabled(d.slug, !d.enabled, scopeSelectorFor(d));
      setActionError(null);
      await load();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Failed to update autoflow');
    }
  }

  const columns = defColumns(handleToggle);

  const shown = (defs ?? []).slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < (defs?.length ?? 0),
    loadMore: () => setVisible((v) => v + STEP),
  });
  const bannerError = error ?? actionError;

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

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {defs === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading autoflow definitions…" />
        </div>
      ) : defs.length === 0 ? (
        <EmptyState
          icon={<Inbox size={20} />}
          title="No autoflow workflows"
          hint="Workflows with autoflow triggers configured (enabled or disabled) will appear here."
        />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Autoflow Workflows" count={defs.length} />
          <SortableTable<AutoflowDefRow>
            columns={columns}
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
