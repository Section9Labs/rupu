// StepForm — the per-kind side-panel editor for the selected workflow step.
//
// Every input is controlled off `node.data`; each change emits a fresh
// StepNodeData (immutable spread). `raw_passthrough` (unmodeled step keys) is
// always carried through untouched. Validation `problems` for this node render
// in an inline alert block at the top.

import { useRef, useState } from 'react';
import type { AgentSummary, ToolSpec } from '../../lib/api';
import {
  canConnect,
  hasInlineApproval,
  type GraphEdge,
  type GraphNode,
  type PanelCfg,
  type PanelGate,
  type StepKind,
  type StepNodeData,
  type SubStep,
} from '../../lib/workflowGraph';
import type { ExprContext } from '../../lib/workflowExpressions';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import ExpressionField from './ExpressionField';
import { Button } from '../ui/Button';
import { parseWithValue, formatWithValue } from '../../lib/withValue';

/** Context for expression fields, minus the per-field gates StepForm derives. */
type StepExprContext = Omit<ExprContext, 'isForEachPrompt' | 'isPanelField'>;

interface StepFormProps {
  node: GraphNode;
  agents: AgentSummary[];
  onChange: (data: StepNodeData) => void;
  problems: string[];
  /** Vocabulary context for the expression editors (inputs + prior steps). */
  exprContext: StepExprContext;
  /** Every node id currently in the graph — powers BranchFields' then/else
   *  target pickers. Defaults to empty (no candidates) for callers that don't
   *  thread it. */
  allNodeIds?: string[];
  /** The graph's derived edges — powers BranchFields' cycle guard (a then/else
   *  target that would close a cycle back onto an upstream node is excluded).
   *  Defaults to empty (no cycle guard) for callers that don't thread it. */
  edges?: GraphEdge[];
  /** Workflow-editor-UI flag — the "Branch (if)" kind option is only offered
   *  in the Kind <select> when 'next', UNLESS the node being edited is
   *  already a branch (an existing branch node must always be editable
   *  regardless of the flag). Defaults to 'classic'. */
  workflowEditorUi?: WorkflowEditorUi;
  /** MCP tool catalog — populates the action body's tool <select> and drives its
   *  `with:` key/value editor (the selected tool's `input_schema.properties`).
   *  Defaults to empty for callers that don't thread it. */
  tools?: ToolSpec[];
  /** Rewrite the SELECTED node's legacy inline approval (`hasInlineApproval`)
   *  into a new standalone gate step inserted before it — the editor's wiring
   *  of `convertInlineApprovalToGate` (a whole-graph transform, so StepForm
   *  can't do it via the per-node `onChange` alone). Only invoked from the
   *  "Convert to gate node" button, which itself only renders when
   *  `hasInlineApproval(d)`. Omitted (or absent) → the button doesn't render. */
  onConvertToGate?: () => void;
}

