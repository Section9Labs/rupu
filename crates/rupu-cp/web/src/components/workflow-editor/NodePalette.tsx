// NodePalette — the graphical drag-source dock for the workflow editor.
//
// Replaces the old plain-text "Add" buttons (§2c of the redesign): four mini
// node-card previews, each a SHRUNK, non-interactive instance of the real
// EditableStepNode look (same kind-colored top-bar + label), so "what you drag
// is what you get". Each card is a real <button> (click-to-add, the accessible
// baseline) that is ALSO HTML5-draggable (drag onto the canvas to drop at the
// pointer). Colors mirror EditableStepNode.KIND_COLOR exactly and are applied
// via inline `style` (NOT Tailwind class interpolation) so the dynamic per-kind
// coloring stays static at the Tailwind scanner level.

import type { DragEvent } from 'react';
import type { StepKind } from '../../lib/workflowGraph';

/** dataTransfer key the canvas reads on drop. Exported so the canvas drop
 *  handler and the palette agree on one string. */
export const NODE_KIND_MIME = 'application/rupu-node-kind';

// Per-kind accent color — identical to EditableStepNode.KIND_COLOR so the
// preview top-bar matches the real card.
const KIND_COLOR: Record<StepKind, string> = {
  step: '#1860f2',
  for_each: '#8b5cf6',
  parallel: '#9333ea',
  panel: '#f59e0b',
};

interface PaletteItem {
  kind: StepKind;
  /** card title — the kind keyword, matching the real node's chip. */
  label: string;
  /** one-line "what this is" tagline (§2c wireframe). */
  sub: string;
}

const ITEMS: readonly PaletteItem[] = [
  { kind: 'step', label: 'step', sub: 'one agent' },
  { kind: 'for_each', label: 'for_each', sub: 'over a list' },
  { kind: 'parallel', label: 'parallel', sub: 'N at once' },
  { kind: 'panel', label: 'panel', sub: 'review+gate' },
];

interface Props {
  /** Click-to-add (accessible baseline + keyboard path): add at canvas center. */
  onAdd: (kind: StepKind) => void;
  /** Drag start: lets the parent track the in-flight kind for drop feedback. */
  onDragStartKind: (kind: StepKind) => void;
  /** When paused (YAML unparseable) the whole dock is inert. */
  disabled?: boolean;
}

export default function NodePalette({ onAdd, onDragStartKind, disabled = false }: Props) {
  const handleDragStart = (kind: StepKind) => (e: DragEvent<HTMLButtonElement>) => {
    if (disabled) {
      e.preventDefault();
      return;
    }
    e.dataTransfer.setData(NODE_KIND_MIME, kind);
    e.dataTransfer.effectAllowed = 'move';
    onDragStartKind(kind);
  };

  return (
    <div className="pointer-events-auto absolute bottom-3 left-3 z-10 max-w-[calc(100%-1.5rem)] rounded-lg border border-border bg-white/95 px-2 py-2 shadow-card">
      <div className="px-0.5 pb-1.5 text-meta text-ink-mute">
        Drag a card onto the canvas, or click to add at center.
      </div>
      <div className="flex flex-wrap gap-1.5">
        {ITEMS.map((item) => {
          const color = KIND_COLOR[item.kind];
          return (
            <button
              key={item.kind}
              type="button"
              draggable={!disabled}
              disabled={disabled}
              onClick={() => onAdd(item.kind)}
              onDragStart={handleDragStart(item.kind)}
              aria-label={`Add ${item.label} node`}
              className="group w-[104px] cursor-grab overflow-hidden rounded-[8px] border border-border bg-white text-left shadow-sm transition hover:ring-1 hover:ring-brand-100 active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-50"
            >
              {/* colored top-bar — by KIND, mirrors the real card */}
              <div className="h-[3px] w-full" style={{ background: color }} />
              <div className="px-2 py-1.5">
                <div className="truncate text-note font-semibold text-ink">{item.label}</div>
                <div className="mt-0.5 truncate text-meta text-ink-mute">{item.sub}</div>
              </div>
            </button>
          );
        })}
      </div>
    </div>
  );
}
