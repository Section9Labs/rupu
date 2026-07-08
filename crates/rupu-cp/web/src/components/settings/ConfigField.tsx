// Config-form field primitives — extracted from components/ConfigEditor.tsx
// (T4 redesign) so the label + provenance-badge + input + lock-toggle row,
// the boolean toggle switch, the field-group card, and the empty-tab state
// are each a single well-contained piece instead of inline JSX repeated
// across every tab body. This is a VISUAL restyle only: every prop, the
// dotted-key wiring, the locked-readonly rendering the project Config tab
// relies on, and the `onChange` / `onToggleLock` call shapes are unchanged
// from the pre-redesign `ConfigField`.

import type { ReactNode } from 'react';
import type { KeyProvenance } from '../../lib/api';
import { cn } from '../../lib/cn';
import { Chip } from '../ui/Chip';
import { Lock, Unlock } from 'lucide-react';

// ---------------------------------------------------------------------------
// Provenance badge — which config layer resolved this key's effective value.
// ---------------------------------------------------------------------------

export const SOURCE_CLASS: Record<KeyProvenance['source'], string> = {
  global: 'bg-info-bg text-info ring-info/30',
  project: 'bg-ok-bg text-ok ring-ok/30',
  env: 'bg-warn-bg text-warn ring-warn/30',
  default: 'bg-surface text-ink-mute ring-border',
};

function ProvenanceBadge({ source }: { source: KeyProvenance['source'] }) {
  return (
    <Chip className={cn(SOURCE_CLASS[source], 'gap-1')}>
      <span aria-hidden="true" className="h-1.5 w-1.5 shrink-0 rounded-full bg-current opacity-80" />
      {source}
    </Chip>
  );
}

// ---------------------------------------------------------------------------
// Lock affordances
// ---------------------------------------------------------------------------

/** Icon-button that toggles a key's membership in the GLOBAL policy lock
 *  list. Purely visual wrapper — `aria-label` / `aria-pressed` / `title` are
 *  what the rest of the app (and tests) key off of, and are unchanged. */
function LockToggle({
  dottedKey,
  locked,
  onToggleLock,
}: {
  dottedKey: string;
  locked: boolean;
  onToggleLock: (key: string) => void;
}) {
  return (
    <button
      type="button"
      aria-label={locked ? `Unlock ${dottedKey}` : `Lock ${dottedKey}`}
      aria-pressed={locked}
      title={
        locked
          ? 'Enforced by global policy — click to unlock'
          : 'Click to lock (enforce this key globally)'
      }
      onClick={() => onToggleLock(dottedKey)}
      className={cn(
        'mt-6 inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border transition-colors',
        locked
          ? 'border-warn/30 bg-warn-bg text-warn hover:bg-warn-bg/70'
          : 'border-border bg-panel text-ink-mute hover:bg-surface-hover hover:text-ink',
      )}
    >
      {locked ? <Lock size={13} aria-hidden="true" /> : <Unlock size={13} aria-hidden="true" />}
    </button>
  );
}

/** Compact "enforced by global policy" note for the project Config tab's
 *  read-only rendering of a globally-locked key (no input, no toggle). */
