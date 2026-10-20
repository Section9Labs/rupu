// ActionPopover — a header action that opens a small form under its button
// (mock §1 Retry, §3 Compact): a labelled `role="dialog"` panel anchored to
// the trigger's right edge. It closes on Escape, on a pointer down outside, or
// when the form calls `close`; focus moves into the panel on open and back to
// the trigger on close. A floating panel, so it may carry a shadow (the house
// rule reserves shadows for things that float).

import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { Button, type ButtonProps } from '../ui/Button';

const FOCUSABLE = 'select, input, textarea, button, [tabindex]:not([tabindex="-1"])';

export interface ActionPopoverProps {
  /** The panel's accessible name, also its heading. */
  title: string;
  /** The trigger's content. */
  label: ReactNode;
  variant?: ButtonProps['variant'];
  disabled?: boolean;
  /** Why the trigger is disabled (its tooltip). */
  disabledReason?: string;
  ariaLabel?: string;
  children: (close: () => void) => ReactNode;
}

export function ActionPopover({
  title,
  label,
  variant = 'secondary',
  disabled,
  disabledReason,
  ariaLabel,
  children,
}: ActionPopoverProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // `Button` doesn't forward refs: the trigger is the wrapper's first button.
  const focusTrigger = () => wrapRef.current?.querySelector<HTMLButtonElement>('button')?.focus();

  useEffect(() => {
    if (!open) return;
    panelRef.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false);
        focusTrigger();
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const close = () => {
    setOpen(false);
    focusTrigger();
  };

  return (
    <div ref={wrapRef} className="relative">
      <Button
        variant={variant}
        disabled={disabled}
        title={disabled ? disabledReason : undefined}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="gap-1.5"
      >
        {label}
      </Button>
      {open && (
        <div
          ref={panelRef}
          role="dialog"
          aria-labelledby={titleId}
          className="absolute right-0 top-full z-30 mt-1.5 w-80 rounded-xl border border-border-strong bg-panel px-3.5 py-3 text-left shadow-lg"
        >
          <h2 id={titleId} className="mb-2 text-ui font-semibold text-ink">
            {title}
          </h2>
          {children(close)}
        </div>
      )}
    </div>
  );
}

export default ActionPopover;
