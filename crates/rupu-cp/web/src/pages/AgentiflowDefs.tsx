// Library → Agentiflows (operator MCP Plan 3b, UI-2, mock §4): the
// agentiflow definitions `rupu agentiflow run` loads, global and per project,
// built like Library → Workflows — a row per definition with its layer, lead
// and engagement profiles, a row Launch, and New agentiflow. A row opens the
// definition page. Flows are local-only, so there is no host scope.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { Orbit, Plus } from 'lucide-react';
import { apiErrorMessage } from '../lib/api';
import { agentiflowsApi, type AgentiflowDefRow } from '../lib/api3b/agentiflows';
import { defHref, leadAndProfiles } from '../lib/agentiflowDefs';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { ListSkeleton } from '../components/lists/ListSkeleton';
import AgentiflowLauncherSheet, { type LaunchInitial } from '../components/agentiflow/AgentiflowLauncherSheet';
import NewAgentiflowDefModal from '../components/agentiflow/NewAgentiflowDefModal';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { PageHeader } from '../components/ui/PageHeader';
import { StatusChip } from '../components/ui/StatusChip';

export function DefLayer({ def }: { def: Pick<AgentiflowDefRow, 'scope' | 'scope_kind'> }) {
  if (def.scope_kind === 'global') return <StatusChip tone="origin-global">global</StatusChip>;
  return (
    <span className="inline-flex items-center gap-1.5">
      <StatusChip tone="origin-project">project</StatusChip>
      <span className="text-note text-ink-dim">{def.scope}</span>
    </span>
  );
}

function columns(onLaunch: (d: LaunchInitial) => void): Column<AgentiflowDefRow>[] {
  return [
    {
      key: 'name',
      header: 'Definition',
      subject: true,
      sortable: true,
      sortValue: (d) => d.name,
      titleValue: (d) => d.name,
      render: (d) => (
        <div className="min-w-0">
          <span className="font-mono text-sm font-medium text-ink">{d.name}</span>
          {d.error ? (
            <p className="mt-0.5 truncate font-mono text-note text-err" title={d.error}>
              {d.error}
            </p>
          ) : (
            d.description && <p className="mt-0.5 truncate text-note text-ink-mute">{d.description}</p>
          )}
        </div>
      ),
    },
    {
      key: 'layer',
      header: 'Layer',
      fit: true,
      sortable: true,
      sortValue: (d) => `${d.scope_kind}:${d.scope}`,
      render: (d) => <DefLayer def={d} />,
    },
    {
      key: 'lead',
      header: 'Lead · profiles',
      sortable: true,
      sortValue: (d) => d.lead,
      render: (d) => (d.error ? <span className="text-ink-mute">—</span> : <span className="text-ui text-ink-dim">{leadAndProfiles(d.lead, d.engagement_profiles)}</span>),
    },
    {
      key: 'action',
      header: '',
      align: 'right',
      fit: true,
      interactive: true,
      render: (d) =>
        d.error ? null : (
          <div
            onClick={(e) => {
              // The row is a link; this click acts instead of navigating.
              e.preventDefault();
              e.stopPropagation();
            }}
          >
            <Button variant="ring" onClick={() => onLaunch(d)} aria-label={`Launch ${d.name}`}>
              Launch
            </Button>
          </div>
        ),
    },
  ];
}

export default function AgentiflowDefs() {
  const [rows, setRows] = useState<AgentiflowDefRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [launching, setLaunching] = useState<LaunchInitial | null>(null);
  const [createOpen, setCreateOpen] = useState(false);

  const load = useCallback(() => {
    const ctl = new AbortController();
    agentiflowsApi
      .listDefs({ signal: ctl.signal })
      .then((r) => {
        setRows(r);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!ctl.signal.aborted) setError(apiErrorMessage(e));
      });
    return () => ctl.abort();
  }, []);

  useEffect(() => load(), [load]);

  const cols = useMemo(() => columns(setLaunching), []);

  const newButton = (
    <Button variant="solid" onClick={() => setCreateOpen(true)} className="gap-1.5">
      <Plus size={14} />
      New agentiflow
    </Button>
  );

  return (
    <div className="p-8">
      <PageHeader
        className="mb-6"
        title="Agentiflows"
        subtitle="Goal-directed fleet definitions — open one to edit its YAML, or launch it."
        actions={newButton}
      />

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {rows === null ? (
        error ? null : <ListSkeleton label="Loading agentiflow definitions…" />
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<Orbit size={20} />}
          title="No agentiflow definitions yet"
          hint="A definition names a lead agent, its goals, a budget and the agents it may dispatch."
        />
      ) : (
        <SortableTable<AgentiflowDefRow>
          columns={cols}
          rows={rows}
          rowKey={(d) => `${d.scope_kind}:${d.scope_id ?? ''}:${d.name}`}
          rowHref={(d) => defHref(d)}
          initialSort={{ key: 'name', dir: 'asc' }}
        />
      )}

      {launching && <AgentiflowLauncherSheet initial={launching} onClose={() => setLaunching(null)} />}
      {createOpen && <NewAgentiflowDefModal onClose={() => setCreateOpen(false)} />}
    </div>
  );
}