function LockedReadOnlyNote() {
  return (
    <span
      className="mt-6 inline-flex shrink-0 items-center gap-1 rounded-md border border-border bg-surface px-2 py-1 text-note text-ink-dim"
      title="Enforced by global policy — cannot be overridden per-project"
    >
      <Lock size={11} aria-hidden="true" />
      enforced by global policy
    </span>
  );
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

export const fieldCls =
  'w-full max-w-sm rounded-md border border-border bg-panel px-3 py-1.5 text-sm text-ink shadow-sm ' +
  'placeholder:text-ink-mute transition-colors focus:border-brand-500 focus:outline-none ' +
  'focus:ring-2 focus:ring-brand-500/20 disabled:cursor-not-allowed disabled:opacity-60';

export const labelCls = 'text-sm font-medium text-ink';

/** Reusable pill-toggle styling for a native `<input type="checkbox">` — used
 *  by boolean `ConfigField`s and by the Settings Policy tab's per-key lock
 *  checkboxes, so every on/off control in the settings surface reads the
 *  same way. */
export const toggleInputCls = [
  'relative h-5 w-9 shrink-0 cursor-pointer appearance-none rounded-full border border-border bg-surface',
  'transition-colors checked:border-brand-600 checked:bg-brand-600',
  'before:absolute before:left-0.5 before:top-0.5 before:h-3.5 before:w-3.5 before:rounded-full',
  'before:bg-panel before:shadow-sm before:transition-transform',
  "before:content-['']",
  'checked:before:translate-x-4',
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40 focus-visible:ring-offset-1',
  'disabled:cursor-not-allowed disabled:opacity-60',
].join(' ');

// ---------------------------------------------------------------------------
// ConfigField — one form row: label + provenance badge + typed input + lock
// ---------------------------------------------------------------------------

export interface ConfigFieldProps {
  label: string;
  dottedKey: string;
  kind: 'text' | 'number' | 'boolean' | 'select';
  value: unknown;
  options?: string[];
  placeholder?: string;
  /** Optional one-line help/context copy rendered under the label. */
  help?: string;
  provenance?: KeyProvenance;
  locked: boolean;
  onChange: (key: string, value: unknown) => void;
  /** Global-scope only: toggles this key on/off the GLOBAL policy lock list. */
  onToggleLock?: (key: string) => void;
  /** Project-scope only: a locked field renders read-only (no input, no
   *  toggle button) with a 🔒 + "enforced by global policy" note. */
  lockedReadOnly?: boolean;
}

export function ConfigField({
  label,
  dottedKey,
  kind,
  value,
  options,
  placeholder,
  help,
  provenance,
  locked,
  onChange,
  onToggleLock,
  lockedReadOnly,
}: ConfigFieldProps) {
  const id = dottedKey;
  const source = provenance?.source ?? 'default';
  const readOnlyLocked = Boolean(lockedReadOnly) && locked;

  return (
    <div className="flex items-start gap-3 py-3 first:pt-0 last:pb-0">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <label htmlFor={readOnlyLocked ? undefined : id} className={labelCls}>
            {label}
          </label>
          <ProvenanceBadge source={source} />
        </div>
        {help && <p className="mt-0.5 text-note text-ink-mute">{help}</p>}

        <div className="mt-1.5">
          {readOnlyLocked ? (
            <p id={id} className="text-sm text-ink">
              {value == null || value === '' ? '—' : String(value)}
            </p>
          ) : kind === 'boolean' ? (
            <input
              id={id}
              type="checkbox"
              checked={Boolean(value)}
              onChange={(e) => onChange(dottedKey, e.target.checked)}
              className={toggleInputCls}
            />
          ) : kind === 'select' ? (
            <select
              id={id}
              value={typeof value === 'string' ? value : ''}
              onChange={(e) => onChange(dottedKey, e.target.value === '' ? undefined : e.target.value)}
              className={fieldCls}
            >
              <option value="">—</option>
              {(options ?? []).map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          ) : (
            <input
              id={id}
              type={kind === 'number' ? 'number' : 'text'}
              value={value == null ? '' : String(value)}
              placeholder={placeholder}
              onChange={(e) => {
                if (kind === 'number') {
                  const raw = e.target.value;
                  onChange(dottedKey, raw.trim() === '' ? undefined : Number(raw));
                } else {
                  onChange(dottedKey, e.target.value === '' ? undefined : e.target.value);
                }
              }}
              className={fieldCls}
            />
          )}
        </div>
      </div>

      {readOnlyLocked ? (
        <LockedReadOnlyNote />
      ) : onToggleLock ? (
        <LockToggle dottedKey={dottedKey} locked={locked} onToggleLock={onToggleLock} />
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Layout helpers shared by every tab body
// ---------------------------------------------------------------------------

/** Card that groups a related set of `ConfigField` rows under a heading (+
 *  optional helper copy). Replaces the flat, undifferentiated field list the
 *  form used to render everything into. */
export function FieldGroup({
  title,
  description,
  children,
}: {
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="rounded-lg border border-border bg-surface/40 p-4">
      <h3 className="text-sm font-semibold text-ink">{title}</h3>
      {description && <p className="mt-0.5 text-note text-ink-mute">{description}</p>}
      <div className="mt-2 divide-y divide-border/60">{children}</div>
    </section>
  );
}

export function EmptyTabState({ text }: { text: string }) {
  return (
    <div className="rounded-lg border border-dashed border-border py-10 text-center text-sm text-ink-mute">
      <p className="mx-auto max-w-sm">{text}</p>
    </div>
  );
}
