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
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import { useThemeColors } from '../../lib/useThemeColors';
import { KIND_ACCENT, KIND_ICON } from './kindVisuals';

/** dataTransfer key the canvas reads on drop. Exported so the canvas drop
 *  handler and the palette agree on one string. */
export const NODE_KIND_MIME = 'application/rupu-node-kind';

// Per-kind accent color — classic-only fixed hex, matching the real card's
// classic top-bar. `next` uses the themed `KIND_ACCENT` imported from
// kindVisuals above (shared with EditableStepNode).
const KIND_COLOR: Record<StepKind, string> = {
  step: '#1860f2',
  for_each: '#8b5cf6',
  parallel: '#9333ea',
  panel: '#f59e0b',
  branch: '#16a34a',
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

// branch is a newer, still-behind-flag kind — only offered from the palette
// when `workflowEditorUi === 'next'` (see Props.workflowEditorUi below).
const BRANCH_ITEM: PaletteItem = { kind: 'branch', label: 'branch', sub: 'if / then / else' };

interface Props {
  /** Click-to-add (accessible baseline + keyboard path): add at canvas center. */
  onAdd: (kind: StepKind) => void;
  /** Drag start: lets the parent track the in-flight kind for drop feedback. */
  onDragStartKind: (kind: StepKind) => void;
  /** When paused (YAML unparseable) the whole dock is inert. */
  disabled?: boolean;
  /** Workflow-editor-UI flag — the branch card renders only when 'next'.
   *  Defaults to 'classic' (no branch card) for callers that don't thread it. */
  workflowEditorUi?: WorkflowEditorUi;
  /** 'float' (default): the classic/next floating dock, unchanged. 'rail': a
   *  compact, non-absolute block for the inspector rail (Task 1) — same cards,
   *  themed accent, no drag-hint copy, sub-text moved to a `title` tooltip. */
  variant?: 'float' | 'rail';
}

export default function NodePalette({
  onAdd,
  onDragStartKind,
  disabled = false,
  workflowEditorUi = 'classic',
  variant = 'float',
}: Props) {
  const items = workflowEditorUi === 'next' ? [...ITEMS, BRANCH_ITEM] : ITEMS;
  const colors = useThemeColors();
  const handleDragStart = (kind: StepKind) => (e: DragEvent<HTMLButtonElement>) => {
    if (disabled) {
      e.preventDefault();
      return;
    }
    e.dataTransfer.setData(NODE_KIND_MIME, kind);
    e.dataTransfer.effectAllowed = 'move';
    onDragStartKind(kind);
  };

  // Inspector-rail dock (Task 1) — compact, non-absolute block meant to sit
  // inside the ~320px aside above the tabs. Reuses the themed `.wfx-pcard`/
  // `.wfx-picon` skin (same accent-tinted kind icon as the `next` float dock)
  // in a 2-col grid; the one-line "what this is" tagline drops to a `title`
  // tooltip instead of a second text row to stay compact. Same onAdd/
  // onDragStart wiring as every other variant.
  if (variant === 'rail') {
    return (
      <div data-ui="next" className="wfx-palette-rail">
        <div className="wfx-palette-rail-label">Blocks</div>
        <div className="wfx-palette-rail-grid">
          {items.map((item) => {
            const color = colors.get(KIND_ACCENT[item.kind]);
            const Icon = KIND_ICON[item.kind];
            return (
              <button
                key={item.kind}
                type="button"
                draggable={!disabled}
                disabled={disabled}
                onClick={() => onAdd(item.kind)}
                onDragStart={handleDragStart(item.kind)}
                aria-label={`Add ${item.label} node`}
                title={item.sub}
                className="wfx-pcard"
              >
                <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
                <div className="wfx-pcard-text">
                  <div className="wfx-pl">{item.label}</div>
                </div>
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  // "next" (instrument) look — a wholly separate render path so the classic
  // markup below stays byte-identical. Ported from the mockup's `.palette`/
  // `.pcard` (row-style card: accent-tinted kind icon + label/sub), styled via
  // the `.wfx-*` CSS block; only the outer dock position (bottom-left float)
  // stays Tailwind, matching the classic dock's placement.
  if (workflowEditorUi === 'next') {
    return (
      <div
        data-ui="next"
        className="pointer-events-auto absolute bottom-3 left-3 z-10 max-w-[calc(100%-1.5rem)]"
      >
        <div className="wfx-palette">
          <div className="wfx-palette-hint">Drag a card onto the canvas, or click to add at center.</div>
          <div className="wfx-palette-list">
            {items.map((item) => {
              const color = colors.get(KIND_ACCENT[item.kind]);
              const Icon = KIND_ICON[item.kind];
              return (
                <button
                  key={item.kind}
                  type="button"
                  draggable={!disabled}
                  disabled={disabled}
                  onClick={() => onAdd(item.kind)}
                  onDragStart={handleDragStart(item.kind)}
                  aria-label={`Add ${item.label} node`}
                  className="wfx-pcard"
                >
                  <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
                  <div className="wfx-pcard-text">
                    <div className="wfx-pl">{item.label}</div>
                    <div className="wfx-pd">{item.sub}</div>
                  </div>
                </button>
              );
            })}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="pointer-events-auto absolute bottom-3 left-3 z-10 max-w-[calc(100%-1.5rem)] rounded-lg border border-border bg-panel/95 px-2 py-2 shadow-card">
      <div className="px-0.5 pb-1.5 text-meta text-ink-mute">
        Drag a card onto the canvas, or click to add at center.
      </div>
      <div className="flex flex-wrap gap-1.5">
        {items.map((item) => {
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
              className="group w-[104px] cursor-grab overflow-hidden rounded-[8px] border border-border bg-panel text-left shadow-sm transition hover:ring-1 hover:ring-brand-100 active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-50"
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
