// Shown while findings are selected: "N selected · Tag… · Untag… · Clear".
// Tag…/Untag… open a TagInput; the result line stays until the next action.
import { useState } from 'react';
import { TagInput, type TagSuggestion } from './tags/TagInput';

export function BulkTagBar({
  count,
  suggestions,
  onApply,
  onClear,
}: {
  count: number;
  suggestions: TagSuggestion[];
  onApply: (mode: 'add' | 'remove', tag: string) => Promise<{ message: string; ok: boolean }>;
  onClear: () => void;
}) {
  const [mode, setMode] = useState<'add' | 'remove' | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ message: string; ok: boolean } | null>(null);

  // true = applied (TagInput clears its text); false = failed (it keeps the
  // text so the tag can be retried).
  const apply = async (tag: string): Promise<boolean> => {
    if (!mode || busy) return false;
    setBusy(true);
    try {
      setResult(await onApply(mode, tag));
      setMode(null);
      return true;
    } catch (e: unknown) {
      setResult({ message: e instanceof Error ? e.message : String(e), ok: false });
      return false;
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-1">
      <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-ui">
        <span className="font-medium text-ink">{count} selected</span>
        <span className="flex-1" />
        {mode ? (
          <TagInput
            label={mode === 'add' ? 'Tag selected findings' : 'Untag selected findings'}
            suggestions={suggestions}
            busy={busy}
            onSubmit={apply}
            onCancel={() => setMode(null)}
          />
        ) : (
          <>
            <button type="button" disabled={busy} onClick={() => setMode('add')} className="rounded border border-border px-2 py-0.5 hover:bg-surface-hover">
              Tag…
            </button>
            <button type="button" disabled={busy} onClick={() => setMode('remove')} className="rounded border border-border px-2 py-0.5 hover:bg-surface-hover">
              Untag…
            </button>
          </>
        )}
        <button type="button" onClick={onClear} className="rounded px-2 py-0.5 text-ink-mute hover:text-ink">
          Clear
        </button>
      </div>
      {result && (
        <p role={result.ok ? 'status' : 'alert'} className={result.ok ? 'text-note text-ink-dim' : 'text-note text-err'}>
          {result.message}
        </p>
      )}
    </div>
  );
}
