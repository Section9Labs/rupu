// Children panel — every child a run launched (`dispatch` / the legacy
// dispatch tools, spec W7 §3.5), read from `GET /api/runs/:id/children`: the
// run's own children, its steps' agent runs' children, and theirs, nested
// under the run that launched them. Local runs only — the endpoint has no
// `?host=`.

import { useEffect, useRef, useState } from 'react';
import { api, apiErrorMessage, type ChildKind, type RunChild, type RunStatusStr } from '../../lib/api';
import SortableTable, { type Column } from '../lists/SortableTable';
import { AgentName } from '../codename/AgentName';
import { StatusPill } from '../StatusPill';
import { Badge, type BadgeTone } from '../ui/Badge';
import { ErrorBanner } from '../ui/ErrorBanner';
import { formatTokens } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { relativeTime } from '../../lib/time';
import { shortId } from '../../lib/shortId';

const POLL_MS = 3000;

interface ChildRow {
  child: RunChild;
  /** Nesting under the panel's roots (the run itself and its step agent
   *  runs), not the ledger's `depth`, which counts from a different base for
   *  a workflow step's children and for legacy rows. */
  level: number;
}

/** Tree order: each parent's children right after it, in launch order. */
export function childTree(children: readonly RunChild[]): ChildRow[] {
  const byParent = new Map<string, RunChild[]>();
  for (const c of children) {
    const siblings = byParent.get(c.parent_run_id) ?? [];
    siblings.push(c);
    byParent.set(c.parent_run_id, siblings);
  }
  const ids = new Set(children.map((c) => c.run_id));
  const rows: ChildRow[] = [];
  const seen = new Set<string>();
  const walk = (c: RunChild, level: number) => {
    if (seen.has(c.run_id)) return;
    seen.add(c.run_id);
    rows.push({ child: c, level });
    for (const k of byParent.get(c.run_id) ?? []) walk(k, level + 1);
  };
  for (const c of children) if (!ids.has(c.parent_run_id)) walk(c, 0);
  return rows;
}

function isActive(c: RunChild): boolean {
  return c.status === 'pending' || c.status === 'running';
}

const KIND_TONE: Record<ChildKind, BadgeTone> = {
  subagent: 'neutral',
  agent: 'sky',
  workflow: 'violet',
};

function ChildStatus({ child }: { child: RunChild }) {
  if (child.status === 'abandoned' || child.status === 'interrupted') {
    return (
      <Badge
        tone={child.status === 'abandoned' ? 'amber' : 'sky'}
        ring
        className="uppercase tracking-wide opacity-80"
      >
        {child.status}
      </Badge>
    );
  }
  const status: RunStatusStr =
    child.status === 'done' ? (child.success === false ? 'failed' : 'completed') : child.status;
  return <StatusPill status={status} size="xs" />;
}

function childHref(c: RunChild): string | undefined {
  if (c.kind === 'workflow') return `/runs/${encodeURIComponent(c.run_id)}`;
  if (!c.transcript_path) return undefined;
  return `/transcript?path=${encodeURIComponent(c.transcript_path)}&live=${isActive(c) ? 1 : 0}`;
}

const COLUMNS: Column<ChildRow>[] = [
  {
    key: 'codename',
    header: 'Codename',
    fit: true,
    render: ({ child, level }) => (
      <span className="inline-flex items-center gap-1.5" style={{ paddingLeft: level * 16 }}>
        {level > 0 && <span className="text-ink-mute">↳</span>}
        {child.codename ? (
          <AgentName codename={child.codename} agent={child.name} />
        ) : (
          <span className="font-mono text-note text-ink-mute">{shortId(child.run_id)}</span>
        )}
        {child.legacy && (
          <span
            className="rounded px-1 text-meta text-ink-mute ring-1 ring-border"
            title="Recorded before the children ledger (read from the run's sub/ directory)"
          >
            legacy
          </span>
        )}
      </span>
    ),
  },
  {
    key: 'kind',
    header: 'Kind',
    fit: true,
    render: ({ child }) => (
      <Badge tone={KIND_TONE[child.kind] ?? 'neutral'} ring>
        {child.kind}
      </Badge>
    ),
  },
  {
    key: 'name',
    header: 'Name',
    subject: true,
    titleValue: ({ child }) => child.error ?? child.name,
    render: ({ child }) => (
      <span className="block truncate">
        <span className="text-sm text-ink">{child.name || '—'}</span>
        {child.error && <span className="ml-2 text-note text-err">{child.error}</span>}
      </span>
    ),
  },
  {
    key: 'status',
    header: 'Status',
    fit: true,
    render: ({ child }) => <ChildStatus child={child} />,
  },
  {
    key: 'tokens',
    header: 'Tokens in/out',
    fit: true,
    align: 'right',
    render: ({ child }) =>
      child.tokens ? (
        <span className="text-ink-dim">
          {formatTokens(child.tokens.input)} / {formatTokens(child.tokens.output)}
        </span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'duration',
    header: 'Duration',
    fit: true,
    align: 'right',
    render: ({ child }) => <span className="text-ink-dim">{formatDuration(child.duration_ms)}</span>,
  },
  {
    key: 'started',
    header: 'Started',
    fit: true,
    align: 'right',
    render: ({ child }) => <span className="text-ink-mute">{relativeTime(child.started_at)}</span>,
  },
];

export default function ChildrenPanel({
  runId,
  onCount,
}: {
  runId: string;
  /** Called with the number of children after every load. */
  onCount?: (n: number) => void;
}) {
  const [children, setChildren] = useState<RunChild[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const onCountRef = useRef(onCount);
  onCountRef.current = onCount;

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    setChildren(null);
    setError(null);
    const load = async () => {
      try {
        const next = await api.getRunChildren(runId, controller.signal);
        if (controller.signal.aborted) return;
        setChildren(next);
        setError(null);
        onCountRef.current?.(next.length);
        if (next.some(isActive)) timer = setTimeout(load, POLL_MS);
      } catch (e) {
        if (controller.signal.aborted) return;
        setError(apiErrorMessage(e));
      }
    };
    void load();
    return () => {
      controller.abort();
      if (timer) clearTimeout(timer);
    };
  }, [runId]);

  if (error && children === null) return <ErrorBanner>{error}</ErrorBanner>;
  if (children === null) return <p className="text-sm text-ink-dim">Loading children…</p>;
  if (children.length === 0) {
    return <p className="text-sm text-ink-mute">No children — this run launched none.</p>;
  }
  return (
    <div data-testid="children-panel" className="space-y-2">
      {error && <ErrorBanner>{error}</ErrorBanner>}
      <SortableTable<ChildRow>
        columns={COLUMNS}
        rows={childTree(children)}
        rowKey={({ child }) => `${child.parent_run_id}/${child.run_id}`}
        rowHref={({ child }) => childHref(child)}
      />
    </div>
  );
}
