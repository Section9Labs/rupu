// Skills library — the installed skills (`GET /api/skills`), the global layer
// plus a chosen project's. Read-only: install and update are `rupu skill`
// operations. Each row links to /skills/:layer/:name.

import { useEffect, useState } from 'react';
import { AlertTriangle, BookOpen } from 'lucide-react';
import { api, apiErrorMessage, type ProjectRow, type SkillRow } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { Badge } from '../components/ui/Badge';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Select } from '../components/ui/Select';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';

export function skillHref(s: Pick<SkillRow, 'layer' | 'name'>, wsId?: string | null): string {
  const path = `/skills/${s.layer}/${encodeURIComponent(s.name)}`;
  return s.layer === 'project' && wsId ? `${path}?ws_id=${encodeURIComponent(wsId)}` : path;
}

export default function Skills() {
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [wsId, setWsId] = useState<string | null>(null);
  const [skills, setSkills] = useState<SkillRow[] | null>(null);
  const [layerWarnings, setLayerWarnings] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getProjects()
      .then((rows) => {
        if (!cancelled) setProjects(rows);
      })
      .catch(() => {
        // The global layer still lists without the project choices.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setSkills(null);
    setLayerWarnings([]);
    api
      .getSkillsListing(wsId ?? undefined)
      .then((listing) => {
        if (cancelled) return;
        setSkills(listing.skills);
        setLayerWarnings(listing.warnings);
        setError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setSkills([]);
        setError(apiErrorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const rows = skills ?? [];

  return (
    <div className="p-8">
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Skills</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Skills installed on this control plane — globally, and in the selected project.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-ui text-ink-dim">
          <span aria-hidden="true">Project</span>
          <Select
            aria-label="Project"
            value={wsId ?? ''}
            onChange={(e) => setWsId(e.target.value === '' ? null : e.target.value)}
          >
            <option value="">global only</option>
            {projects.map((p) => (
              <option key={p.ws_id} value={p.ws_id}>
                {p.name}
              </option>
            ))}
          </Select>
        </div>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {layerWarnings.length > 0 && (
        <div
          role="status"
          className="mb-4 flex items-start gap-2 rounded-lg border border-warn/30 bg-warn-bg px-4 py-2 text-sm text-warn"
        >
          <AlertTriangle size={16} className="mt-0.5 shrink-0" aria-hidden />
          <div className="min-w-0">
            <p className="font-medium">Some skills could not be listed</p>
            <ul className="mt-1 space-y-1 font-mono text-ui">
              {layerWarnings.map((w, i) => (
                <li key={i} className="break-words">
                  {w}
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}

      {skills === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading skills…" />
        </div>
      ) : rows.length === 0 ? (
        error ? null : (
          <EmptyState
            icon={<BookOpen size={20} />}
            title="No skills installed"
            hint={
              <>
                Install skills with <span className="font-mono">rupu skill install &lt;source&gt;</span>
              </>
            }
          />
        )
      ) : (
        <section>
          <SectionHeader tone="muted" label="Skills" count={rows.length} />
          <SortableTable<SkillRow>
            columns={skillColumns}
            rows={rows}
            rowKey={(s) => `${s.layer}/${s.name}`}
            rowHref={(s) => skillHref(s, wsId)}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
        </section>
      )}
    </div>
  );
}

const skillColumns: Column<SkillRow>[] = [
  {
    key: 'name',
    header: 'Name',
    subject: true,
    sortable: true,
    sortValue: (s) => s.name,
    titleValue: (s) => s.name,
    render: (s) => (
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-sm font-medium text-ink">{s.name}</span>
        {s.both_layers && (
          <Badge ring title="A skill of the same name is installed in the other layer">
            both layers
          </Badge>
        )}
      </div>
    ),
  },
  {
    key: 'layer',
    header: 'Layer',
    fit: true,
    sortable: true,
    sortValue: (s) => s.layer,
    render: (s) => <ScopeChip scope={s.layer} />,
  },
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (s) => s.status,
    render: (s) =>
      s.status === 'invalid' ? (
        <Badge tone="red" title={s.error ?? undefined}>
          invalid
        </Badge>
      ) : s.warnings.length > 0 ? (
        <Badge tone="amber" title={s.warnings.join('\n')}>
          ok · {s.warnings.length} warning{s.warnings.length === 1 ? '' : 's'}
        </Badge>
      ) : (
        <Badge tone="green">ok</Badge>
      ),
  },
  {
    key: 'source',
    header: 'Source',
    fit: true,
    sortable: true,
    sortValue: (s) => s.source,
    render: (s) =>
      s.source ? (
        <span className="font-mono text-ui text-ink-dim">{s.source}</span>
      ) : (
        <span className="text-ink-mute">-</span>
      ),
  },
  {
    key: 'description',
    header: 'Description',
    render: (s) =>
      s.description ? (
        <span className="block max-w-md truncate text-ui leading-snug text-ink-dim" title={s.description}>
          {s.description}
        </span>
      ) : (
        <span className="text-ink-mute">-</span>
      ),
  },
];
