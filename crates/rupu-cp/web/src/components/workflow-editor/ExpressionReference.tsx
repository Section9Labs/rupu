// ExpressionReference — a searchable, grouped reference of the supported
// minijinja template vocabulary (inputs / steps / loop / event / issue /
// functions / filters). Data source: `expressionReference()` from
// lib/workflowExpressions (the single source of truth, built in P5).
//
// Each entry is a button. With `onInsert` provided, clicking inserts the entry's
// `insert` text into the focused field (caller wiring); otherwise it copies the
// text to the clipboard and flashes a transient "copied" indicator.

import { useMemo, useRef, useState } from 'react';
import { expressionReference, type ExprEntry, type ExprKind } from '../../lib/workflowExpressions';

interface ExpressionReferenceProps {
  /** When provided, clicking an entry inserts its text instead of copying. */
  onInsert?: (insert: string) => void;
}

const KIND_LABEL: Record<ExprKind, string> = {
  path: 'path',
  filter: 'filter',
  function: 'fn',
  loop: 'loop',
  keyword: 'kw',
};

const KIND_CLASS: Record<ExprKind, string> = {
  path: 'bg-sky-50 text-sky-700 ring-sky-200',
  filter: 'bg-violet-50 text-violet-700 ring-violet-200',
  function: 'bg-amber-50 text-amber-700 ring-amber-200',
  loop: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  keyword: 'bg-slate-100 text-slate-700 ring-slate-200',
};

function matches(entry: ExprEntry, q: string): boolean {
  return (
    entry.label.toLowerCase().includes(q) ||
    entry.insert.toLowerCase().includes(q) ||
    entry.detail.toLowerCase().includes(q)
  );
}

export default function ExpressionReference({ onInsert }: ExpressionReferenceProps) {
  const [query, setQuery] = useState('');
  const [copied, setCopied] = useState<string | null>(null);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const groups = useMemo(() => {
    const all = expressionReference();
    const q = query.trim().toLowerCase();
    if (!q) return all;
    return all
      .map((g) => ({ group: g.group, entries: g.entries.filter((e) => matches(e, q)) }))
      .filter((g) => g.entries.length > 0);
  }, [query]);

  function flashCopied(insert: string) {
    setCopied(insert);
    if (copiedTimer.current) clearTimeout(copiedTimer.current);
    copiedTimer.current = setTimeout(() => setCopied(null), 1200);
  }

  function handleClick(entry: ExprEntry) {
    if (onInsert) {
      onInsert(entry.insert);
      return;
    }
    void navigator.clipboard?.writeText(entry.insert).then(
      () => flashCopied(entry.insert),
      () => {
        /* clipboard denied — no-op (no insert target either) */
      },
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="mb-3">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search expressions…"
          aria-label="Search expressions"
          className="w-full rounded-md border border-border bg-white px-2.5 py-1.5 text-ui text-ink placeholder:text-ink-mute focus:border-brand-400 focus:outline-none focus:ring-1 focus:ring-brand-300"
        />
        <p className="mt-1.5 text-note text-ink-mute">
          {onInsert ? 'Click to insert into the focused field.' : 'Click to copy to the clipboard.'}
        </p>
      </div>

      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto">
        {groups.length === 0 ? (
          <p className="text-ui text-ink-dim">No expressions match “{query}”.</p>
        ) : (
          groups.map((g) => (
            <section key={g.group}>
              <h3 className="mb-1.5 text-note font-semibold uppercase tracking-wide text-ink-mute">
                {g.group}
              </h3>
              <ul className="space-y-1">
                {g.entries.map((entry) => (
                  <li key={entry.insert}>
                    <button
                      type="button"
                      onClick={() => handleClick(entry)}
                      title={entry.insert}
                      className="group flex w-full items-start gap-2 rounded-md border border-transparent px-2 py-1.5 text-left hover:border-border hover:bg-white"
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-mono text-ui text-ink">
                          {entry.label}
                        </span>
                        <span className="block truncate text-note text-ink-mute">{entry.detail}</span>
                      </span>
                      <span
                        className={`mt-0.5 shrink-0 rounded px-1 py-0.5 text-meta font-medium uppercase ring-1 ${KIND_CLASS[entry.kind]}`}
                      >
                        {KIND_LABEL[entry.kind]}
                      </span>
                      {copied === entry.insert && (
                        <span
                          role="status"
                          className="mt-0.5 shrink-0 rounded bg-green-600 px-1 py-0.5 text-meta font-medium text-white"
                        >
                          copied
                        </span>
                      )}
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
      </div>
    </div>
  );
}
