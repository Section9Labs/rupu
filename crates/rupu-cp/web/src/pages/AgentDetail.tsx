// Agent detail — header meta (provider/model/effort/max_tokens) plus the full
// raw definition file (YAML frontmatter + markdown body) shown with syntax
// highlighting. Route: /agents/:name

import { useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { Pencil, Trash2 } from 'lucide-react';
import { api, scopeSelectorFor, skillNamesOf, type AgentDetail } from '../lib/api';
import CodeHighlight from '../components/CodeHighlight';
import AgentBuilder from '../components/agentBuilder/AgentBuilder';
import AgentLauncherSheet from '../components/AgentLauncherSheet';
import AgentUsageTimeline from '../components/agent/AgentUsageTimeline';
import { Button } from '../components/ui/Button';
import { PageHeader } from '../components/ui/PageHeader';
import { Chip } from '../components/ui/Chip';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';
import { UnscopedNote } from '../components/customers/UnscopedNote';

export default function AgentDetailPage() {
  const { name = '' } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const [agent, setAgent] = useState<AgentDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [runOpen, setRunOpen] = useState(false);

  // ── Edit / delete state ──────────────────────────────────────────────
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [skillNames, setSkillNames] = useState<string[]>([]);

  useEffect(() => {
    if (!name) return;
    let cancelled = false;
    setAgent(null);
    setError(null);
    api
      .getAgent(name)
      .then((data) => {
        if (cancelled) return;
        setAgent(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load agent');
      });
    return () => {
      cancelled = true;
    };
  }, [name]);

  // The Skills card suggests what this agent's scope can see: the global
  // layer, plus its project's for a project agent.
  const skillsWsId = agent?.scope_kind === 'project' ? agent.scope_id ?? undefined : undefined;
  useEffect(() => {
    if (!editing) return;
    let cancelled = false;
    api
      .getSkills(skillsWsId)
      .then((rows) => {
        if (!cancelled) setSkillNames(skillNamesOf(rows));
      })
      .catch(() => {
        // Non-fatal — the Skills card just has no suggestions.
      });
    return () => {
      cancelled = true;
    };
  }, [editing, skillsWsId]);

  function startEdit() {
    if (!agent) return;
    setSaveError(null);
    setEditing(true);
  }

  function cancelEdit() {
    setEditing(false);
    setSaveError(null);
  }

  async function saveFrom(raw: string) {
    if (!agent || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      // `scopeSelectorFor(agent)` threads the resolved `scope_kind`/
      // `scope_id` so Save targets THIS exact file — mirrors `remove()`'s
      // Delete call below. Without it a project-scoped Save would silently
      // create a hidden global shadow instead of editing the file shown.
      const updated = await api.saveAgent(name, raw, scopeSelectorFor(agent));
      setAgent(updated);
      setEditing(false);
    } catch (e: unknown) {
      setSaveError(e instanceof Error ? e.message : 'Failed to save agent');
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!agent) return;
    const scopeLabel = agent.scope_kind === 'project' ? `project: ${agent.scope}` : 'global';
    if (
      !window.confirm(
        `Delete agent "${agent.name}" (${scopeLabel})? This removes the definition file. This cannot be undone.`,
      )
    ) {
      return;
    }
    setDeleteError(null);
    try {
      // By `slug` (file stem), never the route's `name` param (frontmatter
      // display name) — `DELETE /api/agents/:name` resolves by file stem
      // (see `AgentSummary.slug`'s doc comment), and the two can differ for
      // hand- or CLI-authored files. `scopeSelectorFor(agent)` threads the
      // resolved `scope_kind`/`scope_id` so the request targets THIS exact
      // file even when another repo defines the same slug. Mirrors
      // `Agents.tsx`'s row Delete.
      const result = await api.deleteAgent(agent.slug ?? name, scopeSelectorFor(agent));
      if (
        agent.scope_kind &&
        (result.scope_kind !== agent.scope_kind || result.scope !== agent.scope)
      ) {
        setDeleteError(
          `Deleted the ${result.scope_kind === 'project' ? `project (${result.scope})` : 'global'} definition — not the one shown here.`,
        );
        return;
      }
      navigate('/agents');
    } catch (e: unknown) {
      setDeleteError(e instanceof Error ? e.message : 'Failed to delete agent');
    }
  }

  if (error) {
    return (
      <div className="p-8">
        <PageHeader back={{ label: 'Agents', to: '/agents' }} title="Agent" />
        <ErrorBanner className="mt-4">{error}</ErrorBanner>
      </div>
    );
  }

  if (!agent) {
    return (
      <div className="p-8">
        <PageHeader back={{ label: 'Agents', to: '/agents' }} title="Agent" />
        <div className="mt-6"><Spinner label="Loading…" /></div>
      </div>
    );
  }

  return (
    <div className="p-8">
      {/* `DELETE /api/agents/:name` resolves project-aware (global-
       *  then-registered-projects, by file stem — see `resolve_agent_scoped`
       *  in `rupu-cp/src/api/agents.rs`), the SAME layer walk this page's
       *  `getAgent` load uses, so it always removes the exact file being shown
       *  here — safe for every scope. `remove()`'s confirm dialog names the
       *  resolved scope before the operator confirms. */}
      <PageHeader
        back={{ label: 'Agents', to: '/agents' }}
        title={agent.name}
        actions={
          <>
            <Button
              variant="danger-outline"
              onClick={remove}
              aria-label={`Delete ${agent.name}`}
              className="gap-1.5"
            >
              <Trash2 size={14} />
              Delete
            </Button>
            <Button variant="solid" onClick={() => setRunOpen(true)} aria-label={`Run ${agent.name}`}>
              Run
            </Button>
          </>
        }
      />

      {deleteError && (
        <p role="alert" className="mt-2 text-ui font-medium text-err">
          {deleteError}
        </p>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        {agent.scope && <ScopeChip scope={agent.scope} />}
        {agent.provider && <MetaChip>{agent.provider}</MetaChip>}
        {agent.model && <MetaChip>{agent.model}</MetaChip>}
        {agent.effort && <MetaChip>effort: {agent.effort}</MetaChip>}
        {typeof agent.max_tokens === 'number' && (
          <MetaChip>max_tokens: {agent.max_tokens.toLocaleString()}</MetaChip>
        )}
      </div>
      {agent.description && (
        <p className="mt-2 text-sm text-ink-dim leading-snug">{agent.description}</p>
      )}
      {agent.load_error && (
        <ErrorBanner className="mt-3">
          <span className="font-semibold">This agent can't be launched: </span>
          {agent.load_error}
        </ErrorBanner>
      )}

      <UnscopedNote className="mt-4">Runs and spend on this page cover every customer.</UnscopedNote>

      <section className="mt-8">
        <h2 className="mb-2 pl-1 text-sm font-semibold text-ink">Spend over time</h2>
        <AgentUsageTimeline agent={name} />
      </section>

      <section className="mt-8">
        <div className="mb-2 flex items-center justify-between pl-1">
          <h2 className="text-sm font-semibold text-ink">Definition</h2>
          {!editing && (
            <Button
              variant="secondary"
              size="sm"
              onClick={startEdit}
              aria-label="Edit definition"
              className="gap-1.5"
            >
              <Pencil size={13} />
              Edit
            </Button>
          )}
        </div>

        {editing ? (
          <div className="h-[80vh] overflow-hidden rounded-xl border border-border bg-panel shadow-card">
            <AgentBuilder
              initialRaw={agent.raw}
              submitLabel="Save"
              submitting={saving}
              error={saveError}
              onSubmit={saveFrom}
              onCancel={cancelEdit}
              skillNames={skillNames}
            />
          </div>
        ) : agent.raw ? (
          <CodeHighlight code={agent.raw} frontmatter />
        ) : (
          <p className="text-sm text-ink-dim pl-1">No definition.</p>
        )}
      </section>

      {runOpen && (
        <AgentLauncherSheet
          agent={agent.name}
          onClose={() => setRunOpen(false)}
        />
      )}
    </div>
  );
}

function MetaChip({ children }: { children: React.ReactNode }) {
  return <Chip className="bg-surface text-ink-mute ring-border">{children}</Chip>;
}
