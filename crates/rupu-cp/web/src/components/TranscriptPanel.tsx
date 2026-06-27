// Conversation-style render of an agent run's transcript (Slice A, mockup A).
//
// All the event → render structure lives in the pure `buildTranscriptView`
// mapping (transcript/transcriptView.ts, unit-tested). This component is the
// thin React shell: fetch + (optional) live subscribe, then paint the upgraded
// view model — a header/footer chrome plus one collapsible `Turn` per turn
// (markdown assistant message + per-tool `ToolCard`s + finding cards).

import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Loader2, AlertTriangle } from 'lucide-react';
import { api } from '../lib/api';
import type { TranscriptEvent } from '../lib/transcript';
import { cn } from '../lib/cn';
import { buildTranscriptView } from './transcript/transcriptView';
import Turn from './transcript/Turn';

type LoadState = 'loading' | 'ready' | 'error';

export default function TranscriptPanel({
  path,
  live,
  embedded = false,
}: {
  path: string;
  live: boolean;
  /**
   * When true, hide the run-level header and usage footer chrome — render only
   * the turn/tool conversation body. Used inside a chat conversation, where each
   * turn shouldn't repeat a big per-run header/footer. Live SSE streaming and the
   * Turn / ToolCard rendering are unchanged. Default (false) is the full panel.
   */
  embedded?: boolean;
}) {
  const [events, setEvents] = useState<TranscriptEvent[]>([]);
  const [state, setState] = useState<LoadState>('loading');
  const [errorMsg, setErrorMsg] = useState<string>('');
  const [connected, setConnected] = useState(false);
  const navigate = useNavigate();

  // Open a sub-run transcript in the same transcript route (reuses the existing
  // `/transcript?path=…` page — no new global state). Sub-runs are completed
  // recordings, so `live=0`.
  function openTranscript(p: string) {
    navigate(`/transcript?path=${encodeURIComponent(p)}&live=0`);
  }

  // Fetch on mount + whenever `path` changes; reset state on path change.
  useEffect(() => {
    let cancelled = false;
    setState('loading');
    setEvents([]);
    setConnected(false);

    api
      .getTranscript(path)
      .then((res) => {
        if (cancelled) return;
        setEvents(res.events);
        setState('ready');
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setErrorMsg(err instanceof Error ? err.message : String(err));
        setState('error');
      });

    return () => {
      cancelled = true;
    };
  }, [path]);

  // Live tail: append new events; close on unmount / path change. Kept in a ref
  // so the cleanup always closes the EventSource we actually opened.
  const unsubRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    if (!live) return;
    setConnected(true);
    const unsub = api.subscribeTranscript(
      path,
      (e) => setEvents((prev) => [...prev, e]),
      () => setConnected(false),
    );
    unsubRef.current = unsub;
    return () => {
      unsub();
      unsubRef.current = null;
    };
  }, [path, live]);

  const view = buildTranscriptView(events);

  // ---- non-ready states ----------------------------------------------------

  if (state === 'loading') {
    return (
      <div className="flex items-center justify-center gap-2 p-8 text-sm text-ink-dim">
        <Loader2 size={14} className="animate-spin" />
        Loading transcript…
      </div>
    );
  }

  if (state === 'error') {
    return (
      <div className="m-3 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
        <div className="flex items-center gap-1.5 font-medium">
          <AlertTriangle size={14} />
          Failed to load transcript
        </div>
        <div className="mt-1 text-xs text-red-600">{errorMsg}</div>
      </div>
    );
  }

  const isEmpty = !view.header && view.turns.length === 0;

  // ---- ready ---------------------------------------------------------------

  return (
    <div className="flex flex-col rounded-xl border border-border bg-bg p-3 text-[11px]">
      {/* Header: agent · provider · model · live · status / tokens */}
      {!embedded && view.header && (
        <div className="mb-2 flex flex-wrap items-center gap-2 border-b border-border pb-1.5 text-[11px] text-ink-dim">
          <b className="text-ink">{view.header.agent || 'agent'}</b>
          {view.header.provider && <span>· {view.header.provider}</span>}
          {view.header.model && <span>· {view.header.model}</span>}
          {live && (
            <span
              className={cn(
                'inline-flex items-center gap-1 rounded px-1.5 py-px text-[9px] font-medium',
                connected ? 'bg-blue-100 text-blue-700' : 'bg-slate-100 text-slate-500',
              )}
            >
              <span
                className={cn(
                  'inline-block h-1.5 w-1.5 rounded-full',
                  connected ? 'animate-pulse bg-blue-500' : 'bg-slate-400',
                )}
              />
              {connected ? 'live' : 'offline'}
            </span>
          )}
          {view.footer?.totalTokens != null && (
            <span className="ml-auto tabular-nums">
              {view.footer.totalTokens.toLocaleString()} tok
            </span>
          )}
        </div>
      )}

      {isEmpty && (
        <div className="p-6 text-center text-sm text-ink-mute">No transcript events yet.</div>
      )}

      {/* Turns — last turn expanded by default, earlier turns collapsed. */}
      <div className="flex flex-col gap-1.5">
        {view.turns.map((turn, ti) => (
          <Turn
            key={ti}
            turn={turn}
            defaultOpen={ti === view.turns.length - 1}
            onOpenTranscript={openTranscript}
          />
        ))}
      </div>

      {/* Footer: status · total tokens · duration */}
      {!embedded && view.footer && (
        <div className="mt-2 flex flex-wrap gap-3 border-t border-border pt-1.5 text-[10px] text-ink-dim">
          {view.footer.status && <span>{statusGlyph(view.footer.status)} {view.footer.status}</span>}
          {view.footer.totalTokens != null && (
            <span className="tabular-nums">{view.footer.totalTokens.toLocaleString()} tokens</span>
          )}
          {view.footer.durationMs != null && (
            <span className="tabular-nums">{formatDuration(view.footer.durationMs)}</span>
          )}
          {view.footer.error && <span className="text-red-600">{view.footer.error}</span>}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)}s`;
  const m = Math.floor(s / 60);
  return `${m}m ${Math.round(s % 60)}s`;
}

function statusGlyph(status: string): string {
  switch (status) {
    case 'completed':
      return '✓';
    case 'failed':
    case 'rejected':
      return '✕';
    case 'awaiting_approval':
      return '⏸';
    default:
      return '•';
  }
}
