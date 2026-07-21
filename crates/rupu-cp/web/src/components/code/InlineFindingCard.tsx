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
 * Multiple findings anchored on the same source line stack as separate
 * `InlineFindingCard`s (see `CodeViewer`), each independently collapsible —
 * the aikido-style "several review comments on one line" pattern.
 */

import { useState } from 'react';
import type { FindingRecord } from '../../lib/api';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';
import Markdown from '../transcript/Markdown';

export interface InlineFindingCardProps {
  finding: FindingRecord;
  /** True when the recorded code excerpt no longer matches the current file
   *  content at this line — surfaces a drift disclaimer instead of hiding
   *  the (possibly stale) finding. */
  stale: boolean;
}

export default function InlineFindingCard({ finding, stale }: InlineFindingCardProps) {
  const [open, setOpen] = useState(false);
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
              ⚠ The code may have changed since this finding was recorded — the line below is
              where it was found.
            </div>
          )}
          {finding.evidence?.rationale && (
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
