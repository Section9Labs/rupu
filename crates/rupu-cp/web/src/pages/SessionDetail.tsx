// Session detail — an interactive chat (CP Phase 2e).
//
// A session is a container of turn-runs. This page is now chat-primary:
//   • a compact identity header (id · agent · model · status · usage)
//   • the conversation (prior turns + live-streaming agent responses)
//   • a composer pinned at the bottom (POST /api/sessions/:id/send)
//   • a secondary, collapsed "Session details" disclosure (usage chart + fields)
// Route: /sessions/:id

import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Archive, ArrowLeft, RotateCcw, Trash2 } from 'lucide-react';
import { api, type SessionSummary, type SessionRunRow, type UsageTimelinePoint } from '../lib/api';
import { cn } from '../lib/cn';
import { absoluteTime, relativeTime } from '../lib/time';
import { sessionStatusDot, sessionStatusLabel, sessionStatusTone } from '../lib/sessionStatus';
import { isSessionActive, pollIntervalFor } from '../lib/sessionPoll';
import UsageChip from '../components/UsageChip';
import { Button } from '../components/ui/Button';
import RunUsageTimeline from '../components/charts/RunUsageTimeline';
import SessionConversation from '../components/session/SessionConversation';

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SessionDetailPage() {
  const { id = '' } = useParams<{ id: string }>();
  const [searchParams] = useSearchParams();
  const host = searchParams.get('host') ?? undefined;
  const navigate = useNavigate();

  const [session, setSession] = useState<SessionSummary | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);

  // Archive / restore / delete action state.
  const [actionPending, setActionPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const [runs, setRuns] = useState<SessionRunRow[] | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);

  // Aggregated per-turn token series across this session's runs (Details chart).
  const [series, setSeries] = useState<UsageTimelinePoint[]>([]);

  // Composer state
  const [prompt, setPrompt] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [sendOk, setSendOk] = useState(false);

  // Fetch session identity (header + active_run_id).
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSession(null);
    setSessionError(null);
    api
      .getSession(id, { host })
      .then((data) => {
        if (cancelled) return;
        setSession(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setSessionError(e instanceof Error ? e.message : 'Failed to load session');
      });
    return () => {
      cancelled = true;
    };
  }, [id, host]);

  // Aggregated per-turn usage timeline for the Details disclosure chart.
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSeries([]);
    api
      .getSessionUsageTimeline(id, { host })
      .then((pts) => {
        if (cancelled) return;
        setSeries(pts);
      })
      .catch(() => {
        if (cancelled) return;
        setSeries([]);
      });
    return () => {
      cancelled = true;
    };
  }, [id, host]);

  // Fetch the session's turn-runs (the conversation).
  const loadRuns = () => {
    if (!id) return;
    api
      .getSessionRuns(id, { host })
      .then((rows) => {
        setRuns(rows);
        setRunsError(null);
      })
      .catch((e: unknown) => {
        setRunsError(e instanceof Error ? e.message : 'Failed to load conversation');
      });
  };

  // Reload both session identity and runs — used after a send or turn completion
  // so the new turn / updated active_run_id surfaces immediately.
  const reload = () => {
    if (!id) return;
    api
      .getSession(id, { host })
      .then((data) => setSession(data))
      .catch(() => {/* keep stale session on transient error */});
    loadRuns();
  };

  // Adaptive poll: fast (1.5s) while a turn is in flight, slow (5s) otherwise.
  // Re-arm when the cadence changes (e.g. session goes active → idle).
  const pollInterval = pollIntervalFor(session);
  useEffect(() => {
    loadRuns();
    const t = window.setInterval(loadRuns, pollInterval);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, pollInterval, host]);

  async function onArchive() {
    if (actionPending) return;
    setActionPending(true);
    setActionError(null);
    try {
      await api.archiveSession(id);
      navigate('/sessions');
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
      setActionPending(false);
    }
  }

  async function onRestore() {
    if (actionPending) return;
    setActionPending(true);
    setActionError(null);
    try {
      await api.restoreSession(id);
      navigate('/sessions');
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
      setActionPending(false);
    }
  }

  async function onDelete() {
    if (actionPending) return;
    if (!window.confirm('Permanently delete this session and its transcripts? This cannot be undone.')) return;
    setActionPending(true);
    setActionError(null);
    try {
      await api.deleteSession(id);
      navigate('/sessions');
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
      setActionPending(false);
    }
  }

  const handleSend = () => {
    const text = prompt.trim();
    if (!id || !text || sending) return;
    setSending(true);
    setSendError(null);
    setSendOk(false);
    api
      .sendSessionMessage(id, text, host)
      .then(() => {
        setPrompt('');
        setSendOk(true);
        // Surface the new turn immediately rather than waiting for the poll —
        // its TranscriptPanel then streams live.
        reload();
      })
      .catch((e: unknown) => {
        setSendError(e instanceof Error ? e.message : 'Failed to send message');
      })
      .finally(() => setSending(false));
  };

  if (sessionError) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {sessionError}
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

  const active = isSessionActive(session);
  const stopped = sessionStatusTone(session.status) === 'stopped';

  return (
    <div className="flex h-full flex-col">
      {/* Compact identity header */}
      <header className="shrink-0 border-b border-border bg-panel px-6 py-3">
        <BackLink />
        <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <h1 className="font-mono text-base font-semibold text-ink break-all">
            {session.session_id}
          </h1>
          <span className="text-ui text-ink-dim">
            <span className="font-mono">{session.agent_name}</span>
            <span className="mx-1 text-border">·</span>
            <span className="font-mono">{session.model}</span>
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span
              className={cn('inline-block h-2 w-2 rounded-full', sessionStatusDot(session.status))}
            />
            <span className="text-ui text-ink-dim">{sessionStatusLabel(session.status)}</span>
          </span>
          {/* "working…" pill — visible while a turn is in flight. */}
          {active && (
            <span className="inline-flex items-center gap-1 rounded px-1.5 py-px text-[9px] font-medium bg-status-running/10 text-status-running">
              <span className="inline-block h-1.5 w-1.5 rounded-full animate-pulse bg-status-running" />
              working…
            </span>
          )}
          {host && (
            <span className="inline-flex items-center rounded bg-surface px-1.5 py-px text-[10px] font-mono font-medium text-ink-dim ring-1 ring-border">
              on {host}
            </span>
          )}
          {session.usage && <UsageChip usage={session.usage} className="ml-auto" />}
        </div>

        {/* Archive / Restore / Delete button cluster */}
        <div className="mt-2 flex flex-col items-end gap-1">
          <div className="flex items-center gap-2">
            {session.scope === 'archived' ? (
              <Button
                variant="secondary"
                onClick={() => void onRestore()}
                disabled={actionPending}
                className="gap-1.5"
                aria-label="Restore session"
              >
                <RotateCcw size={14} /> Restore
              </Button>
            ) : (
              <Button
                variant="secondary"
                onClick={() => void onArchive()}
                disabled={actionPending}
                className="gap-1.5"
                aria-label="Archive session"
              >
                <Archive size={14} /> Archive
              </Button>
            )}
            <Button
              variant="danger-outline"
              onClick={() => void onDelete()}
              disabled={actionPending}
              className="gap-1.5"
              aria-label="Delete session"
            >
              <Trash2 size={14} /> Delete
            </Button>
          </div>
          {actionError && (
            <p role="alert" className="text-ui font-medium text-err">
              {actionError}
            </p>
          )}
        </div>

        {/* Session error banner — only shown when idle/terminal, never while a turn is in flight. */}
        {!active && (session.status === 'failed' || session.last_error) && (
          <div
            role="alert"
            className="mt-2 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-ui text-err"
          >
            {session.last_error ?? 'Session failed.'}
          </div>
        )}

        {/* Secondary: collapsed details (usage chart + identity fields). */}
        <details className="mt-2 group">
          <summary className="cursor-pointer list-none text-ui font-medium text-ink-dim hover:text-ink">
            <span className="group-open:hidden">▸ Session details</span>
            <span className="hidden group-open:inline">▾ Session details</span>
          </summary>
          <div className="mt-3 space-y-4">
            <section className="rounded-xl border border-border bg-panel px-4 py-3 shadow-card">
              <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-dim">
                Token usage by turn
              </h2>
              <RunUsageTimeline series={series} />
            </section>

            <dl className="divide-y divide-border overflow-hidden rounded-xl border border-border bg-panel shadow-card">
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
                <dt className="w-32 shrink-0 text-ui font-medium text-ink-mute">Active run</dt>
                <dd className="min-w-0 flex-1 text-sm text-ink">
                  {session.active_run_id ? (
                    <Link
                      to={`/runs/${encodeURIComponent(session.active_run_id)}`}
                      className="inline-flex items-center rounded bg-info-bg px-2 py-0.5 font-mono text-ui font-medium text-info ring-1 ring-info/30 hover:bg-info-bg"
                    >
                      {session.active_run_id}
                    </Link>
                  ) : (
                    <span className="text-ink-dim">—</span>
                  )}
                </dd>
              </div>
            </dl>
          </div>
        </details>
      </header>

      {/* Conversation — the growing, scrolling chat body. */}
      <SessionConversation session={session} runs={runs ?? []} onTurnComplete={reload} />

      {/* Composer — pinned at the bottom. */}
      <div className="shrink-0 border-t border-border bg-panel px-6 py-3">
        {runsError && (
          <div className="mb-2 rounded-lg border border-warn/30 bg-warn-bg px-3 py-2 text-ui text-warn">
            {runsError}
          </div>
        )}
        <div className="mx-auto max-w-3xl">
          <label htmlFor="session-composer" className="sr-only">
            Message this session
          </label>
          <textarea
            id="session-composer"
            value={prompt}
            onChange={(e) => {
              setPrompt(e.target.value);
              if (sendOk) setSendOk(false);
            }}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
                e.preventDefault();
                handleSend();
              }
            }}
            disabled={sending || stopped}
            rows={2}
            placeholder="Message this session…"
            className="w-full resize-y rounded-lg border border-border bg-panel px-3 py-2 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-200 disabled:cursor-not-allowed disabled:bg-surface disabled:text-ink-dim"
          />

          {sendError && (
            <div
              role="alert"
              className="mt-2 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-sm text-err"
            >
              {sendError}
            </div>
          )}

          <div className="mt-2 flex items-center justify-between gap-3">
            <div className="text-ui text-ink-dim">
              {stopped ? (
                <span>Session is stopped — sending is disabled.</span>
              ) : sendOk ? (
                <span className="text-ok">Sent — turn queued.</span>
              ) : (
                <span className="text-ink-mute">⌘/Ctrl+Enter to send</span>
              )}
            </div>
            <Button
              onClick={handleSend}
              disabled={sending || stopped || prompt.trim().length === 0}
              className="px-4 text-sm disabled:opacity-50"
            >
              {sending ? 'Sending…' : 'Send'}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-4 px-4 py-3">
      <dt className="w-32 shrink-0 text-ui font-medium text-ink-mute">{label}</dt>
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
