// Shared findings row — one finding rendered with severity pill, summary,
// `file:line0–line1` location, optional concern chip, optional provenance
// chip, and collapsible evidence. Lifted from CoverageDetail so it can be
// reused on a top-level Findings page.

import { useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { normFindingSeverity, type FindingRecord } from '../../lib/api';
import { cn } from '../../lib/cn';
import { cweFromFinding } from '../../lib/cwe';
import { SEVERITY_STYLE } from '../../lib/severity';
import { AgentName } from '../codename/AgentName';
import { FindingEvidence } from './FindingEvidence';
import { findingCodename } from './findingCodename';

export interface FindingRowProps {
  finding: FindingRecord;
  /** Optional provenance: source project. Renders a `[project · target]`
   *  chip when provided (used by the cross-project Findings page). */
  project?: string;
  /** Optional provenance: source target id. */
  targetId?: string;
  /** Owning workspace id. When present alongside `file_path` + `line_range`,
   *  the location renders as a deep-link into that project's Code tab. */
  wsId?: string;
}

export function FindingRow({ finding, project, targetId, wsId }: FindingRowProps) {
  const sev = normFindingSeverity(finding.severity);
  const s = SEVERITY_STYLE[sev];
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();

  const locationParts: string[] = [];
  if (finding.file_path) locationParts.push(finding.file_path);
  if (finding.line_range) locationParts.push(`${finding.line_range[0]}–${finding.line_range[1]}`);
  const location = locationParts.join(':');

  const hasEvidence = Boolean(
    finding.evidence?.rationale ||
      finding.evidence?.code_excerpt ||
      (finding.evidence?.references ?? []).length > 0,
  );

  const cwe = cweFromFinding(finding);
  const named = findingCodename(finding);

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
            'shrink-0 inline-flex items-center justify-center text-center min-w-[72px] rounded px-2 py-0.5 text-note font-semibold uppercase tracking-wide ring-1 mt-0.5',
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
              <span className="shrink-0 inline-flex items-center rounded px-1.5 py-0.5 text-meta font-medium font-mono bg-surface text-ink-mute ring-1 ring-border mt-0.5">
                {provenance}
              </span>
            </div>
          ) : (
            <p className="text-sm text-ink leading-snug">{finding.summary}</p>
          )}
          <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-note text-ink-mute">
            {location &&
              (finding.file_path && finding.line_range && wsId ? (
                <button
                  type="button"
                  onClick={() =>
                    navigate(
                      `/projects/${encodeURIComponent(wsId)}/code?path=${encodeURIComponent(finding.file_path!)}&line=${finding.line_range![0]}`,
                    )
                  }
                  className="font-mono break-all text-brand-700 hover:underline"
                >
                  {location}
                </button>
              ) : (
                <span className="font-mono break-all">{location}</span>
              ))}
            {cwe && (
              <a
                href={cwe.url}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center rounded bg-surface px-1.5 py-0.5 text-note font-medium text-ink ring-1 ring-border hover:bg-surface-hover"
              >
                {cwe.id}
              </a>
            )}
            {named && <AgentName codename={named.codename} showCrew derived={named.derived} />}
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
            <div className="mt-2">
              <FindingEvidence finding={finding} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default FindingRow;
