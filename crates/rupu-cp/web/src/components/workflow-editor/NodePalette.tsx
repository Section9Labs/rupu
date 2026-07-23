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

import { useState, type DragEvent } from 'react';
import type { LucideIcon } from 'lucide-react';
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

// ── Block catalog (rail "Blocks" tab detail card) ───────────────────────────
// One entry per StepKind the palette offers. Field names in `example` mirror
// the real `Workflow`/`Step` schema (crates/rupu-orchestrator/src/workflow.rs)
// exactly — verified against `.rupu/workflows/*.yaml` samples (review-changed-
// files.yaml for for_each, code-review-panel.yaml for panel, gate-demo.yaml for
// approval_gate) plus workflowGraph.ts's own field-name comments for branch
// (no branch sample shipped in `.rupu/workflows/`, so condition/then/else are
// taken from `StepNodeData`'s documented mapping to workflow.rs `Branch`).
interface BlockCatalogEntry {
  kind: StepKind;
  label: string;
  /** one-sentence "what this is" blurb. */
  what: string;
  /** required step keys, `*`-marked in the detail card. */
  requiredFields: string[];
  /** a short, real YAML snippet. */
  example: string;
}

const BLOCK_CATALOG: Record<
  'step' | 'for_each' | 'parallel' | 'panel' | 'branch' | 'approval_gate',
  BlockCatalogEntry
> = {
  step: {
    kind: 'step',
    label: 'step',
    what: 'Runs one agent against a prompt and records its output for later steps to reference.',
    requiredFields: ['agent', 'prompt'],
    example: '- id: review\n  agent: code-reviewer\n  prompt: "Review the diff."',
  },
  for_each: {
    kind: 'for_each',
    label: 'for_each',
    what: "Runs one agent's prompt once per item in a list, up to max_parallel at a time.",
    requiredFields: ['agent', 'for_each', 'prompt'],
    example:
      '- id: review_each\n  agent: code-reviewer\n  for_each: "{{ inputs.files }}"\n  max_parallel: 4\n  prompt: "Review {{ item }}."',
  },
  parallel: {
    kind: 'parallel',
    label: 'parallel',
    what: 'Runs a fixed set of named sub-steps concurrently, each with its own agent and prompt.',
    requiredFields: ['parallel'],
    example:
      '- id: fanout\n  parallel:\n    - id: a\n      agent: writer\n      prompt: "..."\n    - id: b\n      agent: code-reviewer\n      prompt: "..."',
  },
  panel: {
    kind: 'panel',
    label: 'panel',
    what: 'Runs several agents ("panelists") against one subject in parallel and aggregates their findings.',
    requiredFields: ['panelists', 'subject'],
    example:
      '- id: review\n  panel:\n    panelists: [security-reviewer, performance-reviewer]\n    subject: "{{ inputs.diff }}"',
  },
  branch: {
    kind: 'branch',
    label: 'branch',
    what: 'Evaluates a condition and routes the run to a then/else set of next steps — no agent runs on this node itself.',
    requiredFields: ['condition'],
    example: '- id: route\n  branch:\n    condition: "{{ steps.assess.output == \'clean\' }}"\n    then: [ship]\n    else: [fix]',
  },
  approval_gate: {
    kind: 'approval_gate',
    label: 'gate',
    what: 'Pauses the run for a human approve/reject decision before continuing; can auto-approve from an expression.',
    requiredFields: ['prompt'],
    example: '- id: ship_gate\n  approval:\n    prompt: "Approve to continue?"\n    timeout_seconds: 86400\n    on_timeout: reject',
  },
};

/** One required field parsed off a tool's `input_schema` (JSON Schema). */
interface SchemaField {
  name: string;
  type?: string;
  description?: string;
}

/** `ToolSpec.input_schema` is `unknown` (server-driven JSON Schema) — parse
 *  defensively. Returns only the `required[]` names (the detail card's
 *  "required fields" list); an unparseable/missing schema yields `[]`, which
 *  the caller renders as a "parameters from the tool schema" fallback note. */
