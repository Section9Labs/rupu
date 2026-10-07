// One free-form tag, validated with the same rules as the server
// (`parseTag`), with the tags already in use offered most-used first.
import { useId, useMemo, useState } from 'react';
import { cn } from '../../../lib/cn';
import { parseTag } from '../../../lib/findingQuery/grammar';
import { fuzzyScore } from '../../../lib/fuzzy';

export interface TagSuggestion {
  tag: string;
  count: number;
}

export function TagInput({
  suggestions,
  exclude = [],
  onSubmit,
  onCancel,
  label,
}: {
  suggestions: TagSuggestion[];
  exclude?: string[];
  onSubmit: (tag: string) => void;
  onCancel?: () => void;
  label: string;
}) {
  const listId = useId();
  const [text, setText] = useState('');
  const [active, setActive] = useState(-1);
  const [error, setError] = useState<string | null>(null);
  const options = useMemo(() => {
    const skip = new Set(exclude);
    const needle = text.trim();
    return suggestions
      .filter((s) => s.tag !== '' && !skip.has(s.tag))
      .map((s) => ({ ...s, hit: fuzzyScore(needle, s.tag) }))
      .filter((s) => s.hit !== null)
      .sort((a, b) => (needle === '' ? 0 : b.hit!.score - a.hit!.score) || b.count - a.count)
      .slice(0, 8);
  }, [suggestions, exclude, text]);

  const submit = (raw: string) => {
    const p = parseTag(raw);
    if (!p.ok) {
      setError(p.message);
      return;
    }
    onSubmit(p.tag);
    setText('');
    setActive(-1);
    setError(null);
  };

  return (
    <div className="relative">
      <input
        role="combobox"
        aria-label={label}
        aria-autocomplete="list"
        aria-haspopup="listbox"
        aria-expanded={options.length > 0}
        aria-controls={listId}
        aria-activedescendant={active >= 0 ? `${listId}-${active}` : undefined}
        autoFocus
        value={text}
        placeholder="tag…"
        onChange={(e) => {
          setText(e.target.value);
          setActive(-1);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            if (options.length === 0) return;
            const step = e.key === 'ArrowDown' ? 1 : -1;
            setActive((i) => (i + step + options.length + (i < 0 && step < 0 ? 1 : 0)) % options.length);
          } else if (e.key === 'Enter') {
            e.preventDefault();
            submit(active >= 0 && options[active] ? options[active].tag : text);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            onCancel?.();
          }
        }}
        className="w-44 rounded-md border border-border bg-panel px-2 py-0.5 font-mono text-note text-ink outline-none focus:ring-2 focus:ring-brand-500/50"
      />
      {error && (
        <p role="alert" className="mt-1 text-note text-err">
          {error}
        </p>
      )}
      {options.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          aria-label="Tags in use"
          className="absolute z-30 mt-1 w-56 rounded-lg border border-border bg-panel py-1 shadow-card"
        >
          {options.map((o, i) => (
            <li
              key={o.tag}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => submit(o.tag)}
              className={cn(
                'flex cursor-pointer items-center justify-between px-3 py-1 font-mono text-note',
                i === active ? 'bg-surface-active text-ink' : 'text-ink-dim hover:bg-surface-hover',
              )}
            >
              <span>{o.tag}</span>
              <span className="text-ink-mute">{o.count}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
