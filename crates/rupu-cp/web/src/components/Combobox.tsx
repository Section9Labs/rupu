// Combobox — typeahead input backed by a fixed option list.
//
// Free-text always allowed: the user may type any string and the typed value
// flows out unchanged via `onChange`.  Selecting an option sets the value to
// `option.value`.  The dropdown closes on blur (with a small delay so click
// registers) and on outside-click.
//
// `filterOptions` is exported as a pure helper so it can be tested in a
// node environment without DOM setup.

import { useEffect, useId, useRef, useState } from 'react';

export interface ComboboxOption {
  value: string;
  label: string;
}

/**
 * Filter `options` by `query`, matching against both `label` and `value`
 * (case-insensitive substring).  Returns all options when `query` is empty.
 */
export function filterOptions(options: ComboboxOption[], query: string): ComboboxOption[] {
  if (!query) return options;
  const q = query.toLowerCase();
  return options.filter(
    (o) => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q),
  );
}

interface ComboboxProps {
  value: string;
  onChange: (value: string) => void;
  options: ComboboxOption[];
  placeholder?: string;
  disabled?: boolean;
  'aria-label'?: string;
  className?: string;
}

export default function Combobox({
  value,
  onChange,
  options,
  placeholder,
  disabled,
  'aria-label': ariaLabel,
  className,
}: ComboboxProps) {
  const uid = useId();
  const listId = `${uid}-list`;
  const containerRef = useRef<HTMLDivElement>(null);
  const blurTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clear any pending blur timer on unmount to avoid setState-after-unmount.
  useEffect(() => () => {
    if (blurTimerRef.current !== null) clearTimeout(blurTimerRef.current);
  }, []);

  // Internal text tracks what the user is typing; syncs from `value` prop.
  const [query, setQuery] = useState(value);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    setQuery(value);
  }, [value]);

  // Close dropdown on outside mousedown.
  useEffect(() => {
    function onOutside(e: MouseEvent) {
      if (!containerRef.current?.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener('mousedown', onOutside);
    return () => document.removeEventListener('mousedown', onOutside);
  }, []);

  const filtered = filterOptions(options, query);

  return (
    <div ref={containerRef} className="relative">
      <input
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={open && filtered.length > 0 ? listId : undefined}
        aria-label={ariaLabel}
        value={query}
        placeholder={placeholder}
        disabled={disabled}
        className={className}
        onChange={(e) => {
          const v = e.target.value;
          setQuery(v);
          onChange(v);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => {
          // Delay so an option's onMouseDown fires before we close.
          blurTimerRef.current = setTimeout(() => setOpen(false), 150);
        }}
      />
      {open && filtered.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          className="absolute z-10 mt-1 max-h-48 w-full overflow-y-auto rounded-md border border-border bg-white shadow-md"
        >
          {filtered.map((opt) => (
            <li
              key={opt.value}
              role="option"
              aria-selected={opt.value === value}
              onMouseDown={(e) => {
                // Prevent blur from firing before we update state.
                e.preventDefault();
                setQuery(opt.value);
                onChange(opt.value);
                setOpen(false);
              }}
              className="cursor-pointer px-2.5 py-1.5 text-[13px] text-ink hover:bg-slate-50 aria-selected:bg-brand-50"
            >
              <span className="font-medium">{opt.label}</span>
              {opt.label !== opt.value && (
                <span className="ml-2 text-ink-mute">{opt.value}</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
