// /findings/:id — the structured finding report. Full-profile findings render
// as a sectioned report with a sticky section rail and a completeness meter;
// summary-profile findings (no structured report) degrade to the summary plus
// its rationale. Section bodies live in components/findings/report/.

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api, type FindingDetail as Detail } from '../lib/api';
import { completeness } from '../lib/findingReport';
import Markdown from '../components/transcript/Markdown';
import { FindingEvidence } from '../components/findings/FindingEvidence';
import Section from '../components/findings/report/Section';
import ReportHeader from '../components/findings/report/ReportHeader';
import CallChain from '../components/findings/report/CallChain';
import EvidenceClaims from '../components/findings/report/EvidenceClaims';
import FixSections from '../components/findings/report/FixSections';
import ReplicationSteps from '../components/findings/report/ReplicationSteps';
import ArtifactBrowser from '../components/findings/report/ArtifactBrowser';
import CrossReferences from '../components/findings/report/CrossReferences';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';

const RAIL: [string, string][] = [
  ['s-desc', 'Description'], ['s-impact', 'Impact'], ['s-loc', 'Location'], ['s-root', 'Root cause'],
  ['s-chain', 'Call chain'], ['s-evidence', 'Evidence'], ['s-artifacts', 'PoC artifacts'], ['s-repro', 'Replication'],
  ['s-remediation', 'Remediation'], ['s-patch', 'Patch'], ['s-ci', 'CI/CD detection'], ['s-reg', 'Regression test'],
  ['s-refs', 'References'], ['s-prov', 'Provenance'],
];

function BackLink() {
  return (
    <Link to="/findings" className="inline-block text-ui text-ink-mute hover:text-ink hover:underline">← Findings</Link>
  );
}

/** `FindingRecord.declared_by` is typed `unknown` (other surfaces carry `null`
 *  or partial shapes), so read the attribution fields defensively. */
function attribution(v: unknown): { run_id?: string; model?: string } {
  if (typeof v !== 'object' || v === null) return {};
  const o = v as Record<string, unknown>;
  return {
    run_id: typeof o.run_id === 'string' && o.run_id ? o.run_id : undefined,
    model: typeof o.model === 'string' && o.model ? o.model : undefined,
  };
}

function Provenance({ detail }: { detail: Detail }) {
  const by = attribution(detail.declared_by);
  return (
    <Section id="s-prov" title="Provenance">
      <dl className="grid gap-x-6 gap-y-1 text-ui text-ink-dim sm:grid-cols-2">
        <div><dt className="text-note text-ink-mute">Finding</dt><dd className="font-mono">{detail.id}</dd></div>
        {by.run_id && (
          <div>
            <dt className="text-note text-ink-mute">Run</dt>
            <dd><Link className="font-mono text-brand-700 hover:underline" to={`/runs/${encodeURIComponent(by.run_id)}`}>{by.run_id}</Link></dd>
          </div>
        )}
        {by.model && <div><dt className="text-note text-ink-mute">Model</dt><dd>{by.model}</dd></div>}
        <div><dt className="text-note text-ink-mute">Declared</dt><dd>{new Date(detail.declared_at).toLocaleString()}</dd></div>
      </dl>
    </Section>
  );
}

export default function FindingDetail() {
  const { id = '' } = useParams();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setDetail(null);
    setError(null);
    api.getFinding(id).then(
      (d) => { if (live) setDetail(d); },
      (e: unknown) => { if (live) setError(e instanceof Error ? e.message : String(e)); },
    );
    return () => { live = false; };
  }, [id]);

  if (error) return <div className="p-8"><ErrorBanner>{error}</ErrorBanner></div>;
  if (!detail) return <div className="p-8"><Spinner label="Loading finding" /></div>;

  const report = detail.report;

  if (!report) {
    return (
      <div className="mx-auto max-w-4xl space-y-6 p-8">
        <BackLink />
        <h1 className="text-2xl font-semibold text-ink">{detail.summary}</h1>
        <p className="text-ui text-ink-mute">This finding was recorded as a summary, without a full report.</p>
        <FindingEvidence finding={detail} />
        <Provenance detail={detail} />
      </div>
    );
  }

  const c = completeness(report);
  const hasArtifacts = (report.artifacts?.length ?? 0) > 0;
  // The PoC artifacts section only renders when there are artifacts; keep the
  // rail in step so it never points at a missing anchor.
  const rail = RAIL.filter(([anchor]) => anchor !== 's-artifacts' || hasArtifacts);
  return (
    <div className="grid gap-8 p-8 lg:grid-cols-[13rem_minmax(0,1fr)]">
      <nav aria-label="Report sections" className="hidden self-start lg:sticky lg:top-4 lg:block">
        <div className="mb-3 rounded-md border border-border bg-panel p-3">
          <div className="flex justify-between text-note"><span className="font-semibold text-ink">Report</span><span className="font-mono text-ink-dim">{c.filled}/{c.total}</span></div>
          <div className="mt-1.5 h-1.5 overflow-hidden rounded bg-surface"><div className="h-full bg-ok" style={{ width: `${(c.filled / c.total) * 100}%` }} /></div>
          {c.gaps.length > 0 && <p className="mt-1.5 text-note text-ink-mute">Gaps: {c.gaps.join(', ')}</p>}
        </div>
        <ul className="space-y-px">
          {rail.map(([anchor, label]) => (
            <li key={anchor}><a href={`#${anchor}`} className="block border-l-2 border-border px-2 py-0.5 text-note text-ink-mute hover:border-brand-500 hover:text-ink">{label}</a></li>
          ))}
        </ul>
      </nav>
      <article className="min-w-0 max-w-4xl space-y-7">
        <BackLink />
        <ReportHeader finding={detail} report={report} />
        <Section id="s-desc" title="Description"><div className="text-ink-dim"><Markdown text={report.description} /></div></Section>
        <Section id="s-impact" title="Impact"><div className="text-ink-dim"><Markdown text={report.impact} /></div></Section>
        <Section id="s-loc" title="Location">
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="rounded-md border border-border bg-panel px-3 py-2"><div className="text-meta uppercase tracking-wide text-ink-mute">Input</div><Markdown text={report.location.input} /></div>
            <div className="rounded-md border border-border bg-panel px-3 py-2"><div className="text-meta uppercase tracking-wide text-ink-mute">Output</div><Markdown text={report.location.output} /></div>
          </div>
        </Section>
        <Section id="s-root" title="Root cause"><div className="rounded-md border border-brand-500 bg-brand-50 px-4 py-3 text-ink"><Markdown text={report.root_cause} /></div></Section>
        <Section id="s-chain" title="Call chain"><CallChain chain={report.call_chain} wsId={detail.ws_id} /></Section>
        <Section id="s-evidence" title="Evidence"><EvidenceClaims claims={report.evidence} states={detail.evidence_status} wsId={detail.ws_id} /></Section>
        {hasArtifacts && <Section id="s-artifacts" title="PoC artifacts"><ArtifactBrowser findingId={detail.id} artifacts={report.artifacts!} /></Section>}
        <Section id="s-repro" title="Replication steps"><ReplicationSteps steps={report.replication_steps} /></Section>
        <Section id="s-remediation" title="Remediation"><div className="text-ink-dim"><Markdown text={report.remediation} /></div></Section>
        <FixSections report={report} />
        <Section id="s-refs" title="References"><CrossReferences refs={report.cross_references} references={report.references} /></Section>
        <Provenance detail={detail} />
      </article>
    </div>
  );
}
