// Workflow detail — header (name/scope/description), a vertical STEPS spine
// (id + agent + for_each/parallel hints), and the raw YAML. Route:
// /workflows/:name. The parsed `workflow` object is typed loosely on the wire,
// so we narrow each field we read defensively.

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type WorkflowDetail } from '../lib/api';
import { cn } from '../lib/cn';
import { ScopeChip } from './Workflows';

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

export default function WorkflowDetailPage() {
  const { name = '' } = useParams<{ name: string }>();

  const [detail, setDetail] = useState<WorkflowDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

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
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load workflow');
      });
    return () => {
      cancelled = true;
    };
  }, [name]);

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

  return (
    <div className="p-8 max-w-5xl">
      <BackLink />

      <header className="mt-3">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold text-ink break-all">{wfName}</h1>
          {scope && <ScopeChip scope={scope} />}
        </div>
        {description && (
          <p className="mt-2 text-sm text-ink-dim leading-snug">{description}</p>
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

      {/* ── Raw YAML ────────────────────────────────────────────── */}
      <section className="mt-8">
        <h2 className="text-sm font-semibold text-ink mb-2 pl-1">YAML</h2>
        <pre className="whitespace-pre-wrap break-words font-mono text-[12px] leading-relaxed text-ink bg-panel border border-border rounded-xl shadow-card p-4 overflow-x-auto">
          {detail.yaml}
        </pre>
      </section>
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
