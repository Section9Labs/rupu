// Dashboard — the top screen, rebuilt on the common visual language and
// reorganised around the dashboard's real job: TRIAGE (spec
// 2026-10-19-rupu-cp-visual-language-design.md + dashboard-redesign.html).
//
// Composition: PageHeader (range + Usage →) → "Needs you" hero (parked gates /
// paused / recent failures, with inline actions) → "Last 30 days" KPI strip
// (type-first, attention in red) → "Fleet" host cards → "Trends" two living
// charts → "Inventory" summary strip. This is rendering only — every data
// source, the per-host async load, the customer scope, and the "not reported ≠
// 0" partial rules are the SAME as before (`useDashboardData`, the scope
// helpers). The Needs-you hero adds `useNeedsYou`, which reads existing run-list
// + mutation endpoints (no new API).
//
// Components come from the shared language (`components/ui/*`): PageHeader,
// KpiStrip, Chart, SummaryStrip, StatusChip, Button, Skeleton. The Needs-you
// hero and fleet cards live in `components/dashboard/*`.

import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useDashboardData } from '../lib/dashboard/useDashboardData';
import { useNeedsYou } from '../lib/dashboard/useNeedsYou';
import { NeedsYou } from '../components/dashboard/NeedsYou';
import { FleetHostCards } from '../components/dashboard/FleetHostCards';
import { PageHeader } from '../components/ui/PageHeader';
import { KpiStrip, type KpiItem } from '../components/ui/KpiStrip';
import { SummaryStrip, type SummaryItem } from '../components/ui/SummaryStrip';
import { Chart, type ChartSeries } from '../components/ui/Chart';
import { Segmented } from '../components/ui/Segmented';
import { Skeleton } from '../components/ui/Skeleton';
import { formatDuration } from '../lib/duration';
import type { DashboardRange, FleetCounts, TerminalBucket, ThroughputBucket } from '../lib/api';
import { useScopedList } from '../lib/useScopedList';
import { ScopeChip } from '../components/customers/ScopeChip';
import { HostsWithoutCustomerBanner } from '../components/customers/HostsWithoutCustomerBanner';
import { UnscopedNote } from '../components/customers/UnscopedNote';

const RANGE_OPTIONS = [
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'All' },
];

/** A section label in the triage flow. */
function SectionLabel({ children, count }: { children: React.ReactNode; count?: number }) {
  return (
    <h2 className="mb-2.5 mt-6 text-note font-semibold uppercase tracking-wide text-ink-dim first:mt-0">
      {children}
      {count != null && <span className="ml-1.5 tabular-nums text-ink-dim">· {count}</span>}
    </h2>
  );
}

/** Compact relative age ("8s" / "6m" / "2h"); null → undefined. */
function ageLabel(iso: string | null | undefined, now: number): string | undefined {
  if (!iso) return undefined;
  const ms = now - Date.parse(iso);
  if (!Number.isFinite(ms) || ms < 0) return undefined;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
}

function num(v: number | null | undefined): string {
  return v == null ? '—' : v.toLocaleString();
}

function outcomeSeries(buckets: TerminalBucket[]): ChartSeries[] {
  return [
    { key: 'completed', label: 'Completed', values: buckets.map((b) => b.completed), color: 'status.completed', fillOpacity: 0.18 },
    { key: 'failed', label: 'Failed', values: buckets.map((b) => b.failed), color: 'status.failed', fillOpacity: 0.28 },
  ];
}

function throughputSeries(buckets: ThroughputBucket[]): ChartSeries[] {
  return [
    { key: 'manual', label: 'Manual', values: buckets.map((b) => b.manual), color: 'info', fillOpacity: 0.16 },
    { key: 'cron', label: 'Cron', values: buckets.map((b) => b.cron), color: 'status.awaiting', fillOpacity: 0.16 },
    { key: 'event', label: 'Event', values: buckets.map((b) => b.event), color: 'brand.500', fillOpacity: 0.18 },
  ];
}

function dayLabels(buckets: { ts: string }[]): string[] {
  return buckets.map((b) => new Date(b.ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }));
}

function inventoryItems(fleet: FleetCounts): SummaryItem[] {
  const disabled = fleet.autoflows_disabled;
  const openIssues = fleet.issues_open;
  return [
    { label: 'Repos', value: num(fleet.repos), testId: 'inv-repos' },
    { label: 'Providers', value: num(fleet.providers_configured), testId: 'inv-providers' },
    {
      label: 'Autoflows',
      value: num(fleet.autoflows_enabled),
      sub: disabled != null && disabled > 0 ? `· ${disabled} off` : undefined,
      testId: 'inv-autoflows',
    },
    { label: 'Workers', value: num(fleet.workers), testId: 'inv-workers' },
    { label: 'Claimed', value: num(fleet.claims_active), testId: 'inv-claims' },
    {
      label: 'Pending',
      value: num(fleet.issues_pending),
      sub:
        openIssues != null ? `· ${openIssues.toLocaleString()}${fleet.issues_capped ? '+' : ''} open` : undefined,
      testId: 'inv-pending',
    },
  ];
}