function requiredFieldsFromSchema(schema: unknown): SchemaField[] {
  if (typeof schema !== 'object' || schema === null) return [];
  const s = schema as Record<string, unknown>;
  const requiredRaw = s.required;
  const required = Array.isArray(requiredRaw) ? requiredRaw.filter((r): r is string => typeof r === 'string') : [];
  if (required.length === 0) return [];
  const props = typeof s.properties === 'object' && s.properties !== null ? (s.properties as Record<string, unknown>) : {};
  return required.map((name) => {
    const raw = props[name];
    const p = typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : {};
    return {
      name,
      type: typeof p.type === 'string' ? p.type : undefined,
      description: typeof p.description === 'string' ? p.description : undefined,
    };
  });
}

/** Case-insensitive substring match used by the rail filter field. An empty
 *  query matches everything. */
function matchesFilter(text: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return text.toLowerCase().includes(q);
}

/** What's currently selected in the rail palette (Blocks tab): either a kind
 *  card or a connector tool chip. Selecting no longer instantly adds the node
 *  (the one deliberate behavior change of this redesign) — it shows a detail
 *  card instead; "Add to canvas" (or drag, unchanged) commits it. */
type SelectedPaletteKey = { type: 'block'; kind: StepKind } | { type: 'tool'; name: string };

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

interface NodeDetailProps {
  icon: LucideIcon;
  accentColor: string;
  title: string;
  kindLabel: string;
  what: string;
  requiredFields: SchemaField[];
  /** Shown INSTEAD of the required-fields list when it's empty (a connector
   *  with no `required[]` in its schema — never shown for blocks, which
   *  always have at least one required field). */
  noSchemaNote?: string;
  example: string;
  onAdd: () => void;
}

/** The Blocks-tab detail card (rail variant only): title + kind badge, blurb,
 *  `*`-marked required fields, a short example, and the "Add to canvas" CTA.
 *  Rendered for whichever palette item is currently `selected` — a block kind
 *  (from `BLOCK_CATALOG`) or a connector tool (parsed from `ToolSpec`). */
