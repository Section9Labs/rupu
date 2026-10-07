// Global Findings — every finding across every project, severity-ordered
// (critical → info, then newest first; the backend pre-sorts). Filtering is
// one query (`?q=`, the findings query language) evaluated server-side by
// `GET /api/findings` — the page never filters rows itself. The severity tiles
// are shortcuts that toggle a `severity:<x>` token in that query; their counts
// come from the response's severity facets. The table's Project / Target
// columns show each finding's owning project · target.

import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api, apiErrorMessage, type FindingsResponse, type FindingsSummary } from '../lib/api';
import { SEVERITY_STYLE, type Severity } from '../lib/severity';
import { FINDING_FIELDS } from '../lib/findingQuery/fields';
import { parseQuery, tokenize } from '../lib/findingQuery/grammar';
import { ExportReportButton } from '../components/findings/ExportReportButton';
import { FindingMetrics } from '../components/findings/FindingMetrics';
import { FindingsTable } from '../components/findings/FindingsTable';
import { QueryBar } from '../components/query/QueryBar';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';

const SEVS: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

function sevTone(key: string, value: string): string | null {
  return key === 'severity' && (SEVS as string[]).includes(value) ? SEVERITY_STYLE[value as Severity].pill : null;
}

/** The severity a lone, positive `severity:<x>` token selects (the active tile). */
function activeSeverity(q: string): Severity | null {
  const p = parseQuery(q, FINDING_FIELDS);
  if (!p.ok) return null;
  const sev = p.terms.filter((t) => t.key === 'severity');
  return sev.length === 1 && !sev[0].neg && sev[0].op === 'eq' && sev[0].values.length === 1
    ? (sev[0].values[0] as Severity)
    : null;
}

/** `q` with every severity token removed, then `severity:<sev>` added (null = none). */
function withSeverity(q: string, sev: Severity | null): string {
  const t = tokenize(q);
  const kept = (t.ok ? t.tokens.map((x) => x.text) : []).filter((raw) => {
    const p = parseQuery(raw, FINDING_FIELDS);
    return !(p.ok && p.terms[0]?.key === 'severity');
  });
  return [...kept, ...(sev ? [`severity:${sev}`] : [])].join(' ');
}

export default function Findings() {
  const [params, setParams] = useSearchParams();
  const q = params.get('q') ?? '';
  // Other params (e.g. the Security tab) ride along untouched.
  const setQ = (next: string) =>
    setParams(
      (prev) => {
        const p = new URLSearchParams(prev);
        if (next.trim()) p.set('q', next);
        else p.delete('q');
        return p;
      },
      { replace: true },
    );
  const localError = useMemo(() => {
    const p = parseQuery(q, FINDING_FIELDS);
    return p.ok ? null : p.error.message;
  }, [q]);

  // Each answer and error is kept with the query it answers: the page shows
  // rows, tiles and the export only for the query in the URL, never an
  // earlier one's while the next loads or after the server rejects it.
  const [answer, setAnswer] = useState<{ q: string; resp: FindingsResponse } | null>(null);
  const [failure, setFailure] = useState<{ q: string; message: string } | null>(null);
  // Facets describe the scope before `q`, so the last answer's keep serving
  // the bar's suggestions while the next query loads or after it fails.
  const [facets, setFacets] = useState<FindingsResponse['facets'] | undefined>(undefined);
  // Set once the first fetch settles, or once an invalid query has shown its
  // error: until then the page is one spinner. After it, the bar stays mounted.
  const [settled, setSettled] = useState(false);

  useEffect(() => {
    // A query that doesn't parse never reaches the server; the page shows why
    // in place of the results. That counts as settled, so fixing a shared
    // link's typo doesn't swap the bar for the full-page spinner.
    if (localError) {
      setSettled(true);
      return;
    }
    let cancelled = false;
    setFailure(null);
    (q ? api.getFindings({ q }) : api.getFindings())
      .then((resp) => {
        if (cancelled) return;
        setAnswer({ q, resp });
        setFacets(resp.facets);
        setSettled(true);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setAnswer(null);
        setFailure({ q, message: apiErrorMessage(e) });
        setSettled(true);
      });
    return () => {
      cancelled = true;
    };
  }, [q, localError]);

  const data = !localError && answer?.q === q ? answer.resp : null;
  const error = !localError && failure?.q === q ? failure.message : null;

  const tileSummary: FindingsSummary = useMemo(() => {
    const counts = Object.fromEntries((data?.facets?.severity ?? []).map((v) => [v.value, v.count]));
    const by = (s: Severity) => counts[s] ?? 0;
    return {
      total: SEVS.reduce((n, s) => n + by(s), 0),
      critical: by('critical'),
      high: by('high'),
      medium: by('medium'),
      low: by('low'),
      info: by('info'),
    };
  }, [data]);

  // Name the projects whose tag logs couldn't be read (fall back to the id).
  const unreadable = useMemo(() => {
    const ids = data?.tags_unavailable ?? [];
    return ids.map((id) => data?.findings.find((f) => f.ws_id === id)?.project || id);
  }, [data]);

  const active = activeSeverity(q);
  // A fetch for this query is in flight (the last answer was for another).
  const loading = data === null && !localError && !error;
  const noneAtAll = !q && data !== null && tileSummary.total === 0 && data.findings.length === 0;

  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Findings</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every finding raised across all registered projects. Filter with the query bar, e.g.{' '}
          <code className="font-mono text-note">severity&gt;=high tag:needs-poc</code> — press / to focus.
        </p>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {loading && !settled ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading findings…" />
        </div>
      ) : noneAtAll ? (
        <EmptyState
          title="No findings"
          hint="Run an assessment workflow to start recording findings across your projects."
        />
      ) : (
        <div className="space-y-6">
          {data && (
            <FindingMetrics
              summary={tileSummary}
              active={active}
              onSelect={(sev) => setQ(withSeverity(q, sev === active ? null : sev))}
            />
          )}

          <div className="flex flex-wrap items-start gap-2.5">
            <div className="min-w-0 flex-1">
              <QueryBar
                value={q}
                onChange={setQ}
                fields={FINDING_FIELDS}
                facets={facets}
                valueTone={sevTone}
                label="Filter findings"
              />
            </div>
            {/* The report covers exactly the rows the query leaves. */}
            {data && <ExportReportButton findings={data.findings} defaultTitle="Findings report" />}
          </div>

          {localError && (
            <p role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
              {localError}
            </p>
          )}

          {loading && (
            <div className="flex items-center gap-2 text-note text-ink-mute">
              <Spinner size="sm" label="Loading findings…" />
            </div>
          )}

          {data && unreadable.length > 0 && (
            <div
              role="status"
              className="rounded-lg bg-warn-bg px-3 py-2 text-note text-warn ring-1 ring-warn/30"
            >
              Tags for {unreadable.join(', ')} couldn't be read, so tag filters may miss their findings.
            </div>
          )}

          {data &&
            (data.findings.length === 0 && q ? (
              <EmptyState title="No matches" hint={`No findings match \`${q}\`.`} />
            ) : (
              <FindingsTable findings={data.findings} showProvenance />
            ))}
        </div>
      )}
    </div>
  );
}
