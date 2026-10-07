// Global Findings — every finding across every project, severity-ordered
// (critical → info, then newest first; the backend pre-sorts). A clickable
// metric strip filters the list to a single severity; Profile / Owner / CWE
// filters narrow it further (all client-side, combined with the severity
// filter — the metric tiles keep the unfiltered totals). The table's Project /
// Target columns show each finding's owning project · target.
//
// Customer scope (customers Plan 2B): the list follows the global scope
// (`useScopedList`), or — embedded in a customer's Findings tab — a fixed
// `customer` prop, which also drops the page header and padding. Either limits
// it to the findings of that customer's projects (`GET /api/findings?customer=`;
// the summary counts only the kept findings). Findings are the coordinator's
// own records, keyed on each project's current assignment, so no host is ever
// left out here. A scope the backend rejects (400) is cleared with a notice.

import { useEffect, useMemo, useState } from 'react';
import { api, normFindingSeverity, type FindingOut, type FindingsSummary } from '../lib/api';
import { type Severity } from '../lib/severity';
import { findingCweIds } from '../lib/cwe';
import { ExportReportButton } from '../components/findings/ExportReportButton';
import { FindingMetrics } from '../components/findings/FindingMetrics';
import { FindingsTable } from '../components/findings/FindingsTable';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { FilterBar } from '../components/ui/FilterBar';
import { FilterPills } from '../components/ui/FilterPills';
import { Select } from '../components/ui/Select';
import { Spinner } from '../components/ui/Spinner';
import { useScopedList } from '../lib/useScopedList';

type ProfileFilter = 'all' | 'full' | 'summary';

const PROFILE_OPTIONS: { value: ProfileFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'full', label: 'Full reports' },
  { value: 'summary', label: 'Summaries' },
];

const EMPTY_SUMMARY: FindingsSummary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 };

export default function Findings({ customer: fixedCustomer }: { customer?: string } = {}) {
  const { customer, embedded, guard } = useScopedList(fixedCustomer, []);
  const [findings, setFindings] = useState<FindingOut[] | null>(null);
  const [summary, setSummary] = useState<FindingsSummary>(EMPTY_SUMMARY);
  const [error, setError] = useState<string | null>(null);
  const [activeSev, setActiveSev] = useState<Severity | null>(null);
  const [profile, setProfile] = useState<ProfileFilter>('all');
  const [owner, setOwner] = useState('');
  const [cwe, setCwe] = useState('');

  useEffect(() => {
    let cancelled = false;
    // A different filter: drop the other filter's rows rather than show them while loading.
    setFindings(null);
    setError(null);
    // Called with no argument when unscoped, as the page always did.
    (customer ? guard(api.getFindings({ customer })) : api.getFindings())
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
  }, [customer, guard]);

  const all = findings ?? [];

  // Option lists come from the full set so choosing one filter never
  // shrinks the choices of another.
  const owners = useMemo(
    () =>
      [...new Set(all.map((f) => f.report_summary?.owner).filter((o): o is string => Boolean(o)))].sort(
        (a, b) => a.localeCompare(b),
      ),
    [all],
  );
  const cwes = useMemo(
    () =>
      [...new Set(all.flatMap((f) => findingCweIds(f)))].sort(
        (a, b) => Number(a.slice(4)) - Number(b.slice(4)),
      ),
    [all],
  );

  // Severity tile + profile / owner / CWE filters, all ANDed (backend order is
  // preserved). Total / re-clicking the active tile clears the severity filter
  // (activeSev === null).
  const rows = useMemo(
    () =>
      all.filter(
        (f) =>
          (!activeSev || normFindingSeverity(f.severity) === activeSev) &&
          (profile === 'all' || (f.profile ?? 'summary') === profile) &&
          (!owner || f.report_summary?.owner === owner) &&
          (!cwe || findingCweIds(f).includes(cwe)),
      ),
    [all, activeSev, profile, owner, cwe],
  );

  const noMatchHint = activeSev && profile === 'all' && !owner && !cwe
    ? `No ${activeSev} findings.`
    : 'No findings match the current filters.';

  return (
    <div className={embedded ? undefined : 'p-8'}>
      {!embedded && (
        <header className="mb-6">
          <h1 className="text-2xl font-semibold text-ink">Findings</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Every finding raised across all registered projects, ordered by severity. Click a metric
            tile to filter the list.
          </p>
        </header>
      )}

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {findings === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading findings…" />
        </div>
      ) : all.length === 0 ? (
        <EmptyState
          title="No findings"
          hint={
            customer === 'none'
              ? 'No project without a customer has recorded a finding yet.'
              : customer
                ? 'None of this customer’s projects has recorded a finding yet.'
                : 'Run an assessment workflow to start recording findings across your projects.'
          }
        />
      ) : (
        <div className="space-y-6">
          <FindingMetrics summary={summary} active={activeSev} onSelect={setActiveSev} />

          <div className="flex flex-wrap items-center gap-2.5">
            <div className="min-w-0 flex-1">
              <FilterBar
                filters={
                  <>
                    <FilterPills
                      label="Profile"
                      options={PROFILE_OPTIONS}
                      value={profile}
                      onChange={(v) => setProfile(v as ProfileFilter)}
                    />
                    <Select aria-label="Owner filter" value={owner} onChange={(e) => setOwner(e.target.value)}>
                      <option value="">All owners</option>
                      {owners.map((o) => (
                        <option key={o} value={o}>
                          {o}
                        </option>
                      ))}
                    </Select>
                    <Select aria-label="CWE filter" value={cwe} onChange={(e) => setCwe(e.target.value)}>
                      <option value="">All CWEs</option>
                      {cwes.map((c) => (
                        <option key={c} value={c}>
                          {c}
                        </option>
                      ))}
                    </Select>
                  </>
                }
              />
            </div>
            {/* The report covers exactly the rows the filters leave. */}
            <ExportReportButton findings={rows} defaultTitle="Findings report" />
          </div>

          {rows.length === 0 ? (
            <EmptyState title="No matches" hint={noMatchHint} />
          ) : (
            <FindingsTable findings={rows} showProvenance />
          )}
        </div>
      )}
    </div>
  );
}
