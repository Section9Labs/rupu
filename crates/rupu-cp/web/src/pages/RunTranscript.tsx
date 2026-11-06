// Transcript-only page — renders a TranscriptPanel for agent/session/standalone
// runs that have no workflow DAG. Path, live flag, host and run come from query
// params:
//   /transcript?path=<encoded-path>&live=<0|1>[&host=<id>&run=<id>]
//
// `host` and `run` are what let a REMOTE transcript resolve at all: the backend
// needs `run` to authorize a not-yet-mirrored path against the run that claims
// it (spec §3.3). Without it a sub-run opened from a remote run's panel 400s.
//
// A local standalone run's page also heads itself with the run (agent, status,
// model, from its `/api/runs/agents` row) and, once the run has ended
// (completed, failed, cancelled, or crashed before recording an end: no
// status), offers Continue (operator MCP Plan 3b, UI-1, mock §2). A completed
// run continues with a prompt, which the composer then requires.
//
// Route: /transcript (registered in App.tsx)

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useLocation, useSearchParams, useNavigate } from 'react-router-dom';
import { ArrowLeft, ChevronDown, ChevronRight } from 'lucide-react';
import TranscriptPanel from '../components/TranscriptPanel';
import ChildrenPanel from '../components/run/ChildrenPanel';
import MessageFeed from '../components/messages/MessageFeed';
import { MessageLinksContext, type MessageLinks } from '../components/transcript/messageLinks';
import { msgHash, parseMsgHash } from '../lib/messages';
import { useMessageSpace } from '../lib/useMessageSpace';
import { api, type AgentRunRow, type RunStatusStr } from '../lib/api';
import { Button } from '../components/ui/Button';
import { RunStatusChip } from '../components/ui/RunStatusChip';
import { ContinueComposer } from '../components/runops/ContinueComposer';

/** The ended statuses a standalone run can be continued from. A run with no
 *  status crashed before recording how it ended, and can be too; any other
 *  status is a run still going (the server would 409). */
const CONTINUABLE = new Set(['completed', 'failed', 'cancelled']);

/** A standalone run's `/api/runs/agents` row (local only), re-read on demand. */
function useAgentRunRow(runId: string | null): [AgentRunRow | null, () => void] {
  const [row, setRow] = useState<AgentRunRow | null>(null);
  const [gen, setGen] = useState(0);
  useEffect(() => {
    setRow(null);
  }, [runId]);
  useEffect(() => {
    if (!runId) return;
    let cancelled = false;
    api
      .getAgentRuns({ q: `id:${runId}`, limit: 5 })
      .then((rows) => {
        if (!cancelled) setRow(rows.find((r) => r.run_id === runId) ?? null);
      })
      .catch(() => {
        /* no row → the page heads itself with the file name */
      });
    return () => {
      cancelled = true;
    };
  }, [runId, gen]);
  const reload = useCallback(() => setGen((g) => g + 1), []);
  return [row, reload];
}

/** The run id a standalone run's transcript (`<transcripts>/<run_id>.jsonl`)
 *  is named after, or `null` for any other transcript. */
function transcriptRunId(path: string): string | null {
  const stem = (path.split('/').filter(Boolean).pop() ?? '').replace(/\.jsonl$/, '');
  return /^run_[A-Za-z0-9_-]+$/.test(stem) ? stem : null;
}