function NodeDetail({
  icon: Icon,
  accentColor,
  title,
  kindLabel,
  what,
  requiredFields,
  noSchemaNote,
  example,
  onAdd,
}: NodeDetailProps) {
  return (
    <div className="wfx-palette-detail" role="region" aria-label={`${title} details`}>
      <div className="wfx-detail-h">
        <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color: accentColor }} aria-hidden />
        <span className="wfx-detail-title">{title}</span>
        <span className="wfx-detail-badge" style={{ color: accentColor, borderColor: accentColor }}>
          {kindLabel}
        </span>
      </div>
      <p className="wfx-detail-blurb">{what}</p>
      {requiredFields.length > 0 ? (
        <ul className="wfx-detail-reqs">
          {requiredFields.map((f) => (
            <li key={f.name} className="wfx-detail-req">
              <code>{f.name}</code>
              <span className="wfx-detail-req-star" aria-hidden>
                *
              </span>
              {(f.type || f.description) && (
                <span className="wfx-detail-req-meta">
                  {f.type ?? ''}
                  {f.type && f.description ? ' — ' : ''}
                  {f.description ?? ''}
                </span>
              )}
            </li>
          ))}
        </ul>
      ) : (
        noSchemaNote && <p className="wfx-detail-note">{noSchemaNote}</p>
      )}
      <pre className="wfx-detail-example">{example}</pre>
      <button type="button" className="wfx-detail-addbtn" onClick={onAdd}>
        Add to canvas
      </button>
    </div>
  );
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
  // Rail-only state (declared unconditionally — Rules of Hooks — even though
  // only the `variant === 'rail'` branch below reads them). Selecting a chip
  // no longer instantly adds it (the one deliberate behavior change of this
  // redesign): it shows a detail card; "Add to canvas" (or drag, unchanged)
  // commits the same `onAdd` the chip click used to call directly.
  const [selected, setSelected] = useState<SelectedPaletteKey | null>(null);
  const [filterQuery, setFilterQuery] = useState('');
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

  // Inspector-rail dock (the "Blocks" tab, Flow Designer rail redesign) —
  // compact, non-absolute block meant to own the FULL rail height (it's a tab
  // now, not a slot competing with the editor). Reuses the themed `.wfx-pcard`/
  // `.wfx-picon` skin (same accent-tinted kind icon as the `next` float dock)
  // in a dense 3-col grid, plus a substring filter and a detail card for
  // whichever chip is `selected`. Drag-to-place is UNCHANGED (still fires
  // `onDragStart` → the same DnD mime the canvas reads); only the CLICK path
  // changed from instant-add to select-then-"Add to canvas".
  if (variant === 'rail') {
    const filteredItems = items.filter((item) => matchesFilter(item.label, filterQuery));
    const filteredGroups = connectorGroups
      .map((g) => ({ ...g, tools: g.tools.filter((t) => matchesFilter(t.name, filterQuery)) }))
      .filter((g) => g.tools.length > 0);

    const selectedBlock = selected?.type === 'block' ? BLOCK_CATALOG[selected.kind as keyof typeof BLOCK_CATALOG] : undefined;
    const selectedTool = selected?.type === 'tool' ? tools?.find((t) => t.name === selected.name) : undefined;

    const commitAdd = (kind: StepKind, seed?: Partial<StepNodeData>) => {
      if (seed) onAdd(kind, seed);
      else onAdd(kind);
      setSelected(null);
    };

    return (
      <div data-ui="next" className="wfx-palette-rail">
        <div className="wfx-palette-rail-label">Blocks</div>
        <input
          type="search"
          value={filterQuery}
          onChange={(e) => setFilterQuery(e.target.value)}
          disabled={disabled}
          placeholder="Filter blocks & actions…"
          aria-label="Filter blocks and actions"
          className="wfx-palette-filter"
        />
        <div className="wfx-palette-rail-grid">
          {filteredItems.map((item) => {
            const color = colors.get(KIND_ACCENT[item.kind]);
            const Icon = KIND_ICON[item.kind];
            const isSelected = selected?.type === 'block' && selected.kind === item.kind;
            return (
              <button
                key={item.kind}
                type="button"
                draggable={!disabled}
                disabled={disabled}
                onClick={() => setSelected({ type: 'block', kind: item.kind })}
                onDragStart={handleDragStart(item.kind)}
                aria-label={`Add ${item.label} node`}
                aria-pressed={isSelected}
                title={item.sub}
                className={isSelected ? 'wfx-pcard wfx-pcard-selected' : 'wfx-pcard'}
              >
                <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
                <div className="wfx-pcard-text">
                  <div className="wfx-pl">{item.label}</div>
                </div>
              </button>
            );
          })}
        </div>
        {filteredGroups.length > 0 && (
          <div className="wfx-palette-connectors">
            {filteredGroups.map((group) => {
              const Icon = KIND_ICON.action;
              const color = colors.get(KIND_ACCENT.action);
              return (
                <div key={group.label} className="wfx-palette-group">
                  <div className="wfx-palette-group-label">{group.label}</div>
                  {group.tools.map((tool) => {
                    const isSelected = selected?.type === 'tool' && selected.name === tool.name;
                    return (
                      <button
                        key={tool.name}
                        type="button"
                        draggable={!disabled}
                        disabled={disabled}
                        onClick={() => setSelected({ type: 'tool', name: tool.name })}
                        onDragStart={handleDragStart('action', { action: tool.name })}
                        aria-label={`Add ${tool.name} action`}
                        aria-pressed={isSelected}
                        title={tool.description || tool.name}
                        className={isSelected ? 'wfx-pcard wfx-pcard-selected' : 'wfx-pcard'}
                      >
                        <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
                        <div className="wfx-pcard-text">
                          <div className="wfx-pl font-mono">{tool.name}</div>
                        </div>
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
        )}
        {selectedBlock && (
          <NodeDetail
            icon={KIND_ICON[selectedBlock.kind]}
            accentColor={colors.get(KIND_ACCENT[selectedBlock.kind])}
            title={selectedBlock.label}
            kindLabel={selectedBlock.label}
            what={selectedBlock.what}
            requiredFields={selectedBlock.requiredFields.map((name) => ({ name }))}
            example={selectedBlock.example}
            onAdd={() => commitAdd(selectedBlock.kind)}
          />
        )}
        {selectedTool && (
          <NodeDetail
            icon={KIND_ICON.action}
            accentColor={colors.get(KIND_ACCENT.action)}
            title={selectedTool.name}
            kindLabel="action"
            what={selectedTool.description || 'Calls an MCP connector tool.'}
            requiredFields={requiredFieldsFromSchema(selectedTool.input_schema)}
            noSchemaNote="No required parameters declared — parameters come from the tool schema."
            example={`action: ${selectedTool.name}\nwith: { … }`}
            onAdd={() => commitAdd('action', { action: selectedTool.name })}
          />
        )}
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
