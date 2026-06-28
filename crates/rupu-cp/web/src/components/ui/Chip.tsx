import { cn } from '../../lib/cn';

export function Chip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1',
        className,
      )}
    >
      {children}
    </span>
  );
}
