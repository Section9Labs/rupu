// Single-line spotlight query bar (ported from Ghost's graph3d/SpotlightBar):
// committed tokens render as chips inside the input row; the draft token gets
// a fuzzy spotlight dropdown of keys and in-use values (icons, colors,
// counts). Generic over a field registry — the view supplies `fields`,
// `facets` (values in use) and optional per-value chip tones.
//
// Keys: ↑/↓ move · Enter/Tab accept (Enter with no active row commits the
// draft) · Escape clears the draft, then closes · Backspace on an empty draft
// pops the last chip back into the draft · `/` focuses from anywhere.
import { Search, X } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import { parseToken, tokenize, type Term } from '../../lib/findingQuery/grammar';
import type { QueryField } from '../../lib/findingQuery/fields';
import { suggest, type FacetValue, type Suggestion } from './suggest';

export interface QueryBarProps {
  value: string;
  onChange: (q: string) => void;
  fields: readonly QueryField[];
  facets?: Record<string, FacetValue[]>;
  placeholder?: string;
  valueTone?: (key: string, value: string) => string | null;
  label?: string;
}

const OP_LABEL: Record<Term['op'], string> = { eq: ':', gt: ' >', ge: ' ≥', lt: ' <', le: ' ≤' };

function chipText(t: Term): string {
  const head = t.key === 'text' ? '' : `${t.key}${OP_LABEL[t.op]} `;
  return `${t.neg ? 'not ' : ''}${head}${t.values.join(', ')}`.trim();
}

function Highlight({ text, matched }: { text: string; matched: number[] }) {
  if (matched.length === 0) return <>{text}</>;
  const set = new Set(matched);
  return (
    <>
      {Array.from(text).map((c, i) =>
        set.has(i) ? (
          <span key={i} className="font-semibold text-brand-600">
            {c}
          </span>
        ) : (
          <span key={i}>{c}</span>
        ),
      )}
    </>
  );
}

