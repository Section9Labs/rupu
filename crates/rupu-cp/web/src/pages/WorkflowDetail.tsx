// Workflow detail — header (name/scope/description), a vertical STEPS spine
// (id + agent + for_each/parallel hints), and the raw YAML. Route:
// /workflows/:name. The parsed `workflow` object is typed loosely on the wire,
// so we narrow each field we read defensively.

import { lazy, Suspense, useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft, Pencil, Trash2 } from 'lucide-react';
import { api, type AgentSummary, type WorkflowDetail } from '../lib/api';
import { cn } from '../lib/cn';
import { ScopeChip } from './Workflows';
import CodeHighlight from '../components/CodeHighlight';
import CodeEditor from '../components/CodeEditor';
import LauncherSheet from '../components/LauncherSheet';

// Lazy so the @xyflow/react canvas (and the rest of the visual editor) stays out
// of the main bundle — only fetched when the operator opens the Graph tab.
const WorkflowEditor = lazy(() => import('../components/workflow-editor/WorkflowEditor'));

type EditorView = 'graph' | 'yaml';

// ── Loose narrowing helpers ──────────────────────────────────────────────
// The backend hands back `workflow: Record<string, unknown>`; we read only the
// few fields the UI needs and tolerate anything missing or oddly-shaped.

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
}

interface ParsedStep {
  id?: string;
  agent?: string;
  forEach?: string;
  parallel?: boolean;
  kind?: string;
}

function parseStep(raw: unknown): ParsedStep {
  if (typeof raw !== 'object' || raw === null) return {};
  const o = raw as Record<string, unknown>;
  const forEachRaw = o.for_each ?? o.forEach;
  return {
    id: asString(o.id),
    agent: asString(o.agent),
    forEach: asString(forEachRaw),
    parallel: o.parallel === true,
    kind: asString(o.kind),
  };
}

function readSteps(workflow: Record<string, unknown>): ParsedStep[] {
  const raw = workflow.steps;
  if (!Array.isArray(raw)) return [];
  return raw.map(parseStep);
}

/** Declared input names from the workflow's `inputs:` block (keys of the
 *  serialized `inputs` map). Empty when none are declared. */
function readInputNames(workflow: Record<string, unknown>): string[] {
  const raw = workflow.inputs;
  if (typeof raw !== 'object' || raw === null) return [];
  return Object.keys(raw as Record<string, unknown>);
}

interface AutoflowInfo {
  /** Human-readable trigger summary, e.g. a cron expression, `event: …`, or
   *  `wakes on: github.issue.opened, …`. Undefined when nothing to show. */
  trigger?: string;
}

/**
 * When the workflow has `autoflow.enabled: true`, return a small descriptor so
 * the header can mark it as an autoflow and summarize what triggers it. Returns
 * null for plain (manually-launched) workflows. Reads the parsed `workflow`
 * object defensively — every field is optional on the wire.
 */
function readAutoflow(workflow: Record<string, unknown>): AutoflowInfo | null {
  const af = workflow.autoflow;
  if (typeof af !== 'object' || af === null) return null;
  const afo = af as Record<string, unknown>;
  if (afo.enabled !== true) return null;

  const trig = workflow.trigger;
  const trigo = typeof trig === 'object' && trig !== null ? (trig as Record<string, unknown>) : {};
  const on = asString(trigo.on);
  if (on === 'cron' && asString(trigo.cron)) return { trigger: `cron: ${asString(trigo.cron)}` };
  if (on === 'event' && asString(trigo.event)) return { trigger: `event: ${asString(trigo.event)}` };

  const wake = afo.wake_on;
  if (Array.isArray(wake)) {
    const events = wake.filter((e): e is string => typeof e === 'string');
    if (events.length > 0) return { trigger: `wakes on: ${events.join(', ')}` };
  }
  return {};
}

