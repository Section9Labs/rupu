// Session detail — read-only field summary for one session. The backend does
// NOT return a transcript here (that's a later task), so we show only the
// session record fields. Route: /sessions/:id

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { cn } from '../lib/cn';
import { absoluteTime, relativeTime } from '../lib/time';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';

export default function SessionDetailPage() {
  const { id = '' } = useParams<{ id: string }>();

  const [session, setSession] = useState<SessionSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSession(null);
    setError(null);
    api
      .getSession(id)
      .then((data) => {
        if (cancelled) return;
        setSession(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load session');
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

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

  if (!session) {
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
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold text-ink break-all font-mono">
            {session.session_id}
          </h1>
          <span className="inline-flex items-center gap-1.5">
            <span
              className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(session.status))}
            />
            <span className="text-[12px] text-ink-dim">{sessionStatusLabel(session.status)}</span>
          </span>
        </div>
      </header>

      <section className="mt-6">
        <dl className="bg-panel border border-border rounded-xl shadow-card divide-y divide-border overflow-hidden">
          <Field label="Agent" value={session.agent_name} mono />
          <Field label="Model" value={session.model} mono />
          <Field label="Scope" value={session.scope} />
          <Field label="Status" value={sessionStatusLabel(session.status)} />
          <Field label="Total turns" value={String(session.total_turns)} />
          <Field label="Target" value={session.target ?? '—'} mono={!!session.target} />
          <Field
            label="Created"
            value={`${absoluteTime(session.created_at)} (${relativeTime(session.created_at)})`}
          />
          <Field
            label="Updated"
            value={`${absoluteTime(session.updated_at)} (${relativeTime(session.updated_at)})`}
          />
          <div className="flex items-baseline gap-4 px-4 py-3">
            <dt className="w-32 shrink-0 text-[12px] font-medium text-ink-mute">Active run</dt>
            <dd className="min-w-0 flex-1 text-sm text-ink">
              {session.active_run_id ? (
                <Link
                  to={`/runs/${encodeURIComponent(session.active_run_id)}`}
                  className="inline-flex items-center rounded px-2 py-0.5 text-[12px] font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100 font-mono"
                >
                  {session.active_run_id}
                </Link>
              ) : (
                <span className="text-ink-dim">—</span>
              )}
            </dd>
          </div>
        </dl>
      </section>
    </div>
  );
}

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-4 px-4 py-3">
      <dt className="w-32 shrink-0 text-[12px] font-medium text-ink-mute">{label}</dt>
      <dd className={cn('min-w-0 flex-1 text-sm text-ink break-all', mono && 'font-mono')}>
        {value}
      </dd>
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/sessions"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Sessions
    </Link>
  );
}
