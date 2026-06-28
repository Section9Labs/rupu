// Agent detail — header meta (provider/model/effort/max_tokens) plus the full
// raw definition file (YAML frontmatter + markdown body) shown with syntax
// highlighting. Route: /agents/:name

import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft, Pencil, Trash2 } from 'lucide-react';
import { api, type AgentDetail } from '../lib/api';
import { cn } from '../lib/cn';
import CodeHighlight from '../components/CodeHighlight';
import CodeEditor from '../components/CodeEditor';
import AgentLauncherSheet from '../components/AgentLauncherSheet';
import { Button } from '../components/ui/Button';

export default function AgentDetailPage() {
  const { name = '' } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const [agent, setAgent] = useState<AgentDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [runOpen, setRunOpen] = useState(false);

  // ── Edit / delete state ──────────────────────────────────────────────
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

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

  function startEdit() {
    if (!agent) return;
    setDraft(agent.raw);
    setSaveError(null);
    setEditing(true);
  }

  function cancelEdit() {
    setEditing(false);
    setSaveError(null);
  }

  async function save() {
    if (!agent || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await api.saveAgent(name, draft);
      setAgent(updated);
      setDraft(updated.raw);
      setEditing(false);
    } catch (e: unknown) {
      setSaveError(e instanceof Error ? e.message : 'Failed to save agent');
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!agent) return;
    if (!window.confirm('Delete this agent?')) return;
    setDeleteError(null);
    try {
      await api.deleteAgent(name);
      navigate('/agents');
    } catch (e: unknown) {
      setDeleteError(e instanceof Error ? e.message : 'Failed to delete agent');
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

  if (!agent) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading…</div>
      </div>
    );
  }

  return (
    <div className="p-8 max-w-5xl mx-auto">
      <BackLink />

      <header className="mt-3">
        <div className="flex flex-wrap items-start gap-2">
          <h1 className="text-2xl font-semibold text-ink break-all">{agent.name}</h1>
          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="danger-outline"
              onClick={remove}
              aria-label={`Delete ${agent.name}`}
              className="gap-1.5"
            >
              <Trash2 size={14} />
              Delete
            </Button>
            <Button onClick={() => setRunOpen(true)} aria-label={`Run ${agent.name}`}>
              Run
            </Button>
          </div>
        </div>
        {deleteError && (
          <p role="alert" className="mt-2 text-ui font-medium text-red-700">
            {deleteError}
          </p>
        )}
        <div className="mt-2 flex flex-wrap items-center gap-2">
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
      </header>

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
          <div className="space-y-3">
            <CodeEditor
              value={draft}
              onChange={setDraft}
              language="markdown"
              ariaLabel="Agent definition"
            />
            {saveError && (
              <p role="alert" className="text-ui font-medium text-red-700">
                {saveError}
              </p>
            )}
            <div className="flex items-center justify-end gap-2">
              <Button variant="secondary" onClick={cancelEdit} disabled={saving}>
                Cancel
              </Button>
              <Button onClick={save} disabled={saving || draft === agent.raw}>
                {saving ? 'Saving…' : 'Save'}
              </Button>
            </div>
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
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200',
      )}
    >
      {children}
    </span>
  );
}

function BackLink() {
  return (
    <Link
      to="/agents"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Agents
    </Link>
  );
}
