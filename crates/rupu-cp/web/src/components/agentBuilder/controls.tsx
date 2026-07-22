// Shared control primitives for the Agent Builder card UI (Task 5 of the
// card-composer plan). Ported from the approved interactive mockup
// (agent-builder.html) — same look/behavior, namespaced `.ab-*` CSS
// (styles.css, appended after the `.sr-*` Situation Room block) so nothing
// collides with the rest of the app. Later tasks (field cards, palette,
// canvas) compose these.

import { useState, type KeyboardEvent, type ReactNode } from 'react';
import { cn } from '../../lib/cn';

/** A segmented button group. The option matching `value` gets aria-pressed. */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { label: string; value: T }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div className="ab-seg">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          aria-pressed={o.value === value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Removable chips + an add-on-Enter input + optional clickable ghost-chip
 *  suggestions (filtered to items not already in `list`). */
export function ChipsInput({
  list,
  suggestions,
  placeholder,
  onChange,
}: {
  list: string[];
  suggestions?: string[];
  placeholder?: string;
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState('');

  function add(value: string) {
    const v = value.trim();
    if (!v) return;
    onChange([...list, v]);
    setDraft('');
  }

  function remove(index: number) {
    onChange(list.filter((_, i) => i !== index));
  }

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter' && draft.trim()) {
      e.preventDefault();
      add(draft);
    }
  }

  const avail = (suggestions ?? []).filter((s) => !list.includes(s)).slice(0, 7);

  return (
    <div className="ab-chips">
      {list.map((item, i) => (
        <span className="ab-chip" key={`${item}-${i}`}>
          {item}
          <button type="button" aria-label={`remove ${item}`} onClick={() => remove(i)}>
            ×
          </button>
        </span>
      ))}
      <span className="ab-addchip">
        <input
          value={draft}
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
      </span>
      {avail.map((s) => (
        <span
          key={s}
          className={cn('ab-chip', 'ab-chip-ghost')}
          role="button"
          tabIndex={0}
          onClick={() => add(s)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              add(s);
            }
          }}
        >
          + {s}
        </span>
      ))}
    </div>
  );
}

/** A segmented scale (e.g. reasoning effort). Active option gets aria-pressed. */
export function Scale({
  options,
  value,
  onChange,
}: {
  options: string[];
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="ab-scale">
      {options.map((o) => (
        <button key={o} type="button" aria-pressed={o === value} onClick={() => onChange(o)}>
          {o}
        </button>
      ))}
    </div>
  );
}

/** An uppercase label (with an optional mono yamlKey shown in brand color) +
 *  the control + an optional hint line underneath. */
export function LabeledRow({
  label,
  yamlKey,
  hint,
  children,
}: {
  label: string;
  yamlKey?: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="ab-row">
      <label className="ab-lab">
        {label}
        {yamlKey && <span className="ab-k">{yamlKey}</span>}
      </label>
      {children}
      {hint && <div className="ab-hint">{hint}</div>}
    </div>
  );
}
