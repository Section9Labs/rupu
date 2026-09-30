import SeverityChip from '../../coverage/SeverityChip';
import { normFindingSeverity, type FindingOut } from '../../../lib/api';
import { isGapSentinel, isSentinel, type FindingReport } from '../../../lib/findingReport';

function Cell({ label, value }: { label: string; value: string }) {
  const gap = isGapSentinel(value);
  return (
    <>
      <dt className="text-ui text-ink-mute">{label}</dt>
      <dd data-gap={gap ? 'true' : 'false'} className={`min-w-0 break-words text-ui ${gap ? 'text-warn' : 'text-ink'}`}>{value}</dd>
    </>
  );
}

export default function ReportHeader({ finding, report }: { finding: FindingOut; report: FindingReport }) {
  const r = report.rating;
  const tickets = isSentinel(report.tickets) ? report.tickets : report.tickets.map((t) => `${t.type} ${t.identifier}`).join(', ');
  return (
    <header className="space-y-3">
      <div className="font-mono text-note text-ink-mute">{finding.project || finding.ws_id} · {finding.workflow_name ?? 'finding'} · {finding.id}</div>
      <h1 className="text-2xl font-semibold leading-tight text-ink">{report.title}</h1>
      <div className="flex flex-wrap items-center gap-1.5">
        <SeverityChip severity={normFindingSeverity(r.risk_rating)} />
        {(report.cwe ?? []).map((c) => {
          const n = c.replace(/^CWE-/, '');
          return (
            <a key={c} href={`https://cwe.mitre.org/data/definitions/${n}.html`} target="_blank" rel="noreferrer"
               className="rounded bg-surface px-1.5 py-0.5 text-note font-medium text-ink ring-1 ring-border hover:bg-surface-hover">{c}</a>
          );
        })}
        {report.verification && <span className="rounded bg-surface px-1.5 py-0.5 text-note text-ink-dim ring-1 ring-border">verification: {report.verification.status}</span>}
        {(report.artifacts?.length ?? 0) > 0 && <span className="rounded bg-ok-bg px-1.5 py-0.5 text-note text-ok">PoC artifacts: {report.artifacts!.length}</span>}
      </div>
      <div className="grid gap-0 overflow-hidden rounded-md border border-border bg-panel md:grid-cols-2">
        <dl className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-3 md:border-r md:border-border">
          <Cell label="Owner" value={report.ownership.owner} />
          <Cell label="Product" value={report.ownership.product} />
          <Cell label="Component" value={report.ownership.affected_component} />
          <Cell label="Source repo" value={report.ownership.source_repository} />
          <Cell label="Tickets" value={tickets} />
        </dl>
        <dl className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-3">
          <Cell label="Category" value={report.category} />
          <Cell label="Attack vector" value={report.attack_vector} />
          <Cell label="Impact" value={r.impact} />
          <Cell label="Likelihood" value={r.likelihood} />
          <Cell label="Risk rating" value={r.risk_rating} />
          <Cell label="Risk factor" value={r.risk_factor} />
          <Cell label="CVSS v3" value={r.cvss_v3} />
        </dl>
      </div>
    </header>
  );
}
