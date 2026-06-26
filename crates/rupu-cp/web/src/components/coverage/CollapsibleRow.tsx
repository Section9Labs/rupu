// Generic accordion row: a clickable header (always visible) + collapsible
// body. Open state is controlled by the parent so expand/collapse-all works.
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/cn';

export default function CollapsibleRow({
  open,
  onToggle,
  header,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  header: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="px-4 py-3">
      <button onClick={onToggle} className="flex w-full items-start gap-2 text-left">
        <ChevronRight
          size={14}
          className={cn('mt-0.5 shrink-0 text-ink-mute transition-transform', open && 'rotate-90')}
        />
        <span className="min-w-0 flex-1">{header}</span>
      </button>
      {open && <div className="mt-2 pl-6">{children}</div>}
    </div>
  );
}
