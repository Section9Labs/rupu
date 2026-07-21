// Skeleton — a themed, pulsing content-shaped placeholder (loading-ux pass).
// Companion to `Spinner`: use `Spinner` for "something is happening" (a
// small glyph near a header/button); use `Skeleton` for "here's roughly what
// is about to render" (a graph area, a table row, a card). Both avoid bare
// "Loading…" text and both are themed via `--c-*` tokens only.

import { type HTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export interface SkeletonProps extends HTMLAttributes<HTMLDivElement> {}

/** A single pulsing block. Compose with `className` for size/shape (e.g. `h-4 w-32 rounded`). */
export function Skeleton({ className, ...rest }: SkeletonProps) {
  return (
    <div
      className={cn('animate-pulse rounded bg-surface', className)}
      {...rest}
    />
  );
}

export default Skeleton;
