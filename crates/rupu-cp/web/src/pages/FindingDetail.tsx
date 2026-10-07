// /findings/:id — the structured finding report. Full-profile findings render
// as a sectioned report with a sticky section rail and a completeness meter;
// summary-profile findings (no structured report) degrade to the summary plus
// its rationale. Section bodies live in components/findings/report/.

import { useEffect, useRef, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api, apiErrorMessage, type FindingDetail as Detail, type FindingExportFormat } from '../lib/api';
import { saveBlob } from '../lib/download';
import { completeness, UNREADABLE_REPORT_NOTE } from '../lib/findingReport';
import Markdown from '../components/transcript/Markdown';
import { FindingEvidence } from '../components/findings/FindingEvidence';
import Section from '../components/findings/report/Section';
import ReportHeader from '../components/findings/report/ReportHeader';
import CallChain from '../components/findings/report/CallChain';
import EvidenceClaims from '../components/findings/report/EvidenceClaims';
import FixSections from '../components/findings/report/FixSections';
import ReplicationSteps from '../components/findings/report/ReplicationSteps';
import ArtifactBrowser from '../components/findings/report/ArtifactBrowser';
import EvidenceBlocks from '../components/findings/report/EvidenceBlocks';
import CrossReferences from '../components/findings/report/CrossReferences';
import { Button } from '../components/ui/Button';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { TagEditor, type TagChangeReport } from '../components/findings/tags/TagEditor';
import { TagHistory } from '../components/findings/tags/TagHistory';
import type { TagSuggestion } from '../components/findings/tags/TagInput';
import { changedOutcomes, summarizeTagResult } from '../components/findings/tags/tagResult';

const RAIL: [string, string][] = [
  ['s-desc', 'Description'], ['s-impact', 'Impact'], ['s-loc', 'Location'], ['s-root', 'Root cause'],
  ['s-chain', 'Call chain'], ['s-evidence', 'Evidence'], ['s-blocks', 'Evidence blocks'], ['s-artifacts', 'PoC artifacts'], ['s-repro', 'Replication'],
  ['s-remediation', 'Remediation'], ['s-patch', 'Patch'], ['s-ci', 'CI/CD detection'], ['s-reg', 'Regression test'],
  ['s-refs', 'References'], ['s-tags', 'Tags'], ['s-prov', 'Provenance'],
];

function BackLink() {
  return (
    <Link to="/findings" className="inline-block text-ui text-ink-mute hover:text-ink hover:underline">← Findings</Link>
  );
}

const EXPORT_FORMATS: [FindingExportFormat, string][] = [['md', 'Markdown'], ['html', 'HTML'], ['pdf', 'PDF']];

/** Buttons that download this finding's report in each format. They fetch the
 *  export (`GET /api/findings/:id/export`) rather than link to it, so a failure
 *  (404, or 501 when this build has no PDF support) reads as a message here
 *  instead of saving the JSON error body as the "report". */
