// StepForm — the per-kind side-panel editor for the selected workflow step.
//
// Every input is controlled off `node.data`; each change emits a fresh
// StepNodeData (immutable spread). `raw_passthrough` (unmodeled step keys) is
// always carried through untouched. Validation `problems` for this node render
// in an inline alert block at the top.

import type { AgentSummary } from '../../lib/api';
import type { GraphNode, PanelCfg, PanelGate, StepKind, StepNodeData, SubStep } from '../../lib/workflowGraph';

interface StepFormProps {
  node: GraphNode;
  agents: AgentSummary[];
  onChange: (data: StepNodeData) => void;
  problems: string[];
}

const fieldCls =
  'w-full rounded-md border border-border bg-white px-2.5 py-1.5 text-[13px] text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim';
const checkLabelCls = 'flex items-center gap-2 text-[13px] text-ink';

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
};

export default function StepForm({ node, agents, onChange, problems }: StepFormProps) {
  const d = node.data;

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
        <div role="alert" className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700">
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
          {(Object.keys(KIND_LABELS) as StepKind[]).map((k) => (
            <option key={k} value={k}>
              {KIND_LABELS[k]}
            </option>
          ))}
        </select>
        <span className="mt-1 block text-[11px] text-ink-mute">
          Switching kind clears the previous kind's specific fields.
        </span>
      </label>

      {(d.kind === 'step' || d.kind === 'for_each') && (
        <LinearFields d={d} agents={agents} patch={patch} />
      )}
      {d.kind === 'parallel' && <ParallelFields d={d} agents={agents} patch={patch} />}
      {d.kind === 'panel' && <PanelFields d={d} agents={agents} patch={patch} />}

      {/* ── common: when / continue_on_error ───────────────────────── */}
      {d.kind !== 'panel' && (
        <>
          <label className="block">
            <span className={labelCls}>When (optional)</span>
            <input
              type="text"
              value={d.when ?? ''}
              onChange={(e) => patch({ when: e.target.value === '' ? undefined : e.target.value })}
              aria-label="When condition"
              className={`${fieldCls} font-mono`}
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

          <ApprovalFields d={d} patch={patch} />
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
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
}) {
  return (
    <>
      <label className="block">
        <span className={labelCls}>Agent</span>
        <AgentSelect value={d.agent} agents={agents} ariaLabel="Agent" onChange={(v) => patch({ agent: v })} />
      </label>

      <label className="block">
        <span className={labelCls}>Prompt</span>
        <textarea
          value={d.prompt ?? ''}
          onChange={(e) => patch({ prompt: e.target.value === '' ? undefined : e.target.value })}
          aria-label="Prompt"
          rows={4}
          className={`${fieldCls} resize-y`}
        />
      </label>

      {d.kind === 'for_each' && (
        <>
          <label className="block">
            <span className={labelCls}>For-each expression</span>
            <input
              type="text"
              value={d.for_each ?? ''}
              onChange={(e) => patch({ for_each: e.target.value === '' ? undefined : e.target.value })}
              aria-label="For-each expression"
              className={`${fieldCls} font-mono`}
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
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
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
          <div key={i} className="space-y-2 rounded-md border border-border bg-slate-50 p-2.5">
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={s.id}
                onChange={(e) => updateSub(i, { id: e.target.value })}
                aria-label={`Sub-step ${i + 1} id`}
                placeholder="id"
                className={`${fieldCls} font-mono`}
              />
              <button
                type="button"
                onClick={() => removeSub(i)}
                aria-label={`Remove sub-step ${i + 1}`}
                className="shrink-0 rounded-md border border-border bg-white px-2.5 py-1.5 text-[12px] font-medium text-red-700 hover:bg-red-50"
              >
                Remove
              </button>
            </div>
            <AgentSelect
              value={s.agent || undefined}
              agents={agents}
              ariaLabel={`Sub-step ${i + 1} agent`}
              onChange={(v) => updateSub(i, { agent: v ?? '' })}
            />
            <textarea
              value={s.prompt}
              onChange={(e) => updateSub(i, { prompt: e.target.value })}
              aria-label={`Sub-step ${i + 1} prompt`}
              rows={3}
              placeholder="prompt"
              className={`${fieldCls} resize-y`}
            />
          </div>
        ))}
      </div>

      <button
        type="button"
        onClick={addSub}
        className="text-[12px] font-medium text-brand-600 hover:text-brand-700"
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
}: {
  d: StepNodeData;
  agents: AgentSummary[];
  patch: (p: Partial<StepNodeData>) => void;
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
        <div className="space-y-1.5 rounded-md border border-border bg-slate-50 p-2.5">
          {panelistNames.length === 0 ? (
            <p className="text-[12px] text-ink-mute">No agents available.</p>
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
        <textarea
          value={panel.subject}
          onChange={(e) => patchPanel({ subject: e.target.value })}
          aria-label="Panel subject"
          rows={2}
          className={`${fieldCls} resize-y`}
        />
      </label>

      <label className="block">
        <span className={labelCls}>Prompt (optional)</span>
        <textarea
          value={panel.prompt ?? ''}
          onChange={(e) => patchPanel({ prompt: e.target.value === '' ? undefined : e.target.value })}
          aria-label="Panel prompt"
          rows={3}
          className={`${fieldCls} resize-y`}
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
        <div className="space-y-3 rounded-md border border-border bg-slate-50 p-2.5">
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

// ── approval (step / for_each) ───────────────────────────────────────────────

function ApprovalFields({
  d,
  patch,
}: {
  d: StepNodeData;
  patch: (p: Partial<StepNodeData>) => void;
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
        <div className="space-y-3 rounded-md border border-border bg-slate-50 p-2.5">
          <label className="block">
            <span className={labelCls}>Approval prompt</span>
            <input
              type="text"
              value={d.approvalPrompt ?? ''}
              onChange={(e) => patch({ approvalPrompt: e.target.value === '' ? undefined : e.target.value })}
              aria-label="Approval prompt"
              className={fieldCls}
            />
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
        </div>
      )}
    </div>
  );
}
