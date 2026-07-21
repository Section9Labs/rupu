// Spinner — the ONE reusable loading indicator for the CP UI (loading-ux
// pass). Standardizes the `lucide-react` `Loader2` + Tailwind `animate-spin`
// idiom that was already used ad hoc in CommandPalette/TranscriptPanel/
// status.ts, so every new "this is loading" spot looks and feels the same
// instead of falling back to bare "Loading…" text.
//
// Themed via Tailwind's `text-*` classes, which resolve to the `--c-*` CSS
// variables (see tailwind.config.ts) — no hardcoded color literal, and it
// follows both the light and dark palettes automatically.

import { Loader2 } from 'lucide-react';
import { cn } from '../../lib/cn';

export type SpinnerSize = 'sm' | 'md' | 'lg';

const SIZE_PX: Record<SpinnerSize, number> = {
  sm: 14,
  md: 20,
  lg: 28,
};

export interface SpinnerProps {
  /** `sm` (14px) / `md` (20px, default) / `lg` (28px), or an explicit pixel size. */
  size?: SpinnerSize | number;
  /** Optional text rendered beside the glyph (e.g. "Loading…", "Updating…"). */
  label?: string;
  /** Extra classes for the wrapping `<span>` — layout only; color/size are owned here. */
  className?: string;
  /** Extra classes for the glyph itself (e.g. a different tone than the default `ink-mute`). */
  iconClassName?: string;
}

/**
 * A themed spin-forever glyph, optionally paired with a label. Renders as an
 * inline `role="status"` element so screen readers announce the label (or a
 * generic "Loading" when none is given) without needing extra plumbing at
 * each call site.
 */
export function Spinner({ size = 'md', label, className, iconClassName }: SpinnerProps) {
  const px = typeof size === 'number' ? size : SIZE_PX[size];
  return (
    <span
      role="status"
      aria-label={label ?? 'Loading'}
      className={cn('inline-flex items-center gap-2 text-ink-mute', className)}
    >
      <Loader2 size={px} className={cn('animate-spin', iconClassName)} aria-hidden="true" />
      {/* No explicit font-size here — inherits from the caller's context
          (a header, a button, a bare paragraph) so this composes cleanly
          wherever it's dropped in; size via `className` on the wrapper if a
          caller needs to override. */}
      {label && <span>{label}</span>}
    </span>
  );
}

export default Spinner;
