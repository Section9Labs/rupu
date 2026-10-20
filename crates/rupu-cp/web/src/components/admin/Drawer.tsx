// Drawer — a right-hand panel over the page for a text report (Explain,
// Doctor, Repair claims). DialogFrame's behaviour in a side sheet: a dimmed
// overlay, `role="dialog"` named by its title, Escape / overlay click close,
// Tab trapped inside, the first control focused on open. The owner renders it
// only while open.

import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from '../../lib/cn';

const FOCUSABLE = 'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])';

export function Drawer({
  title,
  subtitle,
  onClose,
  footer,
  children,
  className,
}: {
  title: string;
  subtitle?: ReactNode;
  onClose: () => void;
  /** Pinned under the scrolling body (the drawer's actions). */
  footer?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    panelRef.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === 'Escape') closeRef.current();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  function trapTab(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== 'Tab' || !panelRef.current) return;
    const stops = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => !(el as HTMLInputElement).disabled,
    );
    if (stops.length === 0) return;
    const at = stops.indexOf(document.activeElement as HTMLElement);
    if (e.shiftKey && at <= 0) {
      e.preventDefault();
      stops[stops.length - 1].focus();
    } else if (!e.shiftKey && (at === -1 || at === stops.length - 1)) {
      e.preventDefault();
      stops[0].focus();
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/30"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={trapTab}
        className={cn('flex h-full w-full max-w-lg flex-col border-l border-border bg-panel shadow-card', className)}
      >
        <div className="flex items-start gap-3 border-b border-border px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2 id={titleId} className="text-base font-semibold text-ink">
              {title}
            </h2>
            {subtitle && <div className="mt-0.5 text-note text-ink-dim">{subtitle}</div>}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded p-1 text-ink-mute hover:bg-surface-hover hover:text-ink"
          >
            <X size={14} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        {footer && <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">{footer}</div>}
      </div>
    </div>
  );
}

export default Drawer;
