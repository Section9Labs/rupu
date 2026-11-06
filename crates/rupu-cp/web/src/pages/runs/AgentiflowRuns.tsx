// Agentiflow run list — the sidebar Runs → Agentiflows destination (its own
// standalone page, NOT an Activity top-strip tab). Same shape as Runs →
// Workflows (page header + Refresh, the shared SortableTable, row click
// → detail), but deliberately simpler for v1: agentiflows are LOCAL-only, so
// there is no host scope, no per-host progressive loading and no paging — one
// `GET /api/agentiflows` (newest first, the whole list) per load, plus the
// Refresh button. A later pass can move this onto `usePerHostPagedList` once
// the endpoint takes `?host=` / `offset` / `limit`.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Plus, RefreshCw } from 'lucide-react';
import { api, apiErrorMessage, type AgentiflowRow } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { RunStatusChip } from '../../components/ui/RunStatusChip';
import { CrewChip } from '../../components/codename/CrewChip';
import { Badge } from '../../components/ui/Badge';
import { Button } from '../../components/ui/Button';
import { PageHeader } from '../../components/ui/PageHeader';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { ListSkeleton } from '../../components/lists/ListSkeleton';
import AgentiflowLauncherSheet from '../../components/agentiflow/AgentiflowLauncherSheet';
import { parseCodename } from '../../lib/codename';
import { cn } from '../../lib/cn';
import { relativeTime } from '../../lib/time';
import { ListQueryBar, NoMatches, useFilteredRows, useListQuery } from '../../components/query/ListQueryBar';
import { AGENTIFLOW_FIELDS } from '../../lib/query/lists/agentiflows';
import {
  agentiflowHref,
  agentiflowPillStatus,
  formatSpendTokens,
  formatSpendUsd,
  STOP_TONE_TEXT,
  stopReasonTone,
} from '../../lib/agentiflow';

// Column order mirrors the other run lists (WorkflowRuns / AgentRuns):
// Status first, then the subject (crew chip + name together), then the
// run's metadata, right-aligned numerics, and Started last.
const COLUMNS: Column<AgentiflowRow>[] = [
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => (
      <span className="inline-flex items-center gap-1.5">
        <RunStatusChip status={agentiflowPillStatus(r.status)} />
        {r.status === 'running' && r.runner_alive === false && (
          <Badge
            tone="amber"
            title="The coordinator process is no longer running; the orphan reaper will close this run out."
          >
            coordinator gone
          </Badge>
        )}
      </span>
    ),
  },
  {
    key: 'name',
    header: 'Agentiflow',
    subject: true,
    sortable: true,
    sortValue: (r) => r.name,
    titleValue: (r) => r.name,
    // Crew chip + name in the subject column, exactly like Workflow Runs.
    // Row navigation is `rowHref` (SortableTable link-wraps the whole row);
    // an inline <Link> here would nest an <a> inside its <a>.
    render: (r) => (
      <span className="inline-flex items-center gap-2">
        {r.codename && <CrewChip crew={parseCodename(r.codename).crew} derived={r.codename_derived} />}
        <span className="text-sm font-medium text-ink">{r.name}</span>
      </span>
    ),
  },
  {
    key: 'stop',
    header: 'Stop reason',
    fit: true,
    sortable: true,
    sortValue: (r) => r.stop_reason,
    render: (r) =>
      r.stop_reason ? (
        <span
          className={cn('block max-w-[11rem] truncate font-mono text-note', STOP_TONE_TEXT[stopReasonTone(r.stop_reason)])}
          title={r.stop_reason}
        >
          {r.stop_reason}
        </span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'rounds',
    header: 'Rounds',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.rounds,
    render: (r) => <span className="text-ink">{r.rounds}</span>,
  },
  {
    key: 'goals',
    header: 'Goals',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => (r.goals_total > 0 ? r.goals_met / r.goals_total : null),
    render: (r) => (
      <span className={cn(r.goals_total > 0 && r.goals_met === r.goals_total ? 'text-ok font-medium' : 'text-ink')}>
        {r.goals_met}/{r.goals_total}
      </span>
    ),
  },
  {
    key: 'spend',
    header: 'Spend',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.spent_usd,
    render: (r) => (
      <span className="inline-flex items-baseline justify-end gap-2">
        <span className="font-medium text-ink">{formatSpendUsd(r.spent_usd)}</span>
        <span className="text-note text-ink-mute">{formatSpendTokens(r.spent_tokens)}</span>
      </span>
    ),
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => (r.started_at ? Date.parse(r.started_at) : null),
    render: (r) => <span className="text-ink-mute">{relativeTime(r.started_at)}</span>,
  },
];

export default function AgentiflowRuns() {
  const [rows, setRows] = useState<AgentiflowRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // The newest load wins: a Refresh that lands before an earlier, slower load
  // does must not be overwritten by it.
  const seq = useRef(0);

  const load = useCallback(() => {
    const mine = ++seq.current;
    const ctl = new AbortController();
    setLoading(true);
    api
      .getAgentiflows({ signal: ctl.signal })
      .then((r) => {
        if (seq.current !== mine) return;
        setRows(r);
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
  }, []);

  useEffect(() => {
    const abort = load();
    return () => {
      seq.current++; // drop any in-flight result on unmount
      abort();
    };
  }, [load]);

  const [launcherOpen, setLauncherOpen] = useState(false);
  const newButton = (
    <Button variant="solid" onClick={() => setLauncherOpen(true)} className="gap-1.5">
      <Plus size={14} />
      New agentiflow
    </Button>
  );

  const [q, setQ] = useListQuery();
  const loaded = useMemo(() => rows ?? [], [rows]);
  const filtered = useFilteredRows(loaded, q, AGENTIFLOW_FIELDS);

  return (
    <div className="p-8">
      <PageHeader
        className="mb-6"
        title="Agentiflow Runs"
        subtitle="Goal-directed agent fleets run from this machine."
        actions={
          <>
            <Button variant="secondary" onClick={() => load()} className="gap-1.5">
              <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
              Refresh
            </Button>
            {newButton}
          </>
        }
      />

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {rows === null && loading ? (
        <ListSkeleton label="Loading agentiflows…" />
      ) : rows === null ? null : rows.length === 0 ? (
        <EmptyState
          title="No agentiflow runs yet"
          hint="Start a goal-directed fleet from one of your agentiflow definitions."
          action={newButton}
        />
      ) : (
        <>
          <ListQueryBar
            value={q}
            onChange={setQ}
            fields={AGENTIFLOW_FIELDS}
            facets={filtered.facets}
            total={loaded.length}
            shown={filtered.rows.length}
            error={filtered.error}
            label="Filter agentiflows"
            placeholder="Filter agentiflows… e.g. status:running stop:budget_exhausted since:7d"
          />
          {filtered.rows.length === 0 ? (
            <NoMatches noun="agentiflows" />
          ) : (
            <SortableTable<AgentiflowRow>
              columns={COLUMNS}
              rows={filtered.rows}
              rowKey={(r) => r.id}
              rowHref={(r) => agentiflowHref(r.id)}
              initialSort={{ key: 'started', dir: 'desc' }}
            />
          )}
        </>
      )}

      {launcherOpen && <AgentiflowLauncherSheet onClose={() => setLauncherOpen(false)} />}
    </div>
  );
}
