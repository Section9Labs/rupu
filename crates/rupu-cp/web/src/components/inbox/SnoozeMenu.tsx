// SnoozeMenu — the "put this down for a while" control. A small text button that
// opens a popover of the three fixed durations (inboxStore.SNOOZE_CHOICES). A
// floating menu, so a soft shadow is allowed here (the house rule reserves
// shadows for things that float). Closes on select, on outside pointer, or Esc.

import { useEffect, useId, useRef, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { SNOOZE_CHOICES, type SnoozeDuration } from '../../lib/dashboard/inboxStore';

export function SnoozeMenu({
  onSnooze,
  disabled,
}: {
  onSnooze: (dur: SnoozeDuration) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-0.5 rounded px-1.5 py-1 text-note font-medium text-ink-mute transition-colors hover:text-ink disabled:opacity-50"
      >
        Snooze
        <ChevronDown size={12} aria-hidden />
      </button>
      {open && (
        <div
          id={menuId}
          role="menu"
          className="absolute right-0 top-full z-20 mt-1 w-40 overflow-hidden rounded-lg border border-border-strong bg-panel py-1 shadow-lg"
        >
          {SNOOZE_CHOICES.map((c) => (
            <button
              key={c.key}
              type="button"
              role="menuitem"
              onClick={() => {
                onSnooze(c.key);
                setOpen(false);
              }}
              className="block w-full px-3 py-1.5 text-left text-note text-ink transition-colors hover:bg-surface"
            >
              {c.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default SnoozeMenu;
