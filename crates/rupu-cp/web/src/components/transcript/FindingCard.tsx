/**
 * FindingCard — Okesu-style finding card for the transcript panel.
 *
 * Anatomy (top → bottom):
 *   1. Severity hairline  — 1-px coloured bar at the very top (sev ramp)
 *   2. Card header row    — severity badge pill  +  scope chip  +  concern_id chip
 *   3. Title              — summary in severity-tinted bold
 *   4. Location chip      — file_path[:start-end] in mono (only when filePath present);
 *                           a clickable button toggling an inline `SourcePreview`
 *                           when `runId` is known, else a plain non-clickable span.
 *   5. Rationale          — rendered via <Markdown>
 *   6. Code excerpt       — <pre> block (only when codeExcerpt present)
 *   7. References         — link list (only when references.length > 0)
 *
 * Props: { finding: FindingView, runId?: string, host?: string }
 * No `any`; static Tailwind class strings only.
 */

import { useState } from 'react';
import type { FindingView } from './transcriptView';
import { SEVERITY_STYLE } from '../../lib/severity';
import Markdown from './Markdown';
import SourcePreview from './SourcePreview';

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

export default function FindingCard({
  finding,
  runId,
  host,
}: {
  finding: FindingView;
  /** Run id for the source-preview affordance on the location chip. Threaded
   *  down from `TranscriptPanel` via `Turn`/`ToolCard`. Absent → the chip
   *  renders as non-clickable text. */
  runId?: string;
  /** Remote host id to forward to `api.readSource`. */
  host?: string;
}) {
  const [previewOpen, setPreviewOpen] = useState(false);
  const sev = finding.severity;
  const s = SEVERITY_STYLE[sev];

  // Build location string: "path" or "path:start-end"
  let location = '';
  if (finding.filePath) {
    location = finding.filePath;
    if (finding.lineRange) {
      location += `:${finding.lineRange[0]}-${finding.lineRange[1]}`;
    }
  }
  const previewLine = finding.lineRange?.[0] ?? 1;

  return (
    <div className="border border-border rounded-lg bg-panel overflow-hidden shadow-sm my-1">
      {/* 1. Severity hairline */}
      <div className={`h-1 ${s.bar}`} aria-hidden />

      {/* Card body */}
      <div className="px-3 py-2.5 space-y-2">
        {/* 2. Header row: badge + chips */}
        <div className="flex flex-wrap items-center gap-1.5">
          {/* Severity badge */}
          <span
            className={`inline-flex items-center rounded px-2 py-0.5 text-meta font-bold uppercase tracking-wider ring-1 ring-inset ${s.pill}`}
          >
            {s.label.toUpperCase()}
          </span>

          {/* Scope chip */}
          {finding.scope && (
            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-meta bg-surface text-ink-mute">
              {finding.scope}
            </span>
          )}

          {/* Concern ID chip */}
          {finding.concernId && (
            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-meta bg-surface text-ink-mute font-mono">
              {finding.concernId}
            </span>
          )}
        </div>

        {/* 3. Summary / title */}
        <p className={`text-lead font-semibold leading-snug ${s.text}`}>
          {finding.summary}
        </p>

        {/* 4. Location chip — clickable when runId is known, else plain text */}
        {location && (
          finding.filePath && runId ? (
            <button
              type="button"
              onClick={() => setPreviewOpen((v) => !v)}
              className="inline-flex items-center rounded bg-surface border border-border px-1.5 py-0.5 text-[10.5px] font-mono text-ink-dim break-all hover:text-brand-700 hover:underline"
            >
              {location}
            </button>
          ) : (
            <span className="inline-flex items-center rounded bg-surface border border-border px-1.5 py-0.5 text-[10.5px] font-mono text-ink-dim break-all">
              {location}
            </span>
          )
        )}
        {previewOpen && finding.filePath && runId && (
          <SourcePreview runId={runId} path={finding.filePath} line={previewLine} host={host} />
        )}

        {/* 5. Rationale via Markdown */}
        {finding.rationale && (
          <div className="text-ui text-ink-dim">
            <Markdown text={finding.rationale} />
          </div>
        )}

        {/* 6. Code excerpt */}
        {finding.codeExcerpt && (
          <pre className="overflow-x-auto rounded bg-surface ring-1 ring-border px-3 py-2 text-[10.5px] font-mono text-ink leading-snug whitespace-pre">
            {finding.codeExcerpt}
          </pre>
        )}

        {/* 7. References */}
        {finding.references.length > 0 && (
          <div>
            <p className="text-meta uppercase tracking-wider text-ink-mute font-semibold mb-1">
              References
            </p>
            <ul className="space-y-0.5">
              {finding.references.map((ref, i) => (
                <li key={i} className="text-note break-all">
                  <a
                    href={ref}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-brand-700 underline underline-offset-2 hover:text-brand-500 transition-colors"
                  >
                    {ref}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
