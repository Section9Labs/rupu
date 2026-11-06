// ConfirmCard — the inline warn card a destructive action asks through
// (Plan 3b mock §4, "Stop now?"): a bold question, what will be lost, then a
// safe way out (focused first) and the danger action. Escape backs out.

import { useEffect, useId, useRef, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { cn } from '../../lib/cn';

export function ConfirmCard({
  title,
  children,
  cancelLabel,
  confirmLabel,
  busyLabel,
  busy = false,
  onCancel,
  onConfirm,
  className,
}: {
  title: string;
  children: ReactNode;
  cancelLabel: string;
  confirmLabel: string;
  busyLabel?: string;
  busy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  className?: string;
}) {
  const titleId = useId();
  const cardRef = useRef<HTMLDivElement>(null);
  const cancel = useRef(onCancel);
  cancel.current = onCancel;

  useEffect(() => {
    // The safe way out takes focus, so Enter never confirms by accident.
    cardRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') cancel.current();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div
      ref={cardRef}
      role="alertdialog"
      aria-labelledby={titleId}
      className={cn('rounded-xl border border-warn/30 bg-warn-bg px-4 py-3 text-ui text-warn', className)}
    >
      <p>
        <b id={titleId} className="font-semibold">
          {title}
        </b>{' '}
        {children}
      </p>
      <div className="mt-2.5 flex justify-end gap-2">
        <Button variant="secondary" size="sm" onClick={onCancel} disabled={busy}>
          {cancelLabel}
        </Button>
        <Button variant="danger" size="sm" data-confirm onClick={onConfirm} disabled={busy}>
          {busy && busyLabel ? busyLabel : confirmLabel}
        </Button>
      </div>
    </div>
  );
}

export default ConfirmCard;
