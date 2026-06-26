// Agent detail — header meta (provider/model/effort/max_tokens) plus the full
// raw definition file (YAML frontmatter + markdown body) shown with syntax
// highlighting. Route: /agents/:name

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type AgentDetail } from '../lib/api';
import { cn } from '../lib/cn';
import CodeHighlight from '../components/CodeHighlight';

export default function AgentDetailPage() {
  const { name = '' } = useParams<{ name: string }>();

  const [agent, setAgent] = useState<AgentDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    <div className="p-8 max-w-5xl">
      <BackLink />

      <header className="mt-3">
        <h1 className="text-2xl font-semibold text-ink break-all">{agent.name}</h1>
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
        <h2 className="text-sm font-semibold text-ink mb-2 pl-1">Definition</h2>
        {agent.raw ? (
          <CodeHighlight code={agent.raw} frontmatter />
        ) : (
          <p className="text-sm text-ink-dim pl-1">No definition.</p>
        )}
      </section>
    </div>
  );
}

function MetaChip({ children }: { children: React.ReactNode }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200',
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
