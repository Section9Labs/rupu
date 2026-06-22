// Shared findings row — one finding rendered with severity pill, summary,
// `file:line0–line1` location, optional concern chip, optional provenance
// chip, and collapsible evidence. Lifted from CoverageDetail so it can be
// reused on a top-level Findings page.

import { useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { normFindingSeverity, type FindingRecord } from '../../lib/api';
import { cn } from '../../lib/cn';
import { SEVERITY_STYLE } from '../../lib/severity';

export interface FindingRowProps {
  finding: FindingRecord;
  /** Optional provenance: source project. Renders a `[project · target]`
   *  chip when provided (used by the cross-project Findings page). */
  project?: string;
  /** Optional provenance: source target id. */
  targetId?: string;
}

export function FindingRow({ finding, project, targetId }: FindingRowProps) {
  const sev = normFindingSeverity(finding.severity);
  const s = SEVERITY_STYLE[sev];
  const [open, setOpen] = useState(false);

  const locationParts: string[] = [];
  if (finding.file_path) locationParts.push(finding.file_path);
  if (finding.line_range) locationParts.push(`${finding.line_range[0]}–${finding.line_range[1]}`);
  const location = locationParts.join(':');

  const rationale = finding.evidence?.rationale ?? '';
  const excerpt = finding.evidence?.code_excerpt ?? '';
  const references = finding.evidence?.references ?? [];
  const hasEvidence = Boolean(rationale || excerpt || references.length > 0);

  // Provenance chip text — `project · target`, omitting empty halves.
  const provParts: string[] = [];
  if (project) provParts.push(project);
  if (targetId) provParts.push(targetId);
  const provenance = provParts.join(' · ');

  return (
    <div className="px-4 py-3">
      <div className="flex items-start gap-3">
        {/* Severity badge */}
        <span
          className={cn(
            'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ring-1 mt-0.5',
            s.pill,
          )}
        >
          {s.label}
        </span>

        {/* Body */}
        <div className="min-w-0 flex-1">
          {provenance ? (
            <div className="flex flex-wrap items-start gap-x-2 gap-y-1">
              <p className="text-sm text-ink leading-snug min-w-0 flex-1">{finding.summary}</p>
              <span className="shrink-0 inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium font-mono bg-slate-100 text-ink-mute ring-1 ring-slate-200 mt-0.5">
                {provenance}
              </span>
            </div>
          ) : (
            <p className="text-sm text-ink leading-snug">{finding.summary}</p>
          )}
          <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute">
            {location && <span className="font-mono break-all">{location}</span>}
            {finding.concern_id && (
              <span>
                concern <span className="font-mono">{finding.concern_id}</span>
              </span>
            )}
            {hasEvidence && (
              <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                className="inline-flex items-center gap-0.5 text-ink-dim hover:text-ink"
              >
                {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                evidence
              </button>
            )}
          </div>

          {hasEvidence && open && (
            <div className="mt-2 space-y-2">
              {rationale && (
                <p className="text-[12px] text-ink-dim leading-snug whitespace-pre-wrap">
                  {rationale}
                </p>
              )}
              {excerpt && (
                <pre className="overflow-x-auto rounded bg-slate-50 ring-1 ring-slate-200 px-3 py-2 text-[11px] font-mono text-ink leading-snug whitespace-pre">
                  {excerpt}
                </pre>
              )}
              {references.length > 0 && (
                <ul className="list-disc pl-4 text-[11px] text-ink-mute space-y-0.5">
                  {references.map((ref, i) => (
                    <li key={i} className="break-all font-mono">{ref}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
