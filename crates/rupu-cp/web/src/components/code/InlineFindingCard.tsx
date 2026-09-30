/**
 * InlineFindingCard — a PR-style inline review comment anchored under a
 * finding's line range in `CodeViewer`.
 *
 * Anatomy:
 *   Collapsed — a one-line marker: severity dot + summary + severity pill.
 *               Click (or Enter/Space, it's a real <button>) expands it.
 *   Expanded  — adds a stale-drift note (when `stale`), the rationale
 *               (rendered as markdown, matching `FindingCard`'s convention
 *               for the same field), a concern-id chip, reference chips, and
 *               a "View on repository" link when `finding.permalink` is
 *               present (added in a later task — guarded here since the
 *               field doesn't exist on `FindingRecord` yet).
 *
 * Full-profile findings (`profile === 'full'`) additionally get a tabbed
 * report view in the expanded body — Root cause / Call chain / Evidence /
 * Patch / Repro — backed by `GET /api/findings/:id`. The detail is fetched
 * lazily, only once the card is expanded (the list rows are slim), and
 * summary-profile findings never fetch and render exactly as before.
 *
 * Multiple findings anchored on the same source line stack as separate
 * `InlineFindingCard`s (see `CodeViewer`), each independently collapsible —
 * the aikido-style "several review comments on one line" pattern.
 */

import { useEffect, useId, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, apiErrorMessage, type FindingDetail, type FindingOut, type FindingRecord } from '../../lib/api';
import { isSentinel, sentinelLabel } from '../../lib/findingReport';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';
import CallChain from '../findings/report/CallChain';
import EvidenceClaims from '../findings/report/EvidenceClaims';
import ReplicationSteps from '../findings/report/ReplicationSteps';
import DiffView from '../transcript/DiffView';
import Markdown from '../transcript/Markdown';
import { Spinner } from '../ui/Spinner';

const TABS = ['Root cause', 'Call chain', 'Evidence', 'Patch', 'Repro'] as const;
type Tab = (typeof TABS)[number];

const EMPTY_NOTE = 'text-ui text-ink-mute';

/** `Root cause` -> `root-cause`: a valid HTML id fragment (no spaces) so the
 *  tabpanel's `aria-labelledby` IDREF resolves. */
const tabSlug = (t: Tab) => t.toLowerCase().replace(/\s+/g, '-');

/** The tabbed report body for a full-profile finding. Rendered only once the
 *  detail (row + `report` + `evidence_status`) has loaded. */
function ReportTabs({ detail, wsId }: { detail: FindingDetail; wsId?: string }) {
  const [tab, setTab] = useState<Tab>('Root cause');
  const baseId = useId();
  const report = detail.report;
  if (!report) {
    return <p className={EMPTY_NOTE}>This finding has no structured report body.</p>;
  }
  const patch = report.recommended_patch;
  return (
    <div>
      <div role="tablist" aria-label="Finding report" className="flex flex-wrap gap-1 border-b border-border">
        {TABS.map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            id={`${baseId}-tab-${tabSlug(t)}`}
            aria-selected={tab === t}
            aria-controls={`${baseId}-panel`}
            onClick={() => setTab(t)}
            className={`-mb-px border-b-2 px-2 py-1 text-note font-medium ${
              tab === t
                ? 'border-brand-600 text-ink'
                : 'border-transparent text-ink-mute hover:text-ink'
            }`}
          >
            {t}
          </button>
        ))}
      </div>
      <div
        role="tabpanel"
        id={`${baseId}-panel`}
        aria-labelledby={`${baseId}-tab-${tabSlug(tab)}`}
        className="pt-2"
      >
        {tab === 'Root cause' && (
          <div className="-mx-1 [&_p]:text-[12px] [&_p]:text-ink-dim">
            <Markdown text={report.root_cause} />
          </div>
        )}
        {tab === 'Call chain' && <CallChain chain={report.call_chain} wsId={wsId} />}
        {tab === 'Evidence' &&
          (report.evidence.length > 0 ? (
            <EvidenceClaims claims={report.evidence} states={detail.evidence_status} wsId={wsId} />
          ) : (
            <p className={EMPTY_NOTE}>No evidence claims recorded.</p>
          ))}
        {tab === 'Patch' &&
          (isSentinel(patch) ? (
            <p className={EMPTY_NOTE}>{sentinelLabel(patch)}</p>
          ) : (
            <div className="space-y-2">
              <DiffView diff={patch.diff} />
              {patch.notes && (
                <div className="text-ink-dim [&_p]:text-[12px]">
                  <Markdown text={patch.notes} />
                </div>
              )}
            </div>
          ))}
        {tab === 'Repro' &&
          (report.replication_steps.length > 0 ? (
            <ReplicationSteps steps={report.replication_steps} />
          ) : (
            <p className={EMPTY_NOTE}>No replication steps recorded.</p>
          ))}
      </div>
    </div>
  );
}