export function QueryBar({ value, onChange, fields, facets, placeholder, valueTone, label = 'Query' }: QueryBarProps) {
  const listId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState('');
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [error, setError] = useState<string | null>(null);

  const tokens = useMemo(() => {
    const t = tokenize(value);
    return t.ok ? t.tokens.map((x) => x.text) : value.split(/\s+/).filter(Boolean);
  }, [value]);
  const chips = useMemo(
    () => tokens.map((raw) => ({ raw, parsed: parseToken(raw, fields) })),
    [tokens, fields],
  );
  const options = useMemo(() => (open ? suggest(draft, fields, facets) : []), [open, draft, fields, facets]);
  // Facets can reload under the dropdown; never point at a row that is gone.
  const activeRow = active >= 0 && active < options.length ? active : -1;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName))) return;
      e.preventDefault();
      inputRef.current?.focus();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  const commit = (next: string[]) => {
    onChange(next.join(' '));
  };

  const commitDraft = (text: string) => {
    const raw = text.trim();
    if (raw === '') return;
    const p = parseToken(raw, fields);
    if (!p.ok) {
      setError(p.error.message);
      return;
    }
    const t = tokenize(raw);
    commit([...tokens, ...(t.ok ? t.tokens.map((x) => x.text) : [raw])]);
    setDraft('');
    setError(null);
    setActive(-1);
  };

  const accept = (s: Suggestion) => {
    if (s.commit) commitDraft(s.insert);
    else {
      setDraft(s.insert);
      setActive(-1);
      setError(null);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setOpen(true);
      if (options.length === 0) return;
      const step = e.key === 'ArrowDown' ? 1 : -1;
      setActive(() => (activeRow + step + options.length + (activeRow < 0 && step < 0 ? 1 : 0)) % options.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (activeRow >= 0) accept(options[activeRow]);
      else commitDraft(draft);
    } else if (e.key === 'Tab' && open && activeRow >= 0) {
      e.preventDefault();
      accept(options[activeRow]);
    } else if (e.key === 'Escape') {
      if (draft) {
        setDraft('');
        setError(null);
      } else {
        setOpen(false);
        inputRef.current?.blur();
      }
      setActive(-1);
    } else if (e.key === 'Backspace' && draft === '' && tokens.length > 0) {
      e.preventDefault();
      setDraft(tokens[tokens.length - 1]);
      commit(tokens.slice(0, -1));
    }
  };

  return (
    <div className="relative" onBlur={(e) => !e.currentTarget.contains(e.relatedTarget) && setOpen(false)}>
      <div
        className={cn(
          'flex flex-wrap items-center gap-1.5 rounded-xl border border-border bg-panel px-2.5 py-1.5 shadow-card',
          'focus-within:ring-2 focus-within:ring-brand-500/50',
        )}
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            inputRef.current?.focus();
          }
        }}
      >
        <Search size={14} className="shrink-0 text-ink-mute" aria-hidden />
        {chips.map(({ raw, parsed }, i) => {
          const term = parsed.ok ? parsed.terms[0] : null;
          const field = term ? fields.find((f) => f.key === term.key) : undefined;
          const Icon = field?.icon ?? Search;
          const tone = term && term.values.length === 1 && !term.neg ? valueTone?.(term.key, term.values[0]) : null;
          const text = term ? chipText(term) : raw;
          return (
            <span
              key={`${i}:${raw}`}
              title={parsed.ok ? raw : parsed.error.message}
              className={cn(
                'inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-mono text-note ring-1',
                !parsed.ok
                  ? 'bg-err-bg text-err ring-err/40'
                  : tone ?? 'bg-brand-500/10 text-brand-700 ring-brand-500/30',
              )}
            >
              <Icon size={12} aria-hidden />
              <span>{text}</span>
              <button
                type="button"
                aria-label={`Remove ${text}`}
                className="rounded text-ink-mute hover:text-ink"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => commit(tokens.filter((_, j) => j !== i))}
              >
                <X size={11} />
              </button>
            </span>
          );
        })}
        <input
          ref={inputRef}
          role="combobox"
          aria-label={label}
          aria-expanded={open && options.length > 0}
          aria-controls={open && options.length > 0 ? listId : undefined}
          aria-activedescendant={activeRow >= 0 ? `${listId}-${activeRow}` : undefined}
          aria-invalid={error ? true : undefined}
          value={draft}
          placeholder={tokens.length === 0 ? (placeholder ?? 'Filter… e.g. severity>=high tag:needs-poc') : ''}
          onFocus={() => setOpen(true)}
          onChange={(e) => {
            setDraft(e.target.value);
            setOpen(true);
            setActive(-1);
            setError(null);
          }}
          onKeyDown={onKeyDown}
          className="min-w-[12rem] flex-1 bg-transparent py-0.5 text-ui text-ink outline-none placeholder:text-ink-mute"
        />
      </div>
      {error && (
        <p role="alert" className="mt-1 text-note text-err">
          {error}
        </p>
      )}
      {open && options.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          className="absolute z-30 mt-1 max-h-80 w-full overflow-auto rounded-xl border border-border bg-panel py-1 shadow-card"
        >
          {options.map((s, i) => {
            const Icon = s.field?.icon ?? Search;
            const tone = s.kind === 'value' && s.field && s.value ? valueTone?.(s.field.key, s.value) : null;
            return (
              <li
                key={s.id}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={i === activeRow}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => accept(s)}
                className={cn(
                  'flex cursor-pointer items-center gap-2 px-3 py-1.5 text-ui',
                  i === activeRow ? 'bg-surface-active text-ink' : 'text-ink-dim hover:bg-surface-hover',
                )}
              >
                <span className={cn('flex h-5 w-5 items-center justify-center rounded', tone ?? 'text-ink-mute')}>
                  <Icon size={13} aria-hidden />
                </span>
                <span className="font-mono">
                  <Highlight text={s.label} matched={s.matched} />
                </span>
                {s.detail && <span className="ml-auto truncate text-note text-ink-mute">{s.detail}</span>}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

export default QueryBar;
