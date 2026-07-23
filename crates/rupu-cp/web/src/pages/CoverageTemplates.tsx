// Global coverage Templates page — bundled concern templates (target-independent).
// Route: /coverage/templates
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type TemplateSummary, type TemplateDetail } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { SEVERITY_STYLE, type Severity } from '../lib/severity';

// Severity columns, most → least severe.
const SEV_COLS: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export default function CoverageTemplates() {
  const [templates, setTemplates] = useState<TemplateSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getCoverageTemplates()
      .then((d) => {
        if (!cancelled) setTemplates(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load templates');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="p-8">
      <Link
        to="/coverage"
        className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
      >
        <ArrowLeft size={14} />
        Coverage
      </Link>
      <header className="mt-3">
        <h1 className="text-2xl font-semibold text-ink">Concern Templates</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Bundled concern catalogs (OWASP, CWE, STRIDE, …).
        </p>
      </header>

      {error && <ErrorBanner className="mt-4">{error}</ErrorBanner>}
      {templates === null ? (
        <div className="mt-4 py-16 flex items-center justify-center">
          <Spinner label="Loading templates…" />
        </div>
      ) : (
        <section className="mt-6">
          <SectionHeader tone="muted" label="Templates" count={templates.length} />
          <TemplatesTable templates={templates} />
        </section>
      )}
    </div>
  );
}

/**
 * Templates as a SortableTable. Columns: Template | Version | Concerns |
 * Critical | High | Medium | Low | Info (severity-breakdown counts). Each row
 * expands (via `renderDetail`) to its nested concern list, lazily fetched.
 * Sortable on Template / Version / Concerns.
 */
function TemplatesTable({ templates }: { templates: TemplateSummary[] }) {
  const sevCol = (sev: Severity): Column<TemplateSummary> => ({
    key: sev,
    header: SEVERITY_STYLE[sev].label,
    align: 'right',
    fit: true,
    render: (t) => {
      const n = t.severity_breakdown[sev] ?? 0;
      return n > 0 ? <span className={SEVERITY_STYLE[sev].text}>{n}</span> : <span className="text-ink-mute">—</span>;
    },
  });

  const columns: Column<TemplateSummary>[] = [
    {
      key: 'name',
      header: 'Template',
      subject: true,
      sortable: true,
      sortValue: (t) => t.name,
      titleValue: (t) => t.name,
      render: (t) => (
        <div className="min-w-0">
          <span className="text-sm font-medium text-ink">{t.name}</span>
          {t.description && (
            <p className="mt-0.5 text-xs text-ink-dim leading-snug">{t.description}</p>
          )}
        </div>
      ),
    },
    {
      key: 'version',
      header: 'Version',
      fit: true,
      sortable: true,
      sortValue: (t) => t.version,
      render: (t) => <span className="text-meta text-ink-mute">v{t.version}</span>,
    },
    {
      key: 'concerns',
      header: 'Concerns',
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (t) => t.concern_count,
      render: (t) => t.concern_count,
    },
    ...SEV_COLS.map(sevCol),
  ];

  return (
    <SortableTable<TemplateSummary>
      columns={columns}
      rows={templates}
      rowKey={(t) => t.name}
      renderDetail={(t) => <TemplateConcerns name={t.name} />}
    />
  );
}

/** Nested concern list for one template — lazily fetched when the row expands. */
function TemplateConcerns({ name }: { name: string }) {
  const [detail, setDetail] = useState<TemplateDetail | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    api
      .getCoverageTemplate(name)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [name]);

  if (failed) return <ErrorBanner className="text-note">Failed to load concerns.</ErrorBanner>;
  if (!detail) return <Spinner size="sm" label="Loading concerns…" />;

  return (
    <ul className="space-y-1 border-l-2 border-border pl-3">
      {detail.concerns.map((c) => (
        <li key={c.id} className="text-xs">
          <span className="font-medium text-ink">{c.name}</span>
          <span className="ml-2 font-mono text-meta text-ink-mute">{c.id}</span>
          <span className="ml-2 text-meta text-ink-mute">{c.severity}</span>
        </li>
      ))}
    </ul>
  );
}
