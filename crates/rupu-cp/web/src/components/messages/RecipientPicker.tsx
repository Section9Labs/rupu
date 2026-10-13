import { useEffect, useRef, useState } from 'react';
import { ChevronDown, Megaphone, Users, Workflow, Check } from 'lucide-react';
import type { RecipientGroup } from '../../lib/messages';
import { cn } from '../../lib/cn';
import { AddressPill, Avatar } from './Avatar';

const GROUP_ICON = { Everyone: Megaphone, Participants: Users, Roles: Users, Steps: Workflow } as const;

/** The composer's "to": a button with the current recipient that opens the
 *  grouped list (everyone · participants · roles · steps). */
export function RecipientPicker({
  groups,
  value,
  onChange,
  disabled,
  defaultOpen = false,
}: {
  groups: RecipientGroup[];
  value: string;
  onChange: (to: string) => void;
  disabled?: boolean;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [open]);

  const current = groups.flatMap((g) => g.options).find((o) => o.value === value);
  return (
    <div ref={root} className="relative">
      <button
        type="button"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex h-8 items-center gap-1 rounded-lg border border-border bg-bg px-2 text-note text-ink-dim hover:bg-surface-hover disabled:opacity-50"
      >
        <span className="text-ink-mute">to</span>
        <AddressPill address={value} label={current?.label} />
        <ChevronDown size={12} className="text-ink-mute" />
      </button>
      {open && (
        <div role="listbox" aria-label="Recipient" className="absolute bottom-full left-0 z-20 mb-1 max-h-80 w-72 overflow-auto rounded-xl border border-border bg-panel p-1 shadow-lg">
          {groups.map((g) => {
            const Icon = GROUP_ICON[g.label];
            return (
              <div key={g.label} role="group" aria-label={g.label} className="py-0.5">
                <div className="flex items-center gap-1 px-2 pb-0.5 pt-1 text-[10px] font-semibold uppercase tracking-wide text-ink-mute">
                  <Icon size={10} />
                  {g.label}
                </div>
                {g.options.map((o) => (
                  <button
                    type="button"
                    role="option"
                    key={o.value}
                    aria-selected={o.value === value}
                    data-dim={o.dim ? 'true' : undefined}
                    onClick={() => {
                      onChange(o.value);
                      setOpen(false);
                    }}
                    className={cn('flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-note hover:bg-surface-hover', o.dim && 'opacity-50')}
                  >
                    {o.avatar ? <Avatar address={o.avatar} size={18} /> : <span className="w-[18px]" />}
                    <span className="min-w-0 flex-1 truncate font-medium text-ink">{o.label}</span>
                    {o.hint && <span className="shrink-0 text-meta text-ink-mute">{o.hint}</span>}
                    {o.value === value && <Check size={12} className="shrink-0 text-brand-500" />}
                  </button>
                ))}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
