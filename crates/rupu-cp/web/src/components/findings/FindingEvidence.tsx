// Evidence panel for a finding — rationale / code excerpt / references. Lifted
// out of FindingRow so it can be reused as the `renderDetail` body of the
// shared SortableTable-based findings list. Renders a muted fallback when a
// finding carries no evidence (the expand chevron is always present on
// expandable tables, so an empty panel would otherwise be confusing).

import { type FindingRecord } from '../../lib/api';

export function FindingEvidence({ finding }: { finding: FindingRecord }) {
  const rationale = finding.evidence?.rationale ?? '';
  const excerpt = finding.evidence?.code_excerpt ?? '';
  const references = finding.evidence?.references ?? [];
  const hasEvidence = Boolean(rationale || excerpt || references.length > 0);

  if (!hasEvidence) {
    return <p className="text-note text-ink-mute">No evidence recorded.</p>;
  }

  return (
    <div className="space-y-2">
      {rationale && (
        <p className="text-ui text-ink-dim leading-snug whitespace-pre-wrap">{rationale}</p>
      )}
      {excerpt && (
        <pre className="overflow-x-auto rounded bg-surface ring-1 ring-border px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">
          {excerpt}
        </pre>
      )}
      {references.length > 0 && (
        <ul className="list-disc pl-4 text-note text-ink-mute space-y-0.5">
          {references.map((ref, i) => (
            <li key={i} className="break-all font-mono">
              {ref}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
