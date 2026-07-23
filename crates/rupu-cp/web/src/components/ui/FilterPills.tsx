// FilterPills — "show me a subset" control. Rounded-full, single-select,
// brand-filled active pill. Every group leads with an All option — the
// CALLER includes `{value:'all', label:'All'}` first; this component never
// injects it (some pages have no neutral "all" concept, e.g. an exclusive
// choice with no default). Replaces the lifecycle rounded-md tabs / trigger
// pill rows / Situation-Room ink pills (≥4 copies).

import { cn } from '../../lib/cn';

export interface FilterPillOption {
  value: string;
  label: string;
}

export interface FilterPillsProps {
  /** Optional tiny uppercase group label rendered before the pills. */
  label?: string;
  options: FilterPillOption[];
  value: string;
  onChange: (value: string) => void;
}

export function FilterPills({ label, options, value, onChange }: FilterPillsProps) {
  return (
    <div className="inline-flex items-center gap-1.5">
      {label && (
        <span className="text-meta font-medium uppercase tracking-wide text-ink-mute">
          {label}
        </span>
      )}
      <div className="inline-flex flex-wrap items-center gap-1">
        {options.map((opt) => {
          const active = opt.value === value;
          return (
            <button
              key={opt.value}
              type="button"
              aria-pressed={active}
              onClick={() => onChange(opt.value)}
              className={cn(
                'rounded-full border px-2.5 py-1 text-note font-medium transition-colors whitespace-nowrap',
                active
                  ? 'border-brand-600 bg-brand-600 text-white font-semibold'
                  : 'border-border bg-panel text-ink-dim hover:text-ink',
              )}
            >
              {opt.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

export default FilterPills;