export interface InlineFindingCardProps {
  finding: FindingRecord;
  /** True when the recorded code excerpt no longer matches the current file
   *  content at this line — surfaces a drift disclaimer instead of hiding
   *  the (possibly stale) finding. */
  stale: boolean;
}

export default function InlineFindingCard({ finding, stale }: InlineFindingCardProps) {
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<FindingDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const isFull = finding.profile === 'full';
  // Only trust a loaded detail that belongs to *this* finding (guards a card
  // instance being re-pointed at a different id).
  const loaded = detail && detail.id === finding.id ? detail : null;

  // Lazy detail fetch: full-profile cards only, and only once expanded. The
  // `live` flag drops a response that lands after collapse / unmount / an id
  // change so it can never set state on a stale card.
  useEffect(() => {
    if (!open || !isFull || loaded) return;
    let live = true;
    setLoadError(null);
    api.getFinding(finding.id).then(
      (d) => {
        if (live) setDetail(d);
      },
      (e: unknown) => {
        if (live) setLoadError(apiErrorMessage(e));
      },
    );
    return () => {
      live = false;
    };
  }, [open, isFull, finding.id, loaded]);

  const sev = (finding.severity as Severity) ?? 'info';
  const style = SEVERITY_STYLE[sev] ?? SEVERITY_STYLE.info;
  const references = finding.evidence?.references ?? [];
  // `permalink` is wired up by Task 12 (SCM deep-link); it's optional on
  // `FindingRecord` today, so this degrades cleanly (no link) until it's
  // populated.
  const permalink = finding.permalink;

  return (
    <div
      className={`my-1 overflow-hidden rounded-md border border-border bg-panel shadow-sm ring-1 ${style.ring}`}
    >
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] hover:bg-surface"
      >
        <span className={`h-2 w-2 shrink-0 rounded-full ${style.bar}`} aria-hidden />
        <span className="min-w-0 flex-1 truncate font-medium text-ink">{finding.summary}</span>
        <span
          className={`ml-auto shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide ring-1 ring-inset ${style.pill}`}
        >
          {style.label}
        </span>
      </button>
      {open && (
        <div className="border-t border-border px-3 py-2.5 text-[12px] text-ink-dim">
          {stale && (
            <div className="mb-2 rounded bg-warn-bg px-2 py-1 text-[11px] text-ink">
              ⚠ The code may have changed since this finding was recorded.
            </div>
          )}
          {/* For full-profile findings the server sets `evidence.rationale` to
              the report's root cause, which is the default tab's content —
              so it only stands in as the placeholder until the report has
              loaded (or if loading fails). */}
          {finding.evidence?.rationale && !(isFull && loaded?.report) && (
            <div className="-mx-1 [&_p]:text-[12px] [&_p]:text-ink-dim">
              <Markdown text={finding.evidence.rationale} />
            </div>
          )}
          {finding.concern_id && (
            <div className="mt-2 text-[11px] text-ink-mute">Concern: {finding.concern_id}</div>
          )}
          {references.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              {references.map((r) => (
                <span
                  key={r}
                  className="rounded bg-surface px-1.5 py-0.5 text-[10.5px] font-mono text-ink-dim"
                >
                  {r}
                </span>
              ))}
            </div>
          )}
          {isFull && (
            <div className="mt-2 border-t border-border pt-2">
              {loaded ? (
                <ReportTabs
                  key={loaded.id}
                  detail={loaded}
                  wsId={(finding as Partial<FindingOut>).ws_id}
                />
              ) : loadError ? (
                <p role="alert" className="text-ui text-err">
                  Couldn’t load the report: {loadError}
                </p>
              ) : (
                <Spinner size="sm" label="Loading report…" className="text-ui" />
              )}
              <Link
                to={`/findings/${encodeURIComponent(finding.id)}`}
                className="mt-2 inline-block text-note text-brand-700 hover:underline"
              >
                Open full report →
              </Link>
            </div>
          )}
          {permalink && (
            <a
              href={permalink}
              target="_blank"
              rel="noreferrer"
              className="mt-2 inline-block text-[11px] text-brand-700 hover:underline"
            >
              View on repository ↗
            </a>
          )}
        </div>
      )}
    </div>
  );
}