export default function WorkflowDetailPage() {
  const { name = '' } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const [detail, setDetail] = useState<WorkflowDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [launcherOpen, setLauncherOpen] = useState(false);

  // ── Edit / delete state ──────────────────────────────────────────────
  // `draftYaml` is the single editable source shared by BOTH the YAML tab
  // (CodeEditor) and the Graph tab (the visual editor emits regenerated YAML
  // into it). It is seeded from the loaded definition and re-synced on save.
  const [view, setView] = useState<EditorView>('yaml');
  const [editing, setEditing] = useState(false);
  const [draftYaml, setDraftYaml] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // ── Agents (for the visual editor's step/panel pickers) ──────────────
  const [agents, setAgents] = useState<AgentSummary[]>([]);

  // ── Live server-side validity badge ──────────────────────────────────
  const [validity, setValidity] = useState<{ ok: boolean; error?: string } | null>(null);

  useEffect(() => {
    if (!name) return;
    let cancelled = false;
    setDetail(null);
    setError(null);
    api
      .getWorkflow(name)
      .then((data) => {
        if (cancelled) return;
        setDetail(data);
        setDraftYaml(data.yaml);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load workflow');
      });
    return () => {
      cancelled = true;
    };
  }, [name]);

  useEffect(() => {
    let cancelled = false;
    api
      .getAgents()
      .then((a) => {
        if (!cancelled) setAgents(a);
      })
      .catch(() => {
        /* agent list is best-effort; the editor still works without it */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Debounced parse-check of the current draft (writes nothing server-side).
  useEffect(() => {
    if (!draftYaml) {
      setValidity(null);
      return;
    }
    let cancelled = false;
    const t = setTimeout(() => {
      api
        .validateWorkflow(draftYaml)
        .then((r) => {
          if (!cancelled) setValidity(r);
        })
        .catch(() => {
          /* network failure → leave the badge unset rather than block saving */
        });
    }, 400);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [draftYaml]);

  function startEdit() {
    if (!detail) return;
    setSaveError(null);
    setEditing(true);
  }

  function cancelEdit() {
    if (detail) setDraftYaml(detail.yaml);
    setEditing(false);
    setSaveError(null);
  }

  function revertDraft() {
    if (detail) setDraftYaml(detail.yaml);
    setSaveError(null);
  }

  async function save() {
    if (!detail || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await api.saveWorkflow(name, draftYaml);
      setDetail(updated);
      setDraftYaml(updated.yaml);
      setEditing(false);
    } catch (e: unknown) {
      setSaveError(e instanceof Error ? e.message : 'Failed to save workflow');
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!detail || deleting) return;
    if (!window.confirm('Delete this workflow?')) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      await api.deleteWorkflow(name);
      navigate('/workflows');
    } catch (e: unknown) {
      setDeleteError(e instanceof Error ? e.message : 'Failed to delete workflow');
      setDeleting(false);
    }
  }

  if (error) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading…</div>
      </div>
    );
  }

  const wfName = asString(detail.workflow.name) ?? name;
  const scope = asString(detail.workflow.scope);
  const description = asString(detail.workflow.description);
  const steps = readSteps(detail.workflow);
  const autoflow = readAutoflow(detail.workflow);
  const inputNames = readInputNames(detail.workflow);

  const dirty = draftYaml !== detail.yaml;
  const invalid = validity?.ok === false;
  const saveDisabled = saving || !dirty || invalid;

  return (
    <div className="p-8 max-w-5xl">
      <BackLink />

      <header className="mt-3">
        <div className="flex flex-wrap items-start gap-2">
          <h1 className="text-2xl font-semibold text-ink break-all">{wfName}</h1>
          {scope && <ScopeChip scope={scope} />}
          {autoflow && (
            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-violet-50 text-violet-700 ring-violet-200">
              Autoflow
            </span>
          )}
          <div className="ml-auto flex items-center gap-2">
            <button
              type="button"
              onClick={remove}
              disabled={deleting}
              aria-label={`Delete ${wfName}`}
              className="inline-flex items-center gap-1.5 rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-red-700 hover:bg-red-50 disabled:cursor-not-allowed disabled:opacity-60"
            >
              <Trash2 size={14} />
              Delete
            </button>
            <button
              type="button"
              onClick={() => setLauncherOpen(true)}
              aria-label={`Run ${wfName}`}
              className="inline-flex items-center rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700"
            >
              Run
            </button>
          </div>
        </div>
        {deleteError && (
          <p role="alert" className="mt-2 text-[12px] font-medium text-red-700">
            {deleteError}
          </p>
        )}
        {description && (
          <p className="mt-2 text-sm text-ink-dim leading-snug">{description}</p>
        )}
        {autoflow?.trigger && (
          <p className="mt-1 text-xs text-ink-mute font-mono break-all">{autoflow.trigger}</p>
        )}
      </header>

      {/* ── Steps ───────────────────────────────────────────────── */}
      <section className="mt-8">
        <h2 className="text-sm font-semibold text-ink mb-3 pl-1">
          Steps <span className="text-xs text-ink-mute tabular-nums">{steps.length}</span>
        </h2>
        {steps.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1">No steps.</p>
        ) : (
          <ol className="relative pl-1">
            {steps.map((s, i) => (
              <StepRow key={`${s.id ?? 'step'}-${i}`} step={s} index={i} last={i === steps.length - 1} />
            ))}
          </ol>
        )}
      </section>

      {/* ── Definition (Graph ⇄ YAML) ───────────────────────────── */}
      <section className="mt-8">
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2 pl-1">
          <div className="flex items-center gap-3">
            <h2 className="text-sm font-semibold text-ink">Definition</h2>
            <div className="inline-flex rounded-lg border border-border bg-white p-0.5">
              <ViewTabButton active={view === 'graph'} onClick={() => setView('graph')}>
                Graph
              </ViewTabButton>
              <ViewTabButton active={view === 'yaml'} onClick={() => setView('yaml')}>
                YAML
              </ViewTabButton>
            </div>
            <ValidityBadge validity={validity} />
          </div>
          {view === 'yaml' && !editing && (
            <button
              type="button"
              onClick={startEdit}
              aria-label="Edit YAML"
              className="inline-flex items-center gap-1.5 rounded-md border border-border bg-white px-2.5 py-1 text-[12px] font-medium text-ink-dim hover:bg-slate-50"
            >
              <Pencil size={13} />
              Edit
            </button>
          )}
        </div>

        {view === 'graph' ? (
          <div className="space-y-3">
            <Suspense
              fallback={<div className="py-12 text-center text-sm text-ink-dim">Loading editor…</div>}
            >
              <WorkflowEditor initialYaml={draftYaml} agents={agents} onYamlChange={setDraftYaml} />
            </Suspense>
            {saveError && (
              <p role="alert" className="text-[12px] font-medium text-red-700">
                {saveError}
              </p>
            )}
            <div className="flex items-center justify-end gap-2">
              <p className="mr-auto text-[11px] text-ink-mute">
                Saving from the graph rewrites the YAML canonically (comments and custom
                formatting are not preserved). Use the YAML tab to hand-edit with comments.
              </p>
              <button
                type="button"
                onClick={revertDraft}
                disabled={saving || !dirty}
                className="inline-flex items-center rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
              >
                Revert
              </button>
              <button
                type="button"
                onClick={save}
                disabled={saveDisabled}
                className="inline-flex items-center rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700 disabled:cursor-not-allowed disabled:opacity-60"
              >
                {saving ? 'Saving…' : 'Save'}
              </button>
            </div>
          </div>
        ) : editing ? (
          <div className="space-y-3">
            <CodeEditor
              value={draftYaml}
              onChange={setDraftYaml}
              language="yaml"
              ariaLabel="Workflow YAML editor"
            />
            {saveError && (
              <p role="alert" className="text-[12px] font-medium text-red-700">
                {saveError}
              </p>
            )}
            <div className="flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={cancelEdit}
                disabled={saving}
                className="inline-flex items-center rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={save}
                disabled={saveDisabled}
                className="inline-flex items-center rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700 disabled:cursor-not-allowed disabled:opacity-60"
              >
                {saving ? 'Saving…' : 'Save'}
              </button>
            </div>
          </div>
        ) : (
          <CodeHighlight code={draftYaml} language="yaml" />
        )}
      </section>

      {launcherOpen && (
        <LauncherSheet
          workflow={wfName}
          declaredInputs={inputNames}
          onClose={() => setLauncherOpen(false)}
        />
      )}
    </div>
  );
}

function StepRow({ step, index, last }: { step: ParsedStep; index: number; last: boolean }) {
  return (
    <li className="flex gap-3">
      {/* Spine */}
      <div className="flex flex-col items-center">
        <span className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-brand-50 text-[11px] font-semibold text-brand-700 ring-1 ring-brand-200 tabular-nums">
          {index + 1}
        </span>
        {!last && <span className="w-px flex-1 bg-border my-1" />}
      </div>

      {/* Body */}
      <div className={cn('min-w-0 flex-1', last ? 'pb-1' : 'pb-4')}>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium text-ink font-mono break-all">
            {step.id ?? '(unnamed step)'}
          </span>
          {step.agent && <StepChip className="bg-blue-50 text-blue-700 ring-blue-200">{step.agent}</StepChip>}
          {step.kind && <StepChip className="bg-slate-100 text-ink-mute ring-slate-200">{step.kind}</StepChip>}
          {step.forEach && (
            <StepChip className="bg-violet-50 text-violet-700 ring-violet-200">
              for_each: {step.forEach}
            </StepChip>
          )}
          {step.parallel && (
            <StepChip className="bg-amber-50 text-amber-800 ring-amber-200">parallel</StepChip>
          )}
        </div>
      </div>
    </li>
  );
}

function ViewTabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'rounded-md px-3 py-1 text-[12px] font-medium',
        active ? 'bg-brand-600 text-white' : 'text-ink-dim hover:text-ink',
      )}
    >
      {children}
    </button>
  );
}

function ValidityBadge({ validity }: { validity: { ok: boolean; error?: string } | null }) {
  if (!validity) return null;
  if (validity.ok) {
    return (
      <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-green-50 text-green-700 ring-green-200">
        ✓ valid
      </span>
    );
  }
  return (
    <span
      className="inline-flex max-w-[20rem] items-center truncate rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-red-50 text-red-700 ring-red-200"
      title={validity.error}
    >
      ✕ {validity.error ?? 'invalid'}
    </span>
  );
}

function StepChip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1',
        className,
      )}
    >
      {children}
    </span>
  );
}

function BackLink() {
  return (
    <Link
      to="/workflows"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Workflows
    </Link>
  );
}