export default function RunTranscript() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const path = searchParams.get('path') ?? '';
  const liveParam = searchParams.get('live') ?? '0';
  const live = liveParam === '1' || liveParam === 'true';
  const host = searchParams.get('host') ?? undefined;
  const run = searchParams.get('run') ?? undefined;
  const local = !host || host === 'local';
  const standaloneId = path ? transcriptRunId(path) : null;
  const [row, reloadRow] = useAgentRunRow(local ? standaloneId : null);
  const [continuing, setContinuing] = useState(false);
  useEffect(() => setContinuing(false), [path]);

  if (!path) {
    return (
      <div className="p-8 max-w-3xl">
        <BackButton navigate={navigate} />
        <div className="mt-6 rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
          No transcript path provided. Navigate here via an agent run or session turn row.
        </div>
      </div>
    );
  }

  // Derive a display label from the path: show the last path segment (filename)
  // so the header reads something meaningful even without a separate run_id param.
  const label = path.split('/').filter(Boolean).pop() ?? 'Transcript';
  const childRunId = standaloneId;
  const status = row?.status ?? null;
  const canContinue =
    !!row && row.source === 'standalone' && !!childRunId && (status === null || CONTINUABLE.has(status));

  return (
    <div className="flex flex-col p-8 gap-4">
      <BackButton navigate={navigate} />

      <header className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          {row?.agent ? (
            <>
              <h1 className="flex flex-wrap items-center gap-2.5 text-xl font-semibold text-ink">
                <span className="break-all">{row.agent}</span>
                {status && <RunStatusChip status={status as RunStatusStr} />}
              </h1>
              <p className="mt-0.5 flex flex-wrap gap-x-1.5 text-note text-ink-dim">
                <span className="font-mono">{row.run_id}</span>
                {row.codename && <span>· {row.codename}</span>}
                {row.model && <span>· {row.model}</span>}
              </p>
            </>
          ) : (
            <h1 className="text-xl font-semibold text-ink font-mono break-all">{label}</h1>
          )}
          <p className="mt-0.5 text-note text-ink-mute font-mono truncate" title={path}>
            {path}
          </p>
        </div>
        {canContinue && !continuing && (
          <Button variant="solid" className="shrink-0" onClick={() => setContinuing(true)}>
            Continue
          </Button>
        )}
      </header>

      {/* The children ledger is local-only: no section for a remote run. */}
      {childRunId && (!host || host === 'local') && <ChildrenSection runId={childRunId} />}

      {childRunId ? (
        <StandaloneRun runId={childRunId} path={path} live={live} host={host} run={run} onComplete={reloadRow} />
      ) : (
        <div className="flex-1 min-h-0">
          <TranscriptPanel path={path} live={live} host={host} runId={run} />
        </div>
      )}

      {canContinue && continuing && childRunId && (
        <ContinueComposer
          key={childRunId}
          runId={childRunId}
          path={path}
          provider={row?.provider}
          model={row?.model}
          status={status}
          onCancel={() => setContinuing(false)}
        />
      )}
    </div>
  );
}

/** A standalone run's transcript, with its message space (spec F1c §3.3)
 *  in a collapsible section above it once the run has one. */
function StandaloneRun({
  runId,
  path,
  live,
  host,
  run,
  onComplete,
}: {
  runId: string;
  path: string;
  live: boolean;
  host?: string;
  run?: string;
  /** The live run ended: re-read its row so Continue can appear. */
  onComplete?: () => void;
}) {
  const { hash, search } = useLocation();
  const navigate = useNavigate();
  const [open, setOpen] = useState(() => parseMsgHash(hash) !== null);
  const messages = useMessageSpace({ route: 'runs', id: runId, host }, { live, load: live || open, reading: open });
  const links = useMemo<MessageLinks>(
    () => ({
      openInFeed: (id) => {
        setOpen(true);
        navigate({ search, hash: msgHash(id) });
      },
    }),
    [navigate, search],
  );
  const present = messages.space === 'present';
  return (
    <>
      {present && (
        <section className="rounded-xl border border-border bg-panel">
          <button
            type="button"
            onClick={() => setOpen((o) => !o)}
            aria-expanded={open}
            className="flex w-full items-center gap-1.5 px-4 py-2 text-left text-sm font-medium text-ink"
          >
            {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            Messages
            {messages.unread > 0 && <span className="rounded-full bg-brand-600 px-1.5 text-[10px] font-semibold leading-4 text-white">{messages.unread}</span>}
          </button>
          {open && (
            <div className="px-4 pb-3">
              <MessageFeed
                route="runs"
                space={messages}
                notLiveReason={live ? null : 'This run has finished: messages only reach a running run.'}
                highlightId={parseMsgHash(hash)}
              />
            </div>
          )}
        </section>
      )}
      <div className="flex-1 min-h-0">
        <MessageLinksContext.Provider value={present ? links : {}}>
          <TranscriptPanel path={path} live={live} host={host} runId={run} onComplete={onComplete} />
        </MessageLinksContext.Provider>
      </div>
    </>
  );
}

/** Collapsible; hidden until the run turns out to have children. */
function ChildrenSection({ runId }: { runId: string }) {
  const [count, setCount] = useState(0);
  const [open, setOpen] = useState(true);
  return (
    <section className={count > 0 ? 'rounded-xl border border-border bg-panel' : 'hidden'}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex w-full items-center gap-1.5 px-4 py-2 text-left text-sm font-medium text-ink"
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        Children ({count})
      </button>
      <div className={open ? 'px-4 pb-3' : 'hidden'}>
        <ChildrenPanel key={runId} runId={runId} onCount={setCount} />
      </div>
    </section>
  );
}

function BackButton({ navigate }: { navigate: ReturnType<typeof useNavigate> }) {
  return (
    <button
      type="button"
      onClick={() => navigate(-1)}
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Back
    </button>
  );
}
