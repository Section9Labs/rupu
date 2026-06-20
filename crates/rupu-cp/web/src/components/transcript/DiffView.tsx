/**
 * DiffView — renders a unified diff with syntax highlighting.
 *
 * Anatomy:
 *   1. Optional header row  — "{editKind} · {path}" when props present
 *   2. Line list            — monospace block, each line colour-coded by type:
 *        hunk  → dim slate (@@  header)
 *        del   → red   (lines removed)
 *        add   → green (lines added)
 *        ctx   → dim slate (context / file-header lines)
 *
 * parseDiff rules:
 *   - Lines starting with "@@" → hunk
 *   - Lines starting with "---" or "+++" → ctx  (NOT del/add — header guard)
 *   - Lines starting with "diff --git" or "index " → ctx
 *   - Lines starting with "-" (but NOT "---") → del
 *   - Lines starting with "+" (but NOT "+++") → add
 *   - Everything else → ctx
 *
 * No `any`.  Static Tailwind class strings only.
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type DiffLineType = 'hunk' | 'add' | 'del' | 'ctx';

export interface DiffLine {
  type: DiffLineType;
  text: string;
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/**
 * Pure function — split diff text into typed lines.
 * Empty trailing lines are discarded.
 */
export function parseDiff(diff: string): DiffLine[] {
  if (!diff) return [];

  return diff
    .split('\n')
    .filter((line, i, arr) => {
      // Drop trailing empty line that split() produces from a trailing \n
      if (line === '' && i === arr.length - 1) return false;
      return true;
    })
    .filter((line) => line !== '' || true) // keep empty non-trailing lines (rare but valid ctx)
    .map((text): DiffLine => {
      // Hunk header
      if (text.startsWith('@@')) return { type: 'hunk', text };

      // File-header guards — must come BEFORE the +/- single-char checks
      if (text.startsWith('---') || text.startsWith('+++')) return { type: 'ctx', text };
      if (text.startsWith('diff --git') || text.startsWith('index ')) return { type: 'ctx', text };

      // Removed line (single dash, not triple)
      if (text.startsWith('-')) return { type: 'del', text };

      // Added line (single plus, not triple)
      if (text.startsWith('+')) return { type: 'add', text };

      // Context line (space-prefixed, blank, or anything else)
      return { type: 'ctx', text };
    });
}

// ---------------------------------------------------------------------------
// Line styling (static Tailwind)
// ---------------------------------------------------------------------------

const LINE_CLASS: Record<DiffLineType, string> = {
  hunk: 'text-slate-400',
  add:  'bg-green-50 text-green-700',
  del:  'bg-red-50 text-red-700',
  ctx:  'text-slate-600',
};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function DiffView({
  diff,
  path,
  editKind,
}: {
  diff: string;
  path?: string;
  editKind?: string;
}) {
  const lines = parseDiff(diff);

  const headerParts: string[] = [];
  if (editKind) headerParts.push(editKind);
  if (path) headerParts.push(path);
  const header = headerParts.join(' · ');

  return (
    <div className="rounded-md border border-slate-200 overflow-hidden my-1 text-[11.5px]">
      {/* Header row */}
      {header && (
        <div className="flex items-center gap-2 px-3 py-1.5 bg-slate-50 border-b border-slate-200">
          <span className="font-mono text-slate-500 truncate">{header}</span>
        </div>
      )}

      {/* Diff lines */}
      <div className="overflow-x-auto">
        <pre className="font-mono leading-5 px-0 py-0 m-0 bg-white">
          {lines.map((line, i) => (
            <div
              key={i}
              className={`px-3 whitespace-pre ${LINE_CLASS[line.type]}`}
            >
              {line.text}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}