export default function Dashboard() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const { customer, rejectIfScopeError } = useScopedList(undefined, []);
  const { data, hosts, error, hostsWithoutCustomer } = useDashboardData(range, customer, rejectIfScopeError);
  const needsYou = useNeedsYou(customer);

  const now = Date.now();

  // Freshness: the most recent ok-host capture, as "updated 8s".
  const freshness = useMemo(() => {
    const caps = hosts
      .filter((h) => h.state === 'ok' && h.summary?.captured_at)
      .map((h) => Date.parse(h.summary!.captured_at));
    if (caps.length === 0) return undefined;
    return ageLabel(new Date(Math.max(...caps)).toISOString(), now);
  }, [hosts, now]);

  const okCount = hosts.filter((h) => h.state === 'ok').length;
  const subtitle = (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1">
      <span>
        {hosts.length > 0 ? `${okCount}/${hosts.length} hosts` : 'Fleet'}
        {freshness && <span className="text-ink-dim"> · updated {freshness}</span>}
      </span>
      <ScopeChip />
    </span>
  );

  const kpis: KpiItem[] | null = useMemo(() => {
    if (!data) return null;
    const failed = data.terminal_buckets.reduce((s, b) => s + b.failed, 0);
    const terminal = data.terminal_buckets.reduce(
      (s, b) => s + b.completed + b.failed + b.rejected + b.cancelled,
      0,
    );
    const completed = data.terminal_buckets.reduce((s, b) => s + b.completed, 0);
    const rate = terminal > 0 ? `${Math.round((completed / terminal) * 100)}%` : '—';
    const longest = data.active_longest;
    const activeMeta = longest
      ? `longest ${formatDuration(longest.age_ms)}`
      : data.fleet.workers != null
        ? `${data.fleet.workers} worker${data.fleet.workers === 1 ? '' : 's'}`
        : undefined;
    return [
      { label: 'Failed', value: failed.toLocaleString(), attention: failed > 0, testId: 'kpi-failed' },
      { label: 'Success rate', value: rate, testId: 'kpi-success' },
      {
        label: 'Cycles',
        value: data.cycles.total.toLocaleString(),
        meta: data.cycles_partial ? 'partial window' : undefined,
        testId: 'kpi-cycles',
      },
      { label: 'Active now', value: data.active.running.toLocaleString(), meta: activeMeta, testId: 'kpi-active' },
      {
        label: 'Open findings',
        value: data.findings_open === null ? '—' : `${data.findings_open.toLocaleString()}${data.findings_partial ? '+' : ''}`,
        meta: data.findings_partial ? `partial · ${okCount} hosts` : undefined,
        testId: 'kpi-findings',
      },
    ];
  }, [data, okCount]);

  return (
    <div className="space-y-2 p-4 lg:p-6">
      <PageHeader
        title="Overview"
        subtitle={subtitle}
        actions={
          <>
            {error && data && (
              <span className="text-note text-status-failed" title={error.message}>
                refresh failed — last good data
              </span>
            )}
            <Segmented
              ariaLabel="Date range"
              options={RANGE_OPTIONS}
              value={range}
              onChange={(v) => setRange(v as DashboardRange)}
            />
            <Link
              to="/usage"
              className="inline-flex items-center rounded-md border border-border bg-panel px-3 py-1.5 text-ui font-medium text-ink transition-colors hover:bg-bg"
            >
              Usage →
            </Link>
          </>
        }
      />

      <HostsWithoutCustomerBanner
        hosts={hosts.map((h) => ({ id: h.hostId, name: h.name, state: h.state, reason: h.reason }))}
        without={hostsWithoutCustomer}
      />

      <SectionLabel count={needsYou.items.length > 0 ? needsYou.items.length : undefined}>
        Needs you
      </SectionLabel>
      <NeedsYou data={needsYou} />

      {data && kpis ? (
        <>
          <SectionLabel>Last 30 days</SectionLabel>
          <KpiStrip items={kpis} />

          <SectionLabel>Fleet</SectionLabel>
          <FleetHostCards hosts={hosts} />

          <SectionLabel>Trends</SectionLabel>
          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            <Chart
              title="Outcomes over time"
              subtitle="Runs per day · completed vs failed"
              dates={dayLabels(data.terminal_buckets)}
              series={outcomeSeries(data.terminal_buckets)}
              headline={{ value: kpis[1].value, label: `ok · ${data.cycles.total.toLocaleString()} cycles` }}
              peakSubLabel={(i) => `${data.terminal_buckets[i]?.failed ?? 0} failed`}
            />
            <Chart
              title="Throughput by trigger"
              subtitle="Runs per day · by what started them"
              dates={dayLabels(data.throughput_buckets)}
              series={throughputSeries(data.throughput_buckets)}
            />
          </div>

          <SectionLabel>Inventory</SectionLabel>
          <UnscopedNote>Fleet counts aren’t per customer.</UnscopedNote>
          <SummaryStrip items={inventoryItems(data.fleet)} />
          {data.fleet_partial && (
            <p className="mt-1 text-note text-ink-dim">Partial sum — some hosts don’t report every count.</p>
          )}
        </>
      ) : error ? (
        <div className="rounded-xl border border-border bg-panel p-6 text-ui text-status-failed">
          Could not load dashboard: {error.message}
        </div>
      ) : (
        <DashboardSkeleton />
      )}
    </div>
  );
}

/** The loading view — skeletons in the real layout, so the page never jumps. */
function DashboardSkeleton() {
  return (
    <div className="space-y-2" aria-hidden>
      <SectionLabel>Last 30 days</SectionLabel>
      <div className="overflow-hidden rounded-xl border border-border bg-panel">
        <div className="-ml-px -mt-px grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
          {[0, 1, 2, 3, 4].map((i) => (
            <div key={i} className="border-l border-t border-border p-4">
              <Skeleton className="h-3 w-16" />
              <Skeleton className="mt-3 h-6 w-12" />
            </div>
          ))}
        </div>
      </div>
      <SectionLabel>Trends</SectionLabel>
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        {[0, 1].map((i) => (
          <div key={i} className="rounded-xl border border-border bg-panel p-4">
            <Skeleton className="h-4 w-40" />
            <Skeleton className="mt-4 h-32 w-full" />
          </div>
        ))}
      </div>
    </div>
  );
}
