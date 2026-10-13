// Skill detail — one installed skill (`GET /api/skills/:layer/:name`): its
// facts, files and `SKILL.md` body. Read-only: install and update are
// `rupu skill` operations. Route: /skills/:layer/:name[?ws_id=] (a project
// skill needs its project's ws_id).

import { useEffect, useState, type ReactNode } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, apiErrorMessage, type SkillDetail, type SkillLayer } from '../lib/api';
import Markdown from '../components/transcript/Markdown';
import { Badge } from '../components/ui/Badge';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';

export default function SkillDetailPage() {
  const { layer = '', name = '' } = useParams<{ layer: string; name: string }>();
  const [params] = useSearchParams();
  const wsId = params.get('ws_id') ?? undefined;
  const [skill, setSkill] = useState<SkillDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSkill(null);
    setError(null);
    api
      .getSkill(layer as SkillLayer, name, wsId)
      .then((s) => {
        if (!cancelled) setSkill(s);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(apiErrorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [layer, name, wsId]);

  if (error) {
    return (
      <div className="p-8">
        <BackLink />
        <ErrorBanner className="mt-4">{error}</ErrorBanner>
      </div>
    );
  }

  if (!skill) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4">
          <Spinner label="Loading…" />
        </div>
      </div>
    );
  }

  const metadata = Object.entries(skill.metadata);

  return (
    <div className="p-8">
      <BackLink />

      <header className="mt-3">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="break-all font-mono text-2xl font-semibold text-ink">{skill.name}</h1>
          <ScopeChip scope={skill.layer} />
          {skill.both_layers && <Badge ring>both layers</Badge>}
          {skill.disable_model_invocation && (
            <Badge ring title="disable-model-invocation: only an operator or a preload brings it in">
              operator-invoked only
            </Badge>
          )}
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-note text-ink-mute">
          <span>
            source{' '}
            <span className="font-mono text-ink-dim">
              {skill.source ?? (skill.source_error ? `unreadable (${skill.source_error})` : '-')}
            </span>
          </span>
          <span className="break-all font-mono">{skill.path}</span>
        </div>
        {skill.status === 'invalid' && skill.error && (
          <ErrorBanner className="mt-3">
            <span className="font-semibold">This skill doesn't load: </span>
            {skill.error}
          </ErrorBanner>
        )}
        {skill.warnings.length > 0 && (
          <div className="mt-3 rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
            {skill.warnings.map((w, i) => (
              <div key={`${i}:${w}`}>{w}</div>
            ))}
          </div>
        )}
      </header>

      <dl className="mt-6 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-3 text-sm">
        <Fact label="Description">{skill.description ?? '-'}</Fact>
        <Fact label="Compatibility">{skill.compatibility ?? '-'}</Fact>
        <Fact label="License">{skill.license ?? '-'}</Fact>
        <Fact label="Allowed tools">
          {skill.allowed_tools.length === 0 ? (
            '-'
          ) : (
            <div>
              <div className="flex flex-wrap gap-1.5">
                {skill.allowed_tools.map((t) => (
                  <Code key={t}>{t}</Code>
                ))}
              </div>
              {skill.mapped_tools.length > 0 && (
                <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-note text-ink-mute">
                  rupu tools
                  {skill.mapped_tools.map((t) => (
                    <Code key={t}>{t}</Code>
                  ))}
                </div>
              )}
              <p className="mt-1.5 text-note text-ink-mute">
                Informational, never a grant: the agent's own <span className="font-mono">tools:</span> decide.
              </p>
            </div>
          )}
        </Fact>
        <Fact label="Metadata">
          {metadata.length === 0 ? (
            '-'
          ) : (
            <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1">
              {metadata.map(([k, v]) => (
                <div key={k} className="contents">
                  <dt className="font-mono text-ink-mute">{k}</dt>
                  <dd className="text-ink-dim">{v}</dd>
                </div>
              ))}
            </dl>
          )}
        </Fact>
        <Fact label="Files">
          {skill.files.length === 0 ? (
            '-'
          ) : (
            <ul aria-label="Files" className="space-y-0.5">
              {skill.files.map((f) => (
                <li key={f} className="font-mono text-ui text-ink-dim">
                  {f}
                </li>
              ))}
            </ul>
          )}
        </Fact>
        <Fact label="Content hash">
          <span className="font-mono">{skill.content_hash ?? '-'}</span>
        </Fact>
      </dl>

      {skill.body && (
        <section className="mt-8">
          <h2 className="mb-2 pl-1 text-sm font-semibold text-ink">SKILL.md</h2>
          <div className="rounded-xl border border-border bg-panel p-5">
            <Markdown text={skill.body} />
          </div>
        </section>
      )}
    </div>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-note font-medium uppercase tracking-wide text-ink-mute">{label}</dt>
      <dd className="min-w-0 text-ink-dim">{children}</dd>
    </>
  );
}

function Code({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded bg-surface px-1.5 py-0.5 font-mono text-note text-ink ring-1 ring-border">
      {children}
    </span>
  );
}

function BackLink() {
  return (
    <Link
      to="/library?tab=skills"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Skills
    </Link>
  );
}