/** Build a full ExprContext for a single field from the shared step context. */
function fieldCtx(
  base: StepExprContext,
  gates: { isForEachPrompt?: boolean; isPanelField?: boolean },
): ExprContext {
  return {
    ...base,
    isForEachPrompt: gates.isForEachPrompt ?? false,
    isPanelField: gates.isPanelField ?? false,
  };
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';
const checkLabelCls = 'flex items-center gap-2 text-lead text-ink';

/** Parse a numeric input value: empty → undefined (never NaN / 0). */
function parseNum(v: string): number | undefined {
  if (v.trim() === '') return undefined;
  const n = Number(v);
  return Number.isNaN(n) ? undefined : n;
}

/** Agent option names: every known agent plus the current value if it's not in
 *  the list (so a step referencing a now-missing agent still round-trips). */
function agentOptions(agents: AgentSummary[], current: string | undefined): string[] {
  const names = agents.map((a) => a.name);
  return current && !names.includes(current) ? [current, ...names] : names;
}

const KIND_LABELS: Record<StepKind, string> = {
  step: 'Step (linear)',
  for_each: 'For-each',
  parallel: 'Parallel',
  panel: 'Panel',
  branch: 'Branch (if)',
  approval_gate: 'Approval gate',
  action: 'Action',
  // split/join (Phase 1 non-linear orchestration nodes). No dedicated form
  // body exists yet for either (Task 5-7's call) — the label exists only to
  // satisfy the exhaustive Record and keep the Kind <select> non-broken for
  // a node that already has one of these kinds.
  split: 'Split (fan-out)',
  join: 'Join (barrier)',
};

// Kinds offered in the Kind <select> only when the `next` flag is on — UNLESS
// the node being edited is already that kind, in which case it must always be
// selectable (an existing node stays fully editable regardless of the flag).
const NEXT_ONLY_KINDS = new Set<StepKind>(['branch', 'approval_gate', 'action', 'split', 'join']);

export default function StepForm({
  node,
  agents,
  onChange,
  problems,
  exprContext,
  allNodeIds = [],
  edges = [],
  workflowEditorUi = 'classic',
  tools = [],
  onConvertToGate,
}: StepFormProps) {
  const d = node.data;

  const kindOptions = (Object.keys(KIND_LABELS) as StepKind[]).filter(
    (k) => !NEXT_ONLY_KINDS.has(k) || workflowEditorUi === 'next' || d.kind === k,
  );

  // Generic field patch — spread the old data so raw_passthrough and every
  // unedited field survive.
  function patch(p: Partial<StepNodeData>): void {
    onChange({ ...d, ...p });
  }

  // Kind switch — keep id; carry shared fields only where the DESTINATION kind
  // can hold them; seed the destination's required defaults so it round-trips
  // and validates with a friendly error rather than a raw parser one.
  function switchKind(kind: StepKind): void {
    const base: StepNodeData = { id: d.id, kind };
    if (d.when !== undefined) base.when = d.when;
    if (d.continue_on_error !== undefined) base.continue_on_error = d.continue_on_error;
    if (d.actions !== undefined) base.actions = d.actions;
    if (d.raw_passthrough !== undefined) base.raw_passthrough = d.raw_passthrough;
    // agent/prompt are shared by the step + for_each forms — preserve across those.
    if (kind === 'step' || kind === 'for_each') {
      if (d.agent !== undefined) base.agent = d.agent;
      if (d.prompt !== undefined) base.prompt = d.prompt;
    }
    // approval is only editable on step/for_each/approval_gate — carry it there, drop it elsewhere.
    if (kind === 'step' || kind === 'for_each' || kind === 'approval_gate') {
      if (d.approvalRequired !== undefined) base.approvalRequired = d.approvalRequired;
      if (d.approvalPrompt !== undefined) base.approvalPrompt = d.approvalPrompt;
      if (d.approvalTimeoutSeconds !== undefined) base.approvalTimeoutSeconds = d.approvalTimeoutSeconds;
    }
    // seed destination defaults (mirrors newNodeData)
    if (kind === 'parallel') base.parallel = [];
    if (kind === 'panel') base.panel = { panelists: [], subject: '' };
    if (kind === 'branch') {
      base.condition = '';
      base.thenTargets = [];
      base.elseTargets = [];
    }
    if (kind === 'action') base.action = '';
    if (kind === 'approval_gate') {
      base.approvalRequired = true;
      base.approvalOnReject = [];
    }
    onChange(base);
  }

  return (
    <div className="space-y-4">
      {problems.length > 0 && (
        <div role="alert" className="rounded-md border border-err/30 bg-err-bg px-3 py-2 text-ui text-err">
          <ul className="list-disc space-y-0.5 pl-4">
            {problems.map((p, i) => (
              <li key={i}>{p}</li>
            ))}
          </ul>
        </div>
      )}

      {/* ── id + kind ──────────────────────────────────────────────── */}
      <label className="block">
        <span className={labelCls}>Step id</span>
        <input
          type="text"
          value={d.id}
          onChange={(e) => patch({ id: e.target.value })}
          aria-label="Step id"
          className={`${fieldCls} font-mono`}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Kind</span>
        <select
          value={d.kind}
          onChange={(e) => switchKind(e.target.value as StepKind)}
          aria-label="Step kind"
          className={fieldCls}
        >
          {kindOptions.map((k) => (
            <option key={k} value={k}>
              {KIND_LABELS[k]}
            </option>
          ))}
        </select>
        <span className="mt-1 block text-note text-ink-mute">
          Switching kind clears the previous kind's specific fields.
        </span>
      </label>

      {(d.kind === 'step' || d.kind === 'for_each') && (
        <LinearFields
          d={d}
          agents={agents}
          patch={patch}
          exprContext={exprContext}
          workflowEditorUi={workflowEditorUi}
        />
      )}
      {d.kind === 'parallel' && (
        <ParallelFields
          d={d}
          agents={agents}
          patch={patch}
          exprContext={exprContext}
          workflowEditorUi={workflowEditorUi}
        />
      )}
      {d.kind === 'panel' && (
        <PanelFields
          d={d}
          agents={agents}
          patch={patch}
          exprContext={exprContext}
          workflowEditorUi={workflowEditorUi}
        />
      )}
      {d.kind === 'branch' && (
        <BranchFields d={d} allNodeIds={allNodeIds} edges={edges} patch={patch} exprContext={exprContext} />
      )}
      {d.kind === 'approval_gate' && (
        <GateFields d={d} agents={agents} patch={patch} exprContext={exprContext} workflowEditorUi={workflowEditorUi} />
      )}
      {d.kind === 'action' && <ActionFields d={d} tools={tools} patch={patch} />}

      {/* ── common: when / continue_on_error / approval ─────────────── */}
      {/* branch/panel/approval_gate hide this block: nodeToStepObject never reads
         when/continue_on_error for branch/panel, and a gate owns its whole
         approval block via GateFields (so the shared inline-approval checkbox
         would double up). Action keeps the block (when/continue_on_error are
         valid on an action step). */}
      {d.kind !== 'panel' && d.kind !== 'branch' && d.kind !== 'approval_gate' && (
        <>
          <label className="block">
            <span className={labelCls}>When (optional)</span>
            <ExpressionField
              value={d.when ?? ''}
              onChange={(v) => patch({ when: v === '' ? undefined : v })}
              context={fieldCtx(exprContext, {})}
              ariaLabel="When condition"
            />
          </label>

          <label className={checkLabelCls}>
            <input
              type="checkbox"
              checked={d.continue_on_error ?? false}
              onChange={(e) => patch({ continue_on_error: e.target.checked ? true : undefined })}
              aria-label="Continue on error"
            />
            Continue on error
          </label>

          <ApprovalFields
            d={d}
            patch={patch}
            exprContext={exprContext}
            workflowEditorUi={workflowEditorUi}
            onConvertToGate={onConvertToGate}
          />
        </>
      )}
    </div>
  );
}

// ── Agent <select> ───────────────────────────────────────────────────────────

function AgentSelect({
  value,
  agents,
  ariaLabel,
  onChange,
}: {
  value: string | undefined;
  agents: AgentSummary[];
  ariaLabel: string;
  onChange: (v: string | undefined) => void;
}) {
  return (
    <select
      value={value ?? ''}
      onChange={(e) => onChange(e.target.value === '' ? undefined : e.target.value)}
      aria-label={ariaLabel}
      className={fieldCls}
    >
      <option value="">— select agent —</option>
      {agentOptions(agents, value).map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
    </select>
  );
}

// ── step / for_each ──────────────────────────────────────────────────────────

function LinearFields({
  d,
  agents,
  patch,
  exprContext,
  workflowEditorUi,
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
  exprContext: StepExprContext;
  workflowEditorUi: WorkflowEditorUi;
}) {
  return (
    <>
      <label className="block">
        <span className={labelCls}>Agent</span>
        <AgentSelect value={d.agent} agents={agents} ariaLabel="Agent" onChange={(v) => patch({ agent: v })} />
      </label>

      <label className="block">
        <span className={labelCls}>Prompt</span>
        <ExpressionField
          value={d.prompt ?? ''}
          onChange={(v) => patch({ prompt: v === '' ? undefined : v })}
          context={fieldCtx(exprContext, { isForEachPrompt: d.kind === 'for_each' })}
          multiline
          ariaLabel="Prompt"
          size={workflowEditorUi === 'next' ? 'large' : undefined}
        />
      </label>

      {d.kind === 'for_each' && (
        <>
          <label className="block">
            <span className={labelCls}>For-each expression</span>
            <ExpressionField
              value={d.for_each ?? ''}
              onChange={(v) => patch({ for_each: v === '' ? undefined : v })}
              context={fieldCtx(exprContext, {})}
              ariaLabel="For-each expression"
            />
          </label>
          <label className="block">
            <span className={labelCls}>Max parallel</span>
            <input
              type="number"
              min={1}
              value={d.max_parallel ?? ''}
              onChange={(e) => patch({ max_parallel: parseNum(e.target.value) })}
              aria-label="Max parallel"
              className={fieldCls}
            />
          </label>
        </>
      )}
    </>
  );
}

// ── parallel ─────────────────────────────────────────────────────────────────

function ParallelFields({
  d,
  agents,
  patch,
  exprContext,
  workflowEditorUi,
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
  exprContext: StepExprContext;
  workflowEditorUi: WorkflowEditorUi;
}) {
  const subs = d.parallel ?? [];

  function setSubs(next: SubStep[]): void {
    patch({ parallel: next });
  }
  function updateSub(i: number, p: Partial<SubStep>): void {
    setSubs(subs.map((s, j) => (j === i ? { ...s, ...p } : s)));
  }
  function addSub(): void {
    setSubs([...subs, { id: `sub-${subs.length + 1}`, agent: '', prompt: '' }]);
  }
  function removeSub(i: number): void {
    setSubs(subs.filter((_, j) => j !== i));
  }

  return (
    <div className="space-y-3">
      <label className="block">
        <span className={labelCls}>Max parallel</span>
        <input
          type="number"
          min={1}
          value={d.max_parallel ?? ''}
          onChange={(e) => patch({ max_parallel: parseNum(e.target.value) })}
          aria-label="Max parallel"
          className={fieldCls}
        />
      </label>

      <div className="space-y-3">
        {subs.map((s, i) => (
          <div key={i} className="space-y-2 rounded-md border border-border bg-surface p-2.5">
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={s.id}
                onChange={(e) => updateSub(i, { id: e.target.value })}
                aria-label={`Sub-step ${i + 1} id`}
                placeholder="id"
                className={`${fieldCls} font-mono`}
              />
              <Button
                variant="danger-outline"
                onClick={() => removeSub(i)}
                aria-label={`Remove sub-step ${i + 1}`}
                className="shrink-0 px-2.5"
              >
                Remove
              </Button>
            </div>
            <AgentSelect
              value={s.agent || undefined}
              agents={agents}
              ariaLabel={`Sub-step ${i + 1} agent`}
              onChange={(v) => updateSub(i, { agent: v ?? '' })}
            />
            <ExpressionField
              value={s.prompt}
              onChange={(v) => updateSub(i, { prompt: v })}
              context={fieldCtx(exprContext, {})}
              multiline
              ariaLabel={`Sub-step ${i + 1} prompt`}
              placeholder="prompt"
              size={workflowEditorUi === 'next' ? 'large' : undefined}
            />
          </div>
        ))}
      </div>

      <button
        type="button"
        onClick={addSub}
        className="text-ui font-medium text-brand-600 hover:text-brand-700"
      >
        Add sub-step
      </button>
    </div>
  );
}

// ── panel ────────────────────────────────────────────────────────────────────

function PanelFields({
  d,
  agents,
  patch,
  exprContext,
  workflowEditorUi,
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
  exprContext: StepExprContext;
  workflowEditorUi: WorkflowEditorUi;
}) {
  const panel: PanelCfg = d.panel ?? { panelists: [], subject: '' };

  function patchPanel(p: Partial<PanelCfg>): void {
    patch({ panel: { ...panel, ...p } });
  }
  function togglePanelist(name: string, on: boolean): void {
    const set = new Set(panel.panelists);
    if (on) set.add(name);
    else set.delete(name);
    patchPanel({ panelists: [...set] });
  }
  function patchGate(p: Partial<PanelGate>): void {
    patchPanel({ gate: { ...(panel.gate ?? {}), ...p } });
  }

  // Union of known agents + any panelist not in the agent list.
  const panelistNames = [...new Set([...agents.map((a) => a.name), ...panel.panelists])];

  return (
    <div className="space-y-3">
      <div>
        <span className={labelCls}>Panelists</span>
        <div className="space-y-1.5 rounded-md border border-border bg-surface p-2.5">
          {panelistNames.length === 0 ? (
            <p className="text-ui text-ink-mute">No agents available.</p>
          ) : (
            panelistNames.map((name) => (
              <label key={name} className={checkLabelCls}>
                <input
                  type="checkbox"
                  checked={panel.panelists.includes(name)}
                  onChange={(e) => togglePanelist(name, e.target.checked)}
                  aria-label={`Panelist ${name}`}
                />
                <span className="font-mono">{name}</span>
              </label>
            ))
          )}
        </div>
      </div>

      <label className="block">
        <span className={labelCls}>Subject</span>
        <ExpressionField
          value={panel.subject}
          onChange={(v) => patchPanel({ subject: v })}
          context={fieldCtx(exprContext, { isPanelField: true })}
          multiline
          ariaLabel="Panel subject"
          size={workflowEditorUi === 'next' ? 'large' : undefined}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Prompt (optional)</span>
        <ExpressionField
          value={panel.prompt ?? ''}
          onChange={(v) => patchPanel({ prompt: v === '' ? undefined : v })}
          context={fieldCtx(exprContext, { isPanelField: true })}
          multiline
          ariaLabel="Panel prompt"
          size={workflowEditorUi === 'next' ? 'large' : undefined}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Max parallel (optional)</span>
        <input
          type="number"
          min={1}
          value={panel.max_parallel ?? ''}
          onChange={(e) => patchPanel({ max_parallel: parseNum(e.target.value) })}
          aria-label="Panel max parallel"
          className={fieldCls}
        />
      </label>

      {/* ── gate ─────────────────────────────────────────────────── */}
      <label className={checkLabelCls}>
        <input
          type="checkbox"
          checked={panel.gate !== undefined}
          onChange={(e) => patchPanel({ gate: e.target.checked ? {} : undefined })}
          aria-label="Enable gate"
        />
        Enable gate (re-run until findings clear)
      </label>

      {panel.gate !== undefined && (
        <div className="space-y-3 rounded-md border border-border bg-surface p-2.5">
          <label className="block">
            <span className={labelCls}>Until no findings at severity or above</span>
            <select
              value={panel.gate.until_no_findings_at_severity_or_above ?? ''}
              onChange={(e) =>
                patchGate({
                  until_no_findings_at_severity_or_above: e.target.value === '' ? undefined : e.target.value,
                })
              }
              aria-label="Until no findings at severity or above"
              className={fieldCls}
            >
              <option value="">(choose severity)</option>
              {['low', 'medium', 'high', 'critical'].map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
              {/* keep an off-list value authored by hand so it still round-trips */}
              {panel.gate.until_no_findings_at_severity_or_above &&
                !['low', 'medium', 'high', 'critical'].includes(panel.gate.until_no_findings_at_severity_or_above) && (
                  <option value={panel.gate.until_no_findings_at_severity_or_above}>
                    {panel.gate.until_no_findings_at_severity_or_above}
                  </option>
                )}
            </select>
          </label>
          <label className="block">
            <span className={labelCls}>Fix with</span>
            <AgentSelect
              value={panel.gate.fix_with}
              agents={agents}
              ariaLabel="Gate fix with"
              onChange={(v) => patchGate({ fix_with: v })}
            />
          </label>
          <label className="block">
            <span className={labelCls}>Max iterations</span>
            <input
              type="number"
              value={panel.gate.max_iterations ?? ''}
              onChange={(e) => patchGate({ max_iterations: parseNum(e.target.value) })}
              aria-label="Gate max iterations"
              className={fieldCls}
            />
          </label>
        </div>
      )}
    </div>
  );
}

// ── branch ───────────────────────────────────────────────────────────────────

function BranchFields({
  d,
  allNodeIds,
  edges,
  patch,
  exprContext,
}: {
  d: StepNodeData;
  allNodeIds: string[];
  edges: GraphEdge[];
  patch: (p: Partial<StepNodeData>) => void;
  exprContext: StepExprContext;
}) {
  const thenTargets = d.thenTargets ?? [];
  const elseTargets = d.elseTargets ?? [];
  // Every other node in the graph is a candidate then/else target, EXCEPT:
  //  - the branch's own id (a self-target would be a self-loop), and
  //  - a target that would close a cycle back onto an upstream node — UNLESS
  //    it's already selected, in which case it must still render (checked) so
  //    the user can uncheck it, even if the graph is currently in a cyclic
  //    state (e.g. hand-edited YAML).
  //
  // `canConnect` also flags "already connected" (a plain chain/data-ref edge
  // already runs branch -> candidate, which is the everyday case for the
  // branch's array-adjacent successor) — that's a drag-connect duplicate
  // concern, not a cycle, so it's deliberately NOT treated as exclusion here.
  const candidates = allNodeIds.filter((id) => {
    if (id === d.id) return false;
    if (thenTargets.includes(id) || elseTargets.includes(id)) return true;
    const res = canConnect(d.id, id, { edges });
    return !(!res.ok && res.kind === 'cycle');
  });

  function toggleThen(id: string, on: boolean): void {
    const set = new Set(thenTargets);
    if (on) set.add(id);
    else set.delete(id);
    patch({ thenTargets: [...set] });
  }
  function toggleElse(id: string, on: boolean): void {
    const set = new Set(elseTargets);
    if (on) set.add(id);
    else set.delete(id);
    patch({ elseTargets: [...set] });
  }

  return (
    <div className="space-y-3">
      <label className="block">
        <span className={labelCls}>Condition</span>
        <ExpressionField
          value={d.condition ?? ''}
          onChange={(v) => patch({ condition: v })}
          context={fieldCtx(exprContext, {})}
          ariaLabel="Branch condition"
        />
      </label>

      <div>
        <span className={labelCls}>Then (true)</span>
        <div className="space-y-1.5 rounded-md border border-border bg-surface p-2.5">
          {candidates.length === 0 ? (
            <p className="text-ui text-ink-mute">No other steps available.</p>
          ) : (
            candidates.map((id) => (
              <label key={id} className={checkLabelCls}>
                <input
                  type="checkbox"
                  checked={thenTargets.includes(id)}
                  onChange={(e) => toggleThen(id, e.target.checked)}
                  aria-label={`Then target ${id}`}
                />
                <span className="font-mono">{id}</span>
              </label>
            ))
          )}
        </div>
      </div>

      <div>
        <span className={labelCls}>Else (false)</span>
        <div className="space-y-1.5 rounded-md border border-border bg-surface p-2.5">
          {candidates.length === 0 ? (
            <p className="text-ui text-ink-mute">No other steps available.</p>
          ) : (
            candidates.map((id) => (
              <label key={id} className={checkLabelCls}>
                <input
                  type="checkbox"
                  checked={elseTargets.includes(id)}
                  onChange={(e) => toggleElse(id, e.target.checked)}
                  aria-label={`Else target ${id}`}
                />
                <span className="font-mono">{id}</span>
              </label>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

// ── approval gate (standalone gate NODE) ─────────────────────────────────────

/** Read a string field off a raw on_reject step record (preserved verbatim in
 *  `approvalOnReject`), tolerating a missing/non-string value. */
function recStr(rec: Record<string, unknown>, key: string): string {
  const v = rec[key];
  return typeof v === 'string' ? v : '';
}

// Monotonic counter backing `nextRowId` — module-level so ids stay unique
// across every GateFields instance/remount, not just within one.
let rowIdSeq = 0;

/** Mint a fresh UI-only row id. Never persisted onto step data (see the
 *  `idState` doc comment in GateFields below for why). */
function nextRowId(): string {
  rowIdSeq += 1;
  return `row-${rowIdSeq}`;
}

function GateFields({
  d,
  agents,
  patch,
  exprContext,
  workflowEditorUi,
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
  exprContext: StepExprContext;
  workflowEditorUi: WorkflowEditorUi;
}) {
  const onReject = d.approvalOnReject ?? [];
  const notify = d.approvalNotify ?? [];

  // Stable UI-only row keys for the on-reject / notify lists. Both entry
  // types are plain `Record<string, unknown>` step data — deliberately NOT
  // stamped with an id field (that would leak into the serialized YAML) — so
  // list rows can't key off entry identity via the data itself, and index
  // keys are unsafe: `updateReject`/`updateNotify` replace the edited entry
  // with a NEW object (`{ ...s, ...p }`) every keystroke, so a plain
  // per-object WeakMap would also mint a new key on every edit (forcing that
  // row's own child components — e.g. WithParamsEditor — to remount and lose
  // their own in-progress draft while the user is mid-edit). Instead this
  // keeps a parallel array of ids per list, mutated in lockstep with this
  // component's own push (add) / splice (remove) calls, so a row's key
  // follows its logical position across add/remove but never changes just
  // because a sibling field on that row was edited. Rows only ever move by
  // position when an earlier row is removed — without this, React reuses the
  // shifted-in row's DOM/component instance (and its local state) for what is
  // now a different entry (the notify-list bug this fixes: a pending "add
  // param name" draft leaking onto the entry that shifted into a removed
  // row's slot). Reset wholesale whenever the node being edited changes
  // (`d.id`) — a different step's lists start fresh regardless of any
  // position-for-position length coincidence with the previous node.
  const idState = useRef({
    nodeId: d.id,
    reject: onReject.map(() => nextRowId()),
    notify: notify.map(() => nextRowId()),
  });
  if (idState.current.nodeId !== d.id) {
    idState.current = {
      nodeId: d.id,
      reject: onReject.map(() => nextRowId()),
      notify: notify.map(() => nextRowId()),
    };
  } else {
    // Defensive resync if a list's length ever changes by some path other
    // than this component's own add/remove (e.g. an external round-trip) —
    // preserves existing per-position ids and only mints new ones for the
    // delta. Not exercised by add/remove themselves, since those already
    // keep `idState` exactly in sync (see addReject/removeReject/addNotify/
    // removeNotify below).
    if (idState.current.reject.length !== onReject.length) {
      idState.current.reject = onReject.map((_, i) => idState.current.reject[i] ?? nextRowId());
    }
    if (idState.current.notify.length !== notify.length) {
      idState.current.notify = notify.map((_, i) => idState.current.notify[i] ?? nextRowId());
    }
  }

  function setOnReject(next: Record<string, unknown>[]): void {
    patch({ approvalOnReject: next });
  }
  // An on_reject entry is a raw step record that's either agent-shaped
  // (`{id, agent, prompt}`) or action-shaped (`{id, action, with}`) — the
  // backend rejects an entry carrying both (`ActionMutuallyExclusive`, P0.5 /
  // rejection-risk F.4). Detect shape off the `action` key so every row edit
  // (including a bare id edit) stays scoped to its own shape's fields.
  function isActionShapedReject(rec: Record<string, unknown>): boolean {
    return 'action' in rec;
  }
  // Belt-and-suspenders on top of kind-aware rendering below (which only ever
  // calls this with same-shape fields): strip whichever shape's fields DON'T
  // belong to this row before merging, so an update can never inject the
  // other shape's keys onto a row.
  function updateReject(i: number, p: Record<string, unknown>): void {
    setOnReject(
      onReject.map((s, j) => {
        if (j !== i) return s;
        const safe = { ...p };
        if (isActionShapedReject(s)) {
          delete safe.agent;
          delete safe.prompt;
        } else {
          delete safe.action;
          delete safe.with;
        }
        return { ...s, ...safe };
      }),
    );
  }
  /** Switch a row's shape wholesale — keeps `id`, drops the other shape's
   *  fields entirely (never merges across shapes) and seeds the target
   *  shape's required fields so it renders/round-trips cleanly. */
  function switchRejectKind(i: number, kind: 'agent' | 'action'): void {
    setOnReject(
      onReject.map((s, j) => {
        if (j !== i) return s;
        const id = recStr(s, 'id');
        return kind === 'action' ? { id, action: '', with: {} } : { id, agent: '', prompt: '' };
      }),
    );
  }
  function addReject(): void {
    idState.current.reject = [...idState.current.reject, nextRowId()];
    setOnReject([...onReject, { id: `cleanup-${onReject.length + 1}`, agent: '', prompt: '' }]);
  }
  function removeReject(i: number): void {
    idState.current.reject = idState.current.reject.filter((_, j) => j !== i);
    setOnReject(onReject.filter((_, j) => j !== i));
  }

  function setNotify(next: Record<string, unknown>[]): void {
    patch({ approvalNotify: next });
  }
  function updateNotify(i: number, p: Record<string, unknown>): void {
    setNotify(notify.map((n, j) => (j === i ? { ...n, ...p } : n)));
  }
  function addNotify(): void {
    idState.current.notify = [...idState.current.notify, nextRowId()];
    setNotify([...notify, {}]);
  }
  function removeNotify(i: number): void {
    idState.current.notify = idState.current.notify.filter((_, j) => j !== i);
    setNotify(notify.filter((_, j) => j !== i));
  }

  return (
    <div className="space-y-3">
      <label className="block">
        <span className={labelCls}>Approval prompt</span>
        {workflowEditorUi === 'next' ? (
          <ExpressionField
            value={d.approvalPrompt ?? ''}
            onChange={(v) => patch({ approvalPrompt: v === '' ? undefined : v })}
            context={fieldCtx(exprContext, {})}
            multiline
            ariaLabel="Approval prompt"
          />
        ) : (
          <input
            type="text"
            value={d.approvalPrompt ?? ''}
            onChange={(e) => patch({ approvalPrompt: e.target.value === '' ? undefined : e.target.value })}
            aria-label="Approval prompt"
            className={fieldCls}
          />
        )}
      </label>

      <label className="block">
        <span className={labelCls}>Auto approve (expression)</span>
        <input
          type="text"
          value={d.approvalAutoApprove ?? ''}
          onChange={(e) => patch({ approvalAutoApprove: e.target.value === '' ? undefined : e.target.value })}
          aria-label="Auto approve"
          placeholder="{{ inputs.trusted }}"
          className={`${fieldCls} font-mono`}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Timeout (seconds)</span>
        <input
          type="number"
          value={d.approvalTimeoutSeconds ?? ''}
          onChange={(e) => patch({ approvalTimeoutSeconds: parseNum(e.target.value) })}
          aria-label="Approval timeout seconds"
          className={fieldCls}
        />
      </label>

      <label className="block">
        <span className={labelCls}>On timeout</span>
        <select
          value={d.approvalOnTimeout ?? ''}
          onChange={(e) =>
            patch({
              approvalOnTimeout:
                e.target.value === '' ? undefined : (e.target.value as 'approve' | 'reject' | 'fail'),
            })
          }
          aria-label="On timeout"
          className={fieldCls}
        >
          <option value="">— default (fail) —</option>
          <option value="approve">approve</option>
          <option value="reject">reject</option>
          <option value="fail">fail</option>
        </select>
      </label>

      <div>
        <span className={labelCls}>On reject (cleanup steps)</span>
        <div className="space-y-3">
          {onReject.map((s, i) => {
            const isAction = isActionShapedReject(s);
            const withObj = isAction ? ((s.with as Record<string, unknown> | undefined) ?? {}) : {};
            return (
              <div key={idState.current.reject[i]} className="space-y-2 rounded-md border border-border bg-surface p-2.5">
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    value={recStr(s, 'id')}
                    onChange={(e) => updateReject(i, { id: e.target.value })}
                    aria-label={`On-reject step ${i + 1} id`}
                    placeholder="id"
                    className={`${fieldCls} font-mono`}
                  />
                  <select
                    value={isAction ? 'action' : 'agent'}
                    onChange={(e) => switchRejectKind(i, e.target.value as 'agent' | 'action')}
                    aria-label={`On-reject step ${i + 1} kind`}
                    className={`${fieldCls} shrink-0 w-auto`}
                  >
                    <option value="agent">agent</option>
                    <option value="action">action</option>
                  </select>
                  <Button
                    variant="danger-outline"
                    onClick={() => removeReject(i)}
                    aria-label={`Remove on-reject step ${i + 1}`}
                    className="shrink-0 px-2.5"
                  >
                    Remove
                  </Button>
                </div>
                {isAction ? (
                  <>
                    <input
                      type="text"
                      value={recStr(s, 'action')}
                      onChange={(e) => updateReject(i, { action: e.target.value })}
                      aria-label={`On-reject step ${i + 1} action`}
                      placeholder="action"
                      className={`${fieldCls} font-mono`}
                    />
                    <WithParamsEditor
                      value={withObj}
                      onChange={(next) => updateReject(i, { with: next })}
                      keys={Object.keys(withObj)}
                      ariaLabel={(key) => `On-reject step ${i + 1} with ${key}`}
                      emptyMessage="No parameters."
                      allowAddKey
                      addKeyAriaLabel={`On-reject step ${i + 1} new param name`}
                    />
                  </>
                ) : (
                  <>
                    <AgentSelect
                      value={recStr(s, 'agent') || undefined}
                      agents={agents}
                      ariaLabel={`On-reject step ${i + 1} agent`}
                      onChange={(v) => updateReject(i, { agent: v ?? '' })}
                    />
                    <ExpressionField
                      value={recStr(s, 'prompt')}
                      onChange={(v) => updateReject(i, { prompt: v })}
                      context={fieldCtx(exprContext, {})}
                      multiline
                      ariaLabel={`On-reject step ${i + 1} prompt`}
                      placeholder="prompt"
                      size={workflowEditorUi === 'next' ? 'large' : undefined}
                    />
                  </>
                )}
              </div>
            );
          })}
        </div>
        <button
          type="button"
          onClick={addReject}
          className="mt-2 text-ui font-medium text-brand-600 hover:text-brand-700"
        >
          Add cleanup step
        </button>
      </div>

      <div>
        <span className={labelCls}>Notify (on park)</span>
        <div className="space-y-3">
          {notify.map((n, i) => {
            const withObj = (n.with as Record<string, unknown> | undefined) ?? {};
            return (
              <div key={idState.current.notify[i]} className="space-y-2 rounded-md border border-border bg-surface p-2.5">
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    value={recStr(n, 'action')}
                    onChange={(e) => updateNotify(i, { action: e.target.value })}
                    aria-label={`Notification ${i + 1} action`}
                    placeholder="action"
                    className={`${fieldCls} font-mono`}
                  />
                  <Button
                    variant="danger-outline"
                    onClick={() => removeNotify(i)}
                    aria-label={`Remove notification ${i + 1}`}
                    className="shrink-0 px-2.5"
                  >
                    Remove
                  </Button>
                </div>
                <WithParamsEditor
                  value={withObj}
                  onChange={(next) => updateNotify(i, { with: next })}
                  keys={Object.keys(withObj)}
                  ariaLabel={(key) => `Notification ${i + 1} with ${key}`}
                  emptyMessage="No parameters."
                  allowAddKey
                  addKeyAriaLabel={`Notification ${i + 1} new param name`}
                />
              </div>
            );
          })}
        </div>
        <button
          type="button"
          onClick={addNotify}
          className="mt-2 text-ui font-medium text-brand-600 hover:text-brand-700"
        >
          Add notification
        </button>
      </div>
    </div>
  );
}

// ── shared: connector `with:` param editor ───────────────────────────────────

/** Editor for a connector's `with:` param bag — shared by `ActionFields` (an
 *  action step's own `with:`, keyed off the selected tool's schema) and
 *  `GateFields`' Notify list (each notify entry's own `with:`, which has no
 *  schema to derive keys from, so it also offers an "add param" control via
 *  `allowAddKey`). `keys` is caller-supplied (schema-derived for actions,
 *  `Object.keys(value)` for notify entries) so both call sites keep their own
 *  key-sourcing logic; this component only renders the per-key text field +
 *  (optionally) the add-key control. `ariaLabel` lets each call site keep its
 *  own accessible-name convention (`With ${key}` for actions; a
 *  per-notify-index-scoped label for notify entries, since a gate can have
 *  multiple notify entries whose param keys would otherwise collide). */
function WithParamsEditor({
  value,
  onChange,
  keys,
  ariaLabel,
  emptyMessage,
  allowAddKey = false,
  addKeyAriaLabel,
}: {
  value: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
  keys: string[];
  ariaLabel: (key: string) => string;
  emptyMessage?: string;
  allowAddKey?: boolean;
  addKeyAriaLabel?: string;
}) {
  const [newKey, setNewKey] = useState('');

  function patchKey(key: string, text: string): void {
    const next = { ...value };
    const v = parseWithValue(text);
    if (v === undefined) delete next[key];
    else next[key] = v;
    onChange(next);
  }

  function addKey(): void {
    const key = newKey.trim();
    if (key === '' || key in value) return;
    onChange({ ...value, [key]: '' });
    setNewKey('');
  }

  return (
    <div className="space-y-2 rounded-md border border-border bg-surface p-2.5">
      {keys.length === 0 ? (
        emptyMessage && <p className="text-ui text-ink-mute">{emptyMessage}</p>
      ) : (
        keys.map((key) => (
          <label key={key} className="block">
            <span className="mb-1 block text-note font-mono text-ink-dim">{key}</span>
            <input
              type="text"
              value={formatWithValue(value[key])}
              onChange={(e) => patchKey(key, e.target.value)}
              aria-label={ariaLabel(key)}
              className={`${fieldCls} font-mono`}
            />
          </label>
        ))
      )}
      {allowAddKey && (
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addKey();
              }
            }}
            aria-label={addKeyAriaLabel}
            placeholder="param name"
            className={`${fieldCls} font-mono`}
          />
          <Button variant="secondary" onClick={addKey} className="shrink-0 px-2.5">
            Add param
          </Button>
        </div>
      )}
    </div>
  );
}

// ── action (connector step) ──────────────────────────────────────────────────

/** Extract the parameter keys the selected tool declares
 *  (`input_schema.properties`), tolerating an absent/malformed schema. */
function toolParamKeys(tool: ToolSpec | undefined): string[] {
  if (!tool) return [];
  const schema = tool.input_schema;
  if (typeof schema !== 'object' || schema === null) return [];
  const props = (schema as Record<string, unknown>).properties;
  if (typeof props !== 'object' || props === null || Array.isArray(props)) return [];
  return Object.keys(props as Record<string, unknown>);
}

function ActionFields({
  d,
  tools,
  patch,
}: {
  d: StepNodeData;
  tools: ToolSpec[];
  patch: (p: Partial<StepNodeData>) => void;
}) {
  const withObj = d.with ?? {};
  const selected = tools.find((t) => t.name === d.action);
  // Tool option names: every catalog tool plus the current value if it's not in
  // the list (so a step referencing an unknown/renamed tool still round-trips).
  const names = tools.map((t) => t.name);
  const options = d.action && !names.includes(d.action) ? [d.action, ...names] : names;
  const paramKeys = toolParamKeys(selected);
  // Show any params the schema declares, PLUS any keys already set on `with:`
  // (so hand-authored / unknown-tool params stay editable and never dropped).
  const keys = [...new Set([...paramKeys, ...Object.keys(withObj)])];

  return (
    <div className="space-y-3">
      <label className="block">
        <span className={labelCls}>Tool</span>
        <select
          value={d.action ?? ''}
          onChange={(e) => patch({ action: e.target.value })}
          aria-label="Action tool"
          className={fieldCls}
        >
          <option value="">— select tool —</option>
          {options.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </label>

      <div>
        <span className={labelCls}>With (parameters)</span>
        <WithParamsEditor
          value={withObj}
          onChange={(next) => patch({ with: next })}
          keys={keys}
          ariaLabel={(key) => `With ${key}`}
          emptyMessage={d.action ? 'This tool takes no parameters.' : 'Select a tool to configure its parameters.'}
        />
      </div>
    </div>
  );
}

// ── approval (step / for_each) ───────────────────────────────────────────────

function ApprovalFields({
  d,
  patch,
  exprContext,
  workflowEditorUi,
  onConvertToGate,
}: {
  d: StepNodeData;
  patch: (p: Partial<StepNodeData>) => void;
  /** Vocabulary context for the Approval-prompt field (next-gated ExpressionField). */
  exprContext: StepExprContext;
  /** Approval prompt renders as an ExpressionField only when 'next' — it is a
   *  minijinja-rendered template (same engine/context as `prompt:`, per the
   *  orchestrator's `Approval.prompt` doc comment), so it's genuinely
   *  expression-capable; classic keeps today's plain input byte-identical. */
  workflowEditorUi: WorkflowEditorUi;
  /** Threaded from StepForm — rewrites this legacy inline approval into a
   *  standalone gate step. Renders the "Convert to gate node" button only when
   *  provided AND `hasInlineApproval(d)` (i.e. `d.approvalRequired` is set —
   *  this component only mounts for step/for_each, so that's the only gate). */
  onConvertToGate?: () => void;
}) {
  return (
    <div className="space-y-3">
      <label className={checkLabelCls}>
        <input
          type="checkbox"
          checked={d.approvalRequired ?? false}
          onChange={(e) =>
            patch(
              e.target.checked
                ? { approvalRequired: true }
                : { approvalRequired: undefined, approvalPrompt: undefined, approvalTimeoutSeconds: undefined },
            )
          }
          aria-label="Require approval"
        />
        Require approval
      </label>

      {d.approvalRequired && (
        <div className="space-y-3 rounded-md border border-border bg-surface p-2.5">
          <label className="block">
            <span className={labelCls}>Approval prompt</span>
            {workflowEditorUi === 'next' ? (
              <ExpressionField
                value={d.approvalPrompt ?? ''}
                onChange={(v) => patch({ approvalPrompt: v === '' ? undefined : v })}
                context={fieldCtx(exprContext, {})}
                ariaLabel="Approval prompt"
              />
            ) : (
              <input
                type="text"
                value={d.approvalPrompt ?? ''}
                onChange={(e) => patch({ approvalPrompt: e.target.value === '' ? undefined : e.target.value })}
                aria-label="Approval prompt"
                className={fieldCls}
              />
            )}
          </label>
          <label className="block">
            <span className={labelCls}>Approval timeout (seconds)</span>
            <input
              type="number"
              value={d.approvalTimeoutSeconds ?? ''}
              onChange={(e) => patch({ approvalTimeoutSeconds: parseNum(e.target.value) })}
              aria-label="Approval timeout seconds"
              className={fieldCls}
            />
          </label>
          {onConvertToGate && hasInlineApproval(d) && (
            <div className="border-t border-border pt-3">
              <Button variant="secondary" onClick={onConvertToGate} className="w-full">
                Convert to gate node
              </Button>
              <p className="mt-1.5 text-note text-ink-mute">
                Moves this approval onto a new standalone gate step inserted just before this one.
              </p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
