// Global coverage Templates page — bundled concern templates (target-independent).
// Route: /coverage/templates
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type TemplateSummary, type TemplateDetail } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import { ListCard } from '../components/lists/ListCard';

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

      {error && <p className="mt-4 text-sm text-red-700">{error}</p>}
      {templates === null ? (
        <p className="mt-4 text-sm text-ink-dim">Loading…</p>
      ) : (
        <section className="mt-6">
          <SectionHeader tone="muted" label="Templates" count={templates.length} />
          <ListCard>
            {templates.map((t) => (
              <TemplateRow key={t.name} t={t} />
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function TemplateRow({ t }: { t: TemplateSummary }) {
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<TemplateDetail | null>(null);

  function toggle() {
    const next = !open;
    setOpen(next);
    if (next && !detail) {
      api
        .getCoverageTemplate(t.name)
        .then(setDetail)
        .catch(() => setDetail(null));
    }
  }

  return (
    <div className="px-4 py-3">
      <button onClick={toggle} className="w-full text-left">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink">{t.name}</span>
          <span className="text-[10px] text-ink-mute">v{t.version}</span>
          <span className="text-[11px] text-ink-mute tabular-nums">{t.concern_count} concerns</span>
          {Object.entries(t.severity_breakdown).map(([sev, n]) => (
            <span key={sev} className="text-[10px] text-ink-mute">
              {sev}:{n}
            </span>
          ))}
        </div>
        {t.description && <p className="mt-1 text-xs text-ink-dim leading-snug">{t.description}</p>}
      </button>
      {open && detail && (
        <ul className="mt-2 space-y-1 border-l-2 border-border pl-3">
          {detail.concerns.map((c) => (
            <li key={c.id} className="text-xs">
              <span className="font-medium text-ink">{c.name}</span>
              <span className="ml-2 font-mono text-[10px] text-ink-mute">{c.id}</span>
              <span className="ml-2 text-[10px] text-ink-mute">{c.severity}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
