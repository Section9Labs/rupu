// A finding's tags, editable: ✕ removes, "+ tag" adds (TagInput). Read-only
// with a reason when the finding's tag log can't be read (decision A).
import { Plus, X } from 'lucide-react';
import { useRef, useState } from 'react';
import { apiErrorMessage } from '../../../lib/api';
import { TagInput, type TagSuggestion } from './TagInput';

export function TagEditor({
  tags,
  suggestions,
  disabledReason,
  onAdd,
  onRemove,
}: {
  tags: string[];
  suggestions: TagSuggestion[];
  disabledReason?: string | null;
  onAdd: (tag: string) => Promise<void>;
  onRemove: (tag: string) => Promise<void>;
}) {
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A ref as well as state: a second submit in the same tick must see the first.
  const inFlight = useRef(false);

  /** Runs one change; false when it failed (the error is shown) or another is in flight. */
  const run = async (f: () => Promise<void>): Promise<boolean> => {
    if (inFlight.current) return false;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await f();
      return true;
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
      return false;
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Tags">
      {tags.map((t) => (
        <span
          key={t}
          className="inline-flex items-center gap-1 rounded bg-surface px-1.5 py-0.5 font-mono text-note text-ink ring-1 ring-border"
        >
          {t}
          {!disabledReason && (
            <button
              type="button"
              aria-label={`Remove tag ${t}`}
              disabled={busy}
              onClick={() => void run(() => onRemove(t))}
              className="rounded text-ink-mute hover:text-ink disabled:opacity-50"
            >
              <X size={11} />
            </button>
          )}
        </span>
      ))}
      {disabledReason ? (
        <span className="text-note text-ink-mute" title={disabledReason}>
          tags read-only
        </span>
      ) : adding ? (
        <TagInput
          label="Add tag"
          suggestions={suggestions}
          exclude={tags}
          busy={busy}
          onCancel={() => setAdding(false)}
          onSubmit={(t) =>
            run(async () => {
              await onAdd(t);
              setAdding(false);
            })
          }
        />
      ) : (
        <button
          type="button"
          disabled={busy}
          onClick={() => setAdding(true)}
          className="inline-flex items-center gap-1 rounded border border-dashed border-border px-1.5 py-0.5 text-note text-ink-mute hover:text-ink"
        >
          <Plus size={11} aria-hidden />
          tag
        </button>
      )}
      {error && (
        <p role="alert" className="w-full text-note text-err">
          {error}
        </p>
      )}
    </div>
  );
}
