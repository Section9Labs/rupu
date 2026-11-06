// Controlled xyflow graphs rebuild their node objects from a model on every
// update. A node object without `measured` makes xyflow drop its handle bounds
// until it re-measures, so every edge unmounts and remounts for a frame. The
// graph keeps the sizes xyflow reports and hands them back on each rebuild.

import type { Node, NodeChange } from '@xyflow/react';

export interface Size {
  width: number;
  height: number;
}

/** `prev` with the sizes `changes` measured; `prev` itself when none changed. */
export function mergeMeasured<N extends Node>(
  prev: ReadonlyMap<string, Size>,
  changes: readonly NodeChange<N>[],
): ReadonlyMap<string, Size> {
  let next: Map<string, Size> | null = null;
  for (const c of changes) {
    if (c.type !== 'dimensions' || !c.dimensions) continue;
    const { width, height } = c.dimensions;
    const had = (next ?? prev).get(c.id);
    if (had && had.width === width && had.height === height) continue;
    next ??= new Map(prev);
    next.set(c.id, { width, height });
  }
  return next ?? prev;
}

