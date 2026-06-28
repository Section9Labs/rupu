// Workflow detail — a constrained header (name/scope/description, validity
// badge, Save/Revert/Delete/Run) above a full-bleed unified editor shell that
// always renders the graph (top) + live YAML (bottom) + inspector rail. Route:
// /workflows/:name. The parsed `workflow` object is typed loosely on the wire,
// so we narrow each field we read defensively.

import { lazy, Suspense, useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft, Trash2 } from 'lucide-react';
import { api, type AgentSummary, type WorkflowDetail } from '../lib/api';
import { ScopeChip } from './Workflows';
import LauncherSheet from '../components/LauncherSheet';
import { Button } from '../components/ui/Button';

// Lazy so the @xyflow/react canvas + CodeMirror (and the rest of the visual
// editor) stay out of the main bundle — only fetched once the page mounts.
const WorkflowEditor = lazy(() => import('../components/workflow-editor/WorkflowEditor'));

// ── Loose narrowing helpers ──────────────────────────────────────────────
// The backend hands back `workflow: Record<string, unknown>`; we read only the
// few fields the UI needs and tolerate anything missing or oddly-shaped.

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
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
  // `draftYaml` is the single editable source the shell shares between the
  // graph (emits regenerated YAML into it) and the always-live YAML pane. It is
  // seeded from the loaded definition and re-synced on save.
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

  // Unsaved-changes guard. The app mounts a plain <BrowserRouter> (not a data
  // router), so react-router's `useBlocker` is unavailable — we rely on the
  // native `beforeunload` prompt for browser close / refresh / external nav while
  // the draft diverges from the saved YAML. In-app route changes are not blocked.
  const dirty = detail !== null && draftYaml !== detail.yaml;
  useEffect(() => {
    if (!dirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = '';
    };
    window.addEventListener('beforeunload', handler);
    return () => window.removeEventListener('beforeunload', handler);
  }, [dirty]);

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
  const autoflow = readAutoflow(detail.workflow);
  const inputNames = readInputNames(detail.workflow);

  const saveDisabled = saving || !dirty || validity?.ok === false;
  const revertDisabled = saving || !dirty;

  return (
    <div className="p-8">
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
            <ValidityBadge validity={validity} />
            <button
              type="button"
              onClick={revertDraft}
              disabled={revertDisabled}
              className="inline-flex items-center rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
            >
              Revert
            </button>
            <Button onClick={save} disabled={saveDisabled}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
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
            <Button onClick={() => setLauncherOpen(true)} aria-label={`Run ${wfName}`}>
              Run
            </Button>
          </div>
        </div>
        {saveError && (
          <p role="alert" className="mt-2 text-[12px] font-medium text-red-700">
            {saveError}
          </p>
        )}
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

      {/* ── Unified editor shell (graph + live YAML + inspector) ──────── */}
      <div className="mt-6">
        <Suspense
          fallback={<div className="py-12 text-center text-sm text-ink-dim">Loading editor…</div>}
        >
          <WorkflowEditor
            draftYaml={draftYaml}
            onYamlChange={setDraftYaml}
            agents={agents}
            validity={validity}
          />
        </Suspense>
      </div>

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

function ValidityBadge({ validity }: { validity: { ok: boolean; error?: string } | null }) {
  if (!validity) return null;
  if (validity.ok) {
    return (
      <span className="inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 bg-green-50 text-green-700 ring-green-200">
        ✓ valid
      </span>
    );
  }
  return (
    <span
      className="inline-flex max-w-[20rem] items-center truncate rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 bg-red-50 text-red-700 ring-red-200"
      title={validity.error}
    >
      ✕ {validity.error ?? 'invalid'}
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
