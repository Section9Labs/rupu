// DialogFrame — the modal shell the customer dialogs share with
// CustomerFormDialog's look: a dimmed overlay, a panel with `role="dialog"`,
// Escape / overlay click ask to close (`onRequestClose` decides), Tab is
// trapped inside the panel, and the first focusable control is focused on
// open. The owner renders it only while open and returns focus to its
// trigger on close. `onEscape` overrides what Escape does (e.g. dismiss an
// inline "Discard changes?" prompt rather than ask to close again).

import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { cn } from '../../lib/cn';

const FOCUSABLE = 'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])';

export function DialogFrame({
  title,
  onRequestClose,
  onEscape,
  children,
  className,
  overlayTestId,
}: {
  title: string;
  onRequestClose: () => void;
  onEscape?: () => void;
  children: ReactNode;
  className?: string;
  overlayTestId?: string;
}) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const escapeRef = useRef(onEscape ?? onRequestClose);
  escapeRef.current = onEscape ?? onRequestClose;

  useEffect(() => {
    panelRef.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === 'Escape') escapeRef.current();
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
      data-testid={overlayTestId}
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/30 p-4 pt-[8vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onRequestClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={trapTab}
        className={cn('w-full max-w-md rounded-xl border border-border bg-panel p-5 shadow-card', className)}
      >
        <h2 id={titleId} className="text-base font-semibold text-ink">
          {title}
        </h2>
        {children}
      </div>
    </div>
  );
}

export default DialogFrame;
