// StepForm — the per-kind side-panel editor for the selected workflow step.
//
// Every input is controlled off `node.data`; each change emits a fresh
// StepNodeData (immutable spread). `raw_passthrough` (unmodeled step keys) is
// always carried through untouched. Validation `problems` for this node render
// in an inline alert block at the top.

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
};

// Kinds offered in the Kind <select> only when the `next` flag is on — UNLESS
// the node being edited is already that kind, in which case it must always be
// selectable (an existing node stays fully editable regardless of the flag).
const NEXT_ONLY_KINDS = new Set<StepKind>(['branch', 'approval_gate', 'action']);

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

  // Kind switch — keep id + common fields (when / continue_on_error / approval /
  // actions / raw_passthrough); drop the previous kind's specific fields.
  function switchKind(kind: StepKind): void {
    const base: StepNodeData = { id: d.id, kind };
    if (d.when !== undefined) base.when = d.when;
    if (d.continue_on_error !== undefined) base.continue_on_error = d.continue_on_error;
    if (d.approvalRequired !== undefined) base.approvalRequired = d.approvalRequired;
    if (d.approvalPrompt !== undefined) base.approvalPrompt = d.approvalPrompt;
    if (d.approvalTimeoutSeconds !== undefined) base.approvalTimeoutSeconds = d.approvalTimeoutSeconds;
    if (d.actions !== undefined) base.actions = d.actions;
    if (d.raw_passthrough !== undefined) base.raw_passthrough = d.raw_passthrough;
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
            <input
              type="text"
              value={panel.gate.until_no_findings_at_severity_or_above ?? ''}
              onChange={(e) =>
                patchGate({
                  until_no_findings_at_severity_or_above: e.target.value === '' ? undefined : e.target.value,
                })
              }
              aria-label="Until no findings at severity or above"
              placeholder="medium"
              className={fieldCls}
            />
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

  function setOnReject(next: Record<string, unknown>[]): void {
    patch({ approvalOnReject: next });
  }
  function updateReject(i: number, p: Record<string, unknown>): void {
    setOnReject(onReject.map((s, j) => (j === i ? { ...s, ...p } : s)));
  }
  function addReject(): void {
    setOnReject([...onReject, { id: `cleanup-${onReject.length + 1}`, agent: '', prompt: '' }]);
  }
  function removeReject(i: number): void {
    setOnReject(onReject.filter((_, j) => j !== i));
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
          {onReject.map((s, i) => (
            <div key={i} className="space-y-2 rounded-md border border-border bg-surface p-2.5">
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={recStr(s, 'id')}
                  onChange={(e) => updateReject(i, { id: e.target.value })}
                  aria-label={`On-reject step ${i + 1} id`}
                  placeholder="id"
                  className={`${fieldCls} font-mono`}
                />
                <Button
                  variant="danger-outline"
                  onClick={() => removeReject(i)}
                  aria-label={`Remove on-reject step ${i + 1}`}
                  className="shrink-0 px-2.5"
                >
                  Remove
                </Button>
              </div>
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
            </div>
          ))}
        </div>
        <button
          type="button"
          onClick={addReject}
          className="mt-2 text-ui font-medium text-brand-600 hover:text-brand-700"
        >
          Add cleanup step
        </button>
      </div>
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

  function patchWith(key: string, value: string): void {
    const next = { ...withObj };
    if (value === '') delete next[key];
    else next[key] = value;
    patch({ with: next });
  }

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
        <div className="space-y-2 rounded-md border border-border bg-surface p-2.5">
          {keys.length === 0 ? (
            <p className="text-ui text-ink-mute">
              {d.action ? 'This tool takes no parameters.' : 'Select a tool to configure its parameters.'}
            </p>
          ) : (
            keys.map((key) => (
              <label key={key} className="block">
                <span className="mb-1 block text-note font-mono text-ink-dim">{key}</span>
                <input
                  type="text"
                  value={typeof withObj[key] === 'string' ? (withObj[key] as string) : ''}
                  onChange={(e) => patchWith(key, e.target.value)}
                  aria-label={`With ${key}`}
                  className={`${fieldCls} font-mono`}
                />
              </label>
            ))
          )}
        </div>
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
