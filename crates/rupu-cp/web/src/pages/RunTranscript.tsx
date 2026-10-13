// Transcript-only page — renders a TranscriptPanel for agent/session/standalone
// runs that have no workflow DAG. Path, live flag, host and run come from query
// params:
//   /transcript?path=<encoded-path>&live=<0|1>[&host=<id>&run=<id>]
//
// `host` and `run` are what let a REMOTE transcript resolve at all: the backend
// needs `run` to authorize a not-yet-mirrored path against the run that claims
// it (spec §3.3). Without it a sub-run opened from a remote run's panel 400s.
//
// Route: /transcript (registered in App.tsx)

import { useState } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { ArrowLeft, ChevronDown, ChevronRight } from 'lucide-react';
import TranscriptPanel from '../components/TranscriptPanel';
import ChildrenPanel from '../components/run/ChildrenPanel';

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
  const childRunId = transcriptRunId(path);

  return (
    <div className="flex flex-col p-8 gap-4">
      <BackButton navigate={navigate} />

      <header>
        <h1 className="text-xl font-semibold text-ink font-mono break-all">{label}</h1>
        <p className="mt-0.5 text-note text-ink-dim font-mono truncate" title={path}>
          {path}
        </p>
      </header>

      {/* The children ledger is local-only: no section for a remote run. */}
      {childRunId && (!host || host === 'local') && <ChildrenSection runId={childRunId} />}

      {/* TranscriptPanel fills the remaining page height and handles its own scroll */}
      <div className="flex-1 min-h-0">
        <TranscriptPanel path={path} live={live} host={host} runId={run} />
      </div>
    </div>
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