function ExportLinks({ id }: { id: string }) {
  const [busy, setBusy] = useState<FindingExportFormat | null>(null);
  const [error, setError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  // Leaving the page abandons a download still in flight.
  useEffect(() => () => abortRef.current?.abort(), []);

  async function download(fmt: FindingExportFormat) {
    if (busy) return;
    const controller = new AbortController();
    abortRef.current = controller;
    setBusy(fmt);
    setError(null);
    try {
      const blob = await api.downloadFindingExport(id, fmt, { signal: controller.signal });
      if (controller.signal.aborted) return;
      saveBlob(blob, `${id}.${fmt}`);
    } catch (e: unknown) {
      if (!controller.signal.aborted) setError(apiErrorMessage(e));
    } finally {
      if (!controller.signal.aborted) setBusy(null);
    }
  }

  return (
    <div className="flex flex-col items-end gap-1">
      <div role="group" aria-label="Export finding" className="flex items-center gap-1.5 text-ui">
        <span className="text-ink-mute">Export</span>
        {EXPORT_FORMATS.map(([fmt, label]) => (
          <Button key={fmt} variant="ring" disabled={busy !== null} onClick={() => void download(fmt)}>
            {label}
          </Button>
        ))}
      </div>
      {error && (
        <p role="alert" className="max-w-md text-right text-note text-err">
          {error}
        </p>
      )}
    </div>
  );
}

/** The top row of both layouts: back to the list, and the export links. */
function TopBar({ id }: { id: string }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
      <BackLink />
      <ExportLinks id={id} />
    </div>
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

/** The editable tag row plus the suggestions it needs. Changes go through
 *  `POST /api/findings/tags`. Only a failed POST, or an answer that changed
 *  nothing and names an error (a workspace error or an unknown id), is a failed
 *  change. Once anything changed the write is committed: the page refetches
 *  the finding, a failed refetch is a note (the chips take the write's own
 *  `after`), and a partial error still shows. */
function FindingTags({
  detail,
  onChanged,
  onSaved,
}: {
  detail: Detail;
  onChanged: () => Promise<void>;
  onSaved: (tags: string[]) => void;
}) {
  const [suggestions, setSuggestions] = useState<TagSuggestion[]>([]);
  useEffect(() => {
    let live = true;
    api.getTagsInUse({ wsId: detail.ws_id }).then(
      (t) => { if (live) setSuggestions(t); },
      () => { /* suggestions are optional */ },
    );
    return () => { live = false; };
  }, [detail.ws_id]);
  const change = async (mode: 'add' | 'remove', tag: string): Promise<TagChangeReport> => {
    const r = await api.tagFindings([detail.id], mode === 'add' ? { add: [tag] } : { remove: [tag] });
    const s = summarizeTagResult(r, mode, (ws) => (ws === detail.ws_id && detail.project) || ws);
    const changed = changedOutcomes(r);
    if (changed.length === 0) {
      if (!s.ok) throw new Error(s.message);
      return {};
    }
    const report: TagChangeReport = s.ok ? {} : { error: s.message };
    try {
      await onChanged();
    } catch (e: unknown) {
      const mine = changed.find((o) => o.finding_id === detail.id);
      if (mine) onSaved(mine.after);
      report.note = `Saved, but the page couldn't refresh: ${apiErrorMessage(e)}`;
    }
    return report;
  };
  return (
    <TagEditor
      tags={detail.tags ?? []}
      suggestions={suggestions}
      disabledReason={detail.tags_editable ? null : "This project's tag log couldn't be read, so its tags can't be changed here."}
      onAdd={(t) => change('add', t)}
      onRemove={(t) => change('remove', t)}
    />
  );
}

function TagsSection({ detail }: { detail: Detail }) {
  return (
    <Section id="s-tags" title="Tags">
      <details>
        <summary className="cursor-pointer text-ui text-ink-dim">Tag history ({detail.tag_history.length})</summary>
        <div className="mt-2">
          <TagHistory events={detail.tag_history} />
        </div>
      </details>
    </Section>
  );
}

export default function FindingDetail() {
  const { id = '' } = useParams();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The finding the route shows now: a refetch answering for an earlier one is dropped.
  const currentId = useRef(id);

  useEffect(() => {
    currentId.current = id;
    let live = true;
    setDetail(null);
    setError(null);
    api.getFinding(id).then(
      (d) => { if (live) setDetail(d); },
      (e: unknown) => { if (live) setError(apiErrorMessage(e)); },
    );
    return () => { live = false; };
  }, [id]);

  const refetch = async () => {
    const forId = id;
    const d = await api.getFinding(forId);
    if (currentId.current === forId) setDetail(d);
  };
  const patchTags = (tags: string[]) => {
    const forId = id;
    setDetail((d) => (d && currentId.current === forId ? { ...d, tags } : d));
  };

  if (error) return <div className="p-8"><ErrorBanner>{error}</ErrorBanner></div>;
  if (!detail) return <div className="p-8"><Spinner label="Loading finding" /></div>;

  const report = detail.report;

  if (!report) {
    return (
      <div className="mx-auto max-w-4xl space-y-6 p-8">
        <TopBar id={detail.id} />
        <h1 className="text-2xl font-semibold text-ink">{detail.summary}</h1>
        <FindingTags detail={detail} onChanged={refetch} onSaved={patchTags} />
        <p className="text-ui text-ink-mute">
          {detail.profile === 'full'
            ? UNREADABLE_REPORT_NOTE
            : 'This finding was recorded as a summary, without a full report.'}
        </p>
        <FindingEvidence finding={detail} />
        <TagsSection detail={detail} />
        <Provenance detail={detail} />
      </div>
    );
  }

  const c = completeness(report);
  const hasArtifacts = (report.artifacts?.length ?? 0) > 0;
  // The PoC artifacts section only renders when there are artifacts; keep the
  // rail in step so it never points at a missing anchor.
  const hasBlocks = (report.blocks?.length ?? 0) > 0;
  const rail = RAIL.filter(([anchor]) => (anchor !== 's-artifacts' || hasArtifacts) && (anchor !== 's-blocks' || hasBlocks));
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
        <TopBar id={detail.id} />
        <ReportHeader finding={detail} report={report} />
        <FindingTags detail={detail} onChanged={refetch} onSaved={patchTags} />
        {/* The rail (and its meter) only shows from `lg` up; below that this
            compact line carries the completeness instead. */}
        <p data-testid="completeness-compact" className="text-note text-ink-mute lg:hidden">
          {`Report ${c.filled}/${c.total}${c.gaps.length > 0 ? ` · gaps: ${c.gaps.join(', ')}` : ''}`}
        </p>
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
        {hasBlocks && <Section id="s-blocks" title="Evidence blocks"><EvidenceBlocks findingId={detail.id} blocks={report.blocks!} /></Section>}
        {hasArtifacts && <Section id="s-artifacts" title="PoC artifacts"><ArtifactBrowser findingId={detail.id} artifacts={report.artifacts!} /></Section>}
        <Section id="s-repro" title="Replication steps"><ReplicationSteps steps={report.replication_steps} /></Section>
        <Section id="s-remediation" title="Remediation"><div className="text-ink-dim"><Markdown text={report.remediation} /></div></Section>
        <FixSections report={report} />
        <Section id="s-refs" title="References"><CrossReferences refs={report.cross_references} references={report.references} /></Section>
        <TagsSection detail={detail} />
        <Provenance detail={detail} />
      </article>
    </div>
  );
}
