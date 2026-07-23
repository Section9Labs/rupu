// Global Findings — every finding across every project, severity-ordered
// (critical → info, then newest first; the backend pre-sorts). A clickable
// metric strip filters the list to a single severity; the table's Project /
// Target columns show each finding's owning project · target.

import { useEffect, useMemo, useState } from 'react';
import { api, normFindingSeverity, type FindingOut, type FindingsSummary } from '../lib/api';
import { type Severity } from '../lib/severity';
import { FindingMetrics } from '../components/findings/FindingMetrics';
import { FindingsTable } from '../components/findings/FindingsTable';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';

const EMPTY_SUMMARY: FindingsSummary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 };

export default function Findings() {
  const [findings, setFindings] = useState<FindingOut[] | null>(null);
  const [summary, setSummary] = useState<FindingsSummary>(EMPTY_SUMMARY);
  const [error, setError] = useState<string | null>(null);
  const [activeSev, setActiveSev] = useState<Severity | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getFindings()
      .then((data) => {
        if (cancelled) return;
        setFindings(data.findings);
        setSummary(data.summary);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load findings');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const all = findings ?? [];

  // Filter to the active severity tile (backend order is preserved). Total /
  // re-clicking the active tile clears the filter (activeSev === null).
  const rows = useMemo(
    () => (activeSev ? all.filter((f) => normFindingSeverity(f.severity) === activeSev) : all),
    [all, activeSev],
  );

  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Findings</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every finding raised across all registered projects, ordered by severity. Click a metric
          tile to filter the list.
        </p>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {findings === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading findings…" />
        </div>
      ) : all.length === 0 ? (
        <EmptyState
          title="No findings"
          hint="Run an assessment workflow to start recording findings across your projects."
        />
      ) : (
        <div className="space-y-6">
          <FindingMetrics summary={summary} active={activeSev} onSelect={setActiveSev} />

          {rows.length === 0 ? (
            <EmptyState title="No matches" hint={`No ${activeSev} findings.`} />
          ) : (
            <FindingsTable findings={rows} showProvenance />
          )}
        </div>
      )}
    </div>
  );
}
