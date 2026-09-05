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

import { useSearchParams, useNavigate } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import TranscriptPanel from '../components/TranscriptPanel';

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

  return (
    <div className="flex flex-col p-8 gap-4">
      <BackButton navigate={navigate} />

      <header>
        <h1 className="text-xl font-semibold text-ink font-mono break-all">{label}</h1>
        <p className="mt-0.5 text-note text-ink-dim font-mono truncate" title={path}>
          {path}
        </p>
      </header>

      {/* TranscriptPanel fills the remaining page height and handles its own scroll */}
      <div className="flex-1 min-h-0">
        <TranscriptPanel path={path} live={live} host={host} runId={run} />
      </div>
    </div>
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
