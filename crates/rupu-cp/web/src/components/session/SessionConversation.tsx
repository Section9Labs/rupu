// Conversation-style render of a session's turn-runs (CP Phase 2e).
//
// A session is a container of turn-runs (oldest→newest). Each turn renders as a
// chat exchange: a right-aligned "You" bubble with the operator's prompt, then
// the agent's response via an embedded `TranscriptPanel` (which streams live
// over SSE while the turn is in flight). The newest turn sits at the BOTTOM; a
// "Load older turns" button at the TOP widens the visible window.

import { useEffect, useRef, useState } from 'react';
import TranscriptPanel from '../TranscriptPanel';
import { Button } from '../ui/Button';
import type { SessionSummary, SessionRunRow } from '../../lib/api';

/** How many turns to reveal initially, and per "Load older" click. */
const PAGE = 10;

/**
 * A turn is "active" (still streaming) when it is the session's active run, or
 * when its status is null/absent — the wire contract for an in-flight turn.
 */
function isActive(run: SessionRunRow, session: SessionSummary): boolean {
  return run.run_id === session.active_run_id || run.status == null;
}

export default function SessionConversation({
  session,
  runs,
  onTurnComplete,
}: {
  session: SessionSummary;
  /** Turn-runs in stored order (oldest→newest). */
  runs: SessionRunRow[];
  /** Called when the active turn's transcript stream fires run_complete/run_failed.
   *  Allows the parent (SessionDetail) to reload session + runs immediately. */
  onTurnComplete?: () => void;
}) {
  const [visible, setVisible] = useState(PAGE);

  // Scroll bookkeeping: only auto-scroll to the bottom when the operator is
  // already near it — never yank them down while they're reading older turns.
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const nearBottomRef = useRef(true);

  // Newest run id (last element, since runs are oldest→newest).
  const newestId = runs.length > 0 ? runs[runs.length - 1].run_id : null;

  // Auto-scroll on mount and whenever the newest turn changes — but only if the
  // operator hasn't scrolled away from the bottom.
  useEffect(() => {
    if (nearBottomRef.current) {
      bottomRef.current?.scrollIntoView({ block: 'end' });
    }
  }, [newestId]);

  function handleScroll() {
    const el = scrollRef.current;
    if (!el) return;
    // Within ~120px of the bottom counts as "near bottom".
    nearBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  }

  if (runs.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto px-4 py-10">
        <p className="text-center text-sm text-ink-dim">No messages yet — send one below.</p>
      </div>
    );
  }

  const shown = runs.slice(Math.max(0, runs.length - visible));
  const hasOlder = runs.length > visible;

  return (
    <div
      ref={scrollRef}
      onScroll={handleScroll}
      className="flex-1 overflow-y-auto px-4 py-4"
    >
      {hasOlder && (
        <div className="mb-4 flex justify-center">
          <Button variant="secondary" onClick={() => setVisible((v) => v + PAGE)}>
            Load older turns
          </Button>
        </div>
      )}

      <div className="flex flex-col gap-6">
        {shown.map((run) => (
          <div key={run.run_id} className="flex flex-col gap-2">
            {/* User bubble — right-aligned, tinted. */}
            <div className="flex justify-end">
              <div className="max-w-[80%] rounded-2xl rounded-br-sm bg-brand-600 px-3.5 py-2 text-sm text-white">
                <div className="mb-0.5 text-meta font-medium uppercase tracking-wide text-white/70">
                  You
                </div>
                <div className="whitespace-pre-wrap break-words">{run.prompt}</div>
              </div>
            </div>

            {/* Agent response — embedded transcript, live while in flight. */}
            <TranscriptPanel
              path={run.transcript_path}
              live={isActive(run, session)}
              embedded
              onComplete={isActive(run, session) ? onTurnComplete : undefined}
            />

            {/* Per-turn error line (shown when the run terminated with an error). */}
            {run.error && (
              <p className="text-note text-err">{run.error}</p>
            )}
          </div>
        ))}
      </div>

      <div ref={bottomRef} />
    </div>
  );
}
