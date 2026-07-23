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
import type { StepKind, StepNodeData } from '../../lib/workflowGraph';
import type { ToolSpec } from '../../lib/api';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import { useThemeColors } from '../../lib/useThemeColors';
import { KIND_ACCENT, KIND_ICON } from './kindVisuals';

/** dataTransfer key the canvas reads on drop. Exported so the canvas drop
 *  handler and the palette agree on one string. */
export const NODE_KIND_MIME = 'application/rupu-node-kind';

/** dataTransfer key carrying a JSON `Partial<StepNodeData>` seed — set by
 *  connector ACTION cards so the dropped node arrives pre-filled with the tool
 *  name. Absent for the plain kind cards (whose drop yields a bare node). */
export const NODE_SEED_MIME = 'application/rupu-node-seed';

// Per-kind accent color — classic-only fixed hex, matching the real card's
// classic top-bar. `next` uses the themed `KIND_ACCENT` imported from
// kindVisuals above (shared with EditableStepNode).
const KIND_COLOR: Record<StepKind, string> = {
  step: '#1860f2',
  for_each: '#8b5cf6',
  parallel: '#9333ea',
  panel: '#f59e0b',
  branch: '#16a34a',
  // gate/action are `next`-only cards — these classic hexes exist only to
  // satisfy the exhaustive Record; the classic dock never renders them.
  approval_gate: '#a855f7',
  action: '#0ea5e9',
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

// branch + gate are newer, still-behind-flag kinds — only offered from the
// palette when `workflowEditorUi === 'next'` (see Props.workflowEditorUi below).
const BRANCH_ITEM: PaletteItem = { kind: 'branch', label: 'branch', sub: 'if / then / else' };
const GATE_ITEM: PaletteItem = { kind: 'approval_gate', label: 'gate', sub: 'human approval' };

/** One connector-action card: a tool from the MCP catalog. Dropping/clicking it
 *  adds an `action` node seeded with `{ action: <tool name> }`. */
interface ConnectorGroup {
  /** the dotted-prefix heading (e.g. `scm`, `issues`, `github`). */
  label: string;
  tools: ToolSpec[];
}

/** Group the tool catalog by its first dotted segment (`scm.prs.create` → `scm`,
 *  `issues.comment` → `issues`, `github.*`/`gitlab.*` → per-platform). Groups
 *  and the tools within them keep catalog order so the dock is deterministic. */
function groupConnectors(tools: ToolSpec[]): ConnectorGroup[] {
  const groups: ConnectorGroup[] = [];
  const byLabel = new Map<string, ConnectorGroup>();
  for (const t of tools) {
    const label = t.name.split('.')[0] || 'other';
    let g = byLabel.get(label);
    if (!g) {
      g = { label, tools: [] };
      byLabel.set(label, g);
      groups.push(g);
    }
    g.tools.push(t);
  }
  return groups;
}

interface Props {
  /** Click-to-add (accessible baseline + keyboard path): add at canvas center.
   *  `seed` pre-fills kind-specific fields (connector cards seed `action`). */
  onAdd: (kind: StepKind, seed?: Partial<StepNodeData>) => void;
  /** Drag start: lets the parent track the in-flight kind for drop feedback. */
  onDragStartKind: (kind: StepKind) => void;
  /** When paused (YAML unparseable) the whole dock is inert. */
  disabled?: boolean;
  /** Workflow-editor-UI flag — the branch/gate + connector cards render only
   *  when 'next'. Defaults to 'classic' (kind cards only) for callers that
   *  don't thread it. */
  workflowEditorUi?: WorkflowEditorUi;
  /** 'float' (default): the classic/next floating dock, unchanged. 'rail': a
   *  compact, non-absolute block for the inspector rail (Task 1) — same cards,
   *  themed accent, no drag-hint copy, sub-text moved to a `title` tooltip. */
  variant?: 'float' | 'rail';
  /** MCP tool catalog — grouped into connector ACTION cards (`next` only). */
  tools?: ToolSpec[];
}

export default function NodePalette({
  onAdd,
  onDragStartKind,
  disabled = false,
  workflowEditorUi = 'classic',
  variant = 'float',
  tools,
}: Props) {
  const items = workflowEditorUi === 'next' ? [...ITEMS, BRANCH_ITEM, GATE_ITEM] : ITEMS;
  // Connector cards are `next`-only (classic dock stays byte-stable).
  const connectorGroups = workflowEditorUi === 'next' && tools ? groupConnectors(tools) : [];
  const colors = useThemeColors();
  const handleDragStart =
    (kind: StepKind, seed?: Partial<StepNodeData>) => (e: DragEvent<HTMLButtonElement>) => {
      if (disabled) {
        e.preventDefault();
        return;
      }
      e.dataTransfer.setData(NODE_KIND_MIME, kind);
      if (seed) e.dataTransfer.setData(NODE_SEED_MIME, JSON.stringify(seed));
      e.dataTransfer.effectAllowed = 'move';
      onDragStartKind(kind);
    };

  // Shared connector-card section (both the `next` float dock and the rail),
  // rendered after the kind cards. Each card drops an `action` node seeded with
  // its tool name. Nothing renders in classic (connectorGroups is empty).
  const connectorSection =
    connectorGroups.length > 0 ? (
      <div className="wfx-palette-connectors">
        {connectorGroups.map((group) => {
          const Icon = KIND_ICON.action;
          const color = colors.get(KIND_ACCENT.action);
          return (
            <div key={group.label} className="wfx-palette-group">
              <div className="wfx-palette-group-label">{group.label}</div>
              {group.tools.map((tool) => (
                <button
                  key={tool.name}
                  type="button"
                  draggable={!disabled}
                  disabled={disabled}
                  onClick={() => onAdd('action', { action: tool.name })}
                  onDragStart={handleDragStart('action', { action: tool.name })}
                  aria-label={`Add ${tool.name} action`}
                  title={tool.description || tool.name}
                  className="wfx-pcard"
                >
                  <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
                  <div className="wfx-pcard-text">
                    <div className="wfx-pl font-mono">{tool.name}</div>
                  </div>
                </button>
              ))}
            </div>
          );
        })}
      </div>
    ) : null;

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
        {connectorSection}
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
          {connectorSection}
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
