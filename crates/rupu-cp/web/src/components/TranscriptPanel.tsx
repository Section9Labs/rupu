// Conversation-style render of an agent run's transcript (Slice A, mockup A).
//
// All the event → render structure lives in the pure `buildTranscriptView`
// mapping (transcript/transcriptView.ts, unit-tested). This component is the
// thin React shell: fetch + (optional) live subscribe, then paint the upgraded
// view model — a header/footer chrome plus one collapsible `Turn` per turn
// (markdown assistant message + per-tool `ToolCard`s + finding cards).

import { useEffect, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { AlertTriangle } from 'lucide-react';
import { api, apiErrorMessage } from '../lib/api';
import type { TranscriptEvent } from '../lib/transcript';
import { cn } from '../lib/cn';
import { formatDuration } from '../lib/duration';
import { formatCost } from '../lib/usage';
import { Spinner } from './ui/Spinner';
import { buildTranscriptView } from './transcript/transcriptView';
import { parseMsgHash } from '../lib/messages';
import Turn from './transcript/Turn';
import TranscriptControls from './transcript/TranscriptControls';
import TranscriptOutline from './transcript/TranscriptOutline';
import { TranscriptViewProvider } from './transcript/TranscriptViewContext';
import { filterCounts } from './transcript/eventKind';
import type { FilterKind } from './transcript/eventKind';
import { AgentName } from './codename/AgentName';

type LoadState = 'loading' | 'ready' | 'error';

/**
 * Combine the REST snapshot with the live SSE stream into one event list.
 *
 * The CP tailer replays the transcript file from byte 0 once per connection
 * (then follows it), so the snapshot and the stream are each a PREFIX of the
 * same event sequence — the stream is not "the events after the snapshot".
 * Appending one to the other would render the whole backlog twice, so the
 * longer prefix wins: the snapshot paints immediately, and once the stream has
 * replayed past it the stream takes over (and keeps growing).
 *
 * ASSUMES the tailed transcript is append-only. "Longer wins" (and the
 * reconnect replay buffer in the live-tail effect, which swaps a replay in
 * only once it is at least as long as what is shown) is only right while
 * every copy is a prefix of one growing file. If the file were REPLACED by
 * shorter content (e.g. a mirror's authoritative rewrite that drops lines),
 * both would keep showing the old, longer stream: the view freezes on stale
 * events until the new file outgrows them. Today's rewrite (a host mirror's
 * terminal pull) replaces a transcript with a superset of what was tailed, so
 * this holds; a writer that could shrink one must also reset this view.
 */
export function mergeSnapshotAndStream<T>(snapshot: T[], stream: T[]): T[] {
  return stream.length >= snapshot.length ? stream : snapshot;
}

export default function TranscriptPanel({
  path,
  live,
  host,
  runId,
  embedded = false,
  onComplete,
  costUsd,
}: {
  path: string;
  live: boolean;
  /** Remote host id to forward to the backend transcript endpoints. When set,
   *  appended as `&host=<id>` on both the fetch and the SSE stream URLs. Also
   *  threaded down to the source-preview affordance (see `runId`). */
  host?: string;
  /**
   * The run's id, when known — threaded down through `Turn`/`ToolCard` to
   * power the clickable `path:line` → `SourcePreview` affordance on ast_grep
   * match headers and finding location chips (`GET /api/runs/:id/source`
   * keys on this id). Absent in render contexts where no resolvable run id
   * exists (e.g. the standalone agent-run transcript page) — those
   * references degrade gracefully to non-clickable text.
   */
  runId?: string;
  /**
   * When true, hide the run-level header and usage footer chrome — render only
   * the turn/tool conversation body. Used inside a chat conversation, where each
   * turn shouldn't repeat a big per-run header/footer. Live SSE streaming and the
   * Turn / ToolCard rendering are unchanged. Default (false) is the full panel.
   */
  embedded?: boolean;
  /** Called once when the live SSE stream receives a run_complete or run_failed
   *  event. Allows the parent to trigger a reload on turn completion. */
  onComplete?: () => void;
  /** The run's cost in USD, when the host page already has it. Shown in the
   *  metrics strip only when provided — the feed never fetches or invents it. */
  costUsd?: number;
}) {
  // The REST snapshot and the live SSE stream are kept apart: both start at
  // byte 0 of the same file, so each is a prefix of the other (see
  // `mergeSnapshotAndStream`). Rendering appends neither to the other.
  const [snapshot, setSnapshot] = useState<TranscriptEvent[]>([]);
  const [stream, setStream] = useState<TranscriptEvent[]>([]);
  const [state, setState] = useState<LoadState>('loading');
  const [errorMsg, setErrorMsg] = useState<string>('');
  const [connected, setConnected] = useState(false);
  // Lines the server couldn't parse in the REST snapshot (absent on older
  // servers, or when there were none). Live-tail SSE events don't affect this
  // count — the badge covers the snapshot only; torn tails are excluded
  // server-side.
  const [unparsed, setUnparsed] = useState<number | undefined>(undefined);
  // The coordinator could not collect the rest of this transcript from its
  // host (spec §4.2). Snapshot-only, like `unparsed` — live-tail SSE events
  // don't affect it.
  const [partial, setPartial] = useState(false);
  const navigate = useNavigate();
  const location = useLocation();
  // Reader view state: the event-kind filter and the expand-all/collapse-all
  // signal (a new nonce re-applies the target even when it repeats).
  const [filter, setFilter] = useState<FilterKind>('all');
  const [expandTarget, setExpandTarget] = useState<'open' | 'closed' | null>(null);
  const [expandNonce, setExpandNonce] = useState(0);
  const expandAll = (target: 'open' | 'closed') => {
    setExpandTarget(target);
    setExpandNonce((n) => n + 1);
  };

  // Open a sub-run transcript in the same transcript route (reuses the existing
  // `/transcript?path=…` page — no new global state). Sub-runs are completed
  // recordings, so `live=0`.
  //
  // `host` and `run` travel with it: a sub-run of a REMOTE run is itself a
  // remote path, and the backend needs both to resolve it (`run` is what
  // authorizes a not-yet-mirrored path against the run that claims it).
  // Dropping them here is what made remote sub-run links 400.
  function openTranscript(p: string) {
    let url = `/transcript?path=${encodeURIComponent(p)}&live=0`;
    if (host !== undefined) url += `&host=${encodeURIComponent(host)}`;
    if (runId !== undefined) url += `&run=${encodeURIComponent(runId)}`;
    navigate(url);
  }

  // Fetch on mount + whenever `path` changes; reset state on path change.
  useEffect(() => {
    let cancelled = false;
    setState('loading');
    setSnapshot([]);
    setStream([]);
    setConnected(false);
    setUnparsed(undefined);
    setPartial(false);

    api
      .getTranscript(path, { host, run: runId })
      .then((res) => {
        if (cancelled) return;
        setSnapshot(res.events);
        setUnparsed(res.unparsed);
        setPartial(res.partial === true);
        setState('ready');
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setErrorMsg(apiErrorMessage(err));
        setState('error');
      });

    return () => {
      cancelled = true;
    };
  }, [path, host, runId]);

  // Live tail: accumulate the stream's own copy of the transcript; close on
  // unmount / path change. Kept in a ref so the cleanup always closes the
  // EventSource we actually opened.
  const unsubRef = useRef<(() => void) | null>(null);
  // Guard so onComplete fires at most once per live session (even if duplicate
  // run_complete / run_failed events arrive).
  const completedRef = useRef(false);
  // Keep onComplete stable inside the effect without adding it to the dep array.
  const onCompleteRef = useRef(onComplete);
  useEffect(() => {
    onCompleteRef.current = onComplete;
  });
  useEffect(() => {
    completedRef.current = false;
    if (!live) return;
    setConnected(true);
    // A fresh subscription replays the file from byte 0, so start its copy
    // empty (covers `live` flipping off and back on for the same path).
    setStream([]);
    // `shown` mirrors what `stream` state holds. EventSource auto-reconnects
    // after an error. This server numbers events and resumes after the last
    // one (handled first, below); an older remote CP replays from byte 0
    // again instead, and for that stream extending the
    // shown copy would duplicate the backlog, and restarting it would rewind
    // the view (the merge falls back to the shorter snapshot) until the replay
    // caught up. So a post-error replay is buffered in `replay` and swapped in
    // only once it is at least as long as what is already on screen — the
    // previous stream stays visible until then, and the view never shrinks.
    // Like `mergeSnapshotAndStream`, this assumes the file is append-only: a
    // replay of a SHORTER replacement never reaches `shown.length`, so the old
    // stream would stay on screen (see that function's note).
    let shown: TranscriptEvent[] = [];
    let replay: TranscriptEvent[] | null = null;
    let reconnected = false;
    // Numbered events (`id` = line number): a reconnect resumes after the
    // last one (`Last-Event-ID`), so append anything newer and drop repeats.
    // The replay buffering below is only for an older remote CP whose
    // stream carries no ids and replays from the start.
    let lastId = 0;
    const unsub = api.subscribeTranscript(
      path,
      (e, id) => {
        if (id !== undefined) {
          if (id <= lastId) return;
          lastId = id;
          shown = [...shown, e];
          setStream(shown);
        } else {
          if (reconnected) {
            reconnected = false;
            replay = [];
          }
          if (replay) {
            replay.push(e);
            if (replay.length >= shown.length) {
              shown = replay;
              replay = null;
              setStream(shown);
            }
          } else {
            shown = [...shown, e];
            setStream(shown);
          }
        }
        if (
          !completedRef.current &&
          (e.type === 'run_complete' || e.type === 'run_failed')
        ) {
          completedRef.current = true;
          onCompleteRef.current?.();
        }
      },
      () => {
        setConnected(false);
        reconnected = true;
      },
      { host, run: runId },
    );
    unsubRef.current = unsub;
    return () => {
      unsub();
      unsubRef.current = null;
    };
  }, [path, live, host, runId]);

  const view = buildTranscriptView(mergeSnapshotAndStream(snapshot, stream));
  // `#msg-<id>`: the turn holding the bubble opens and the bubble scrolls
  // itself into view (Turn, MessageBubble).
  const highlightMsg = parseMsgHash(location.hash);
  // An anchored message must not be filtered out: reset to `all` whenever a
  // `#msg-<id>` anchor appears.
  useEffect(() => {
    if (highlightMsg) setFilter('all');
  }, [highlightMsg]);

  // ---- non-ready states ----------------------------------------------------

  if (state === 'loading') {
    return (
      <div className="flex items-center justify-center gap-2 p-8">
        <Spinner label="Loading transcript…" />
      </div>
    );
  }

  if (state === 'error') {
    return (
      <div className="m-3 rounded-lg border border-err/30 bg-err-bg p-4 text-sm text-err">
        <div className="flex items-center gap-1.5 font-medium">
          <AlertTriangle size={14} />
          Failed to load transcript
        </div>
        <div className="mt-1 text-xs text-err">{errorMsg}</div>
      </div>
    );
  }

  const isEmpty = !view.header && view.turns.length === 0;

  // ---- ready ---------------------------------------------------------------

  const counts = filterCounts(view.turns);
  const toolCalls = view.turns.reduce((n, t) => n + t.summary.toolCount, 0);
  const totalTokens = view.footer?.totalTokens;
  const hasChrome =
    view.header != null || (typeof unparsed === 'number' && unparsed > 0) || partial;

  // The feed: every turn mounted up front (the outline observes `#turn-<n>`).
  const feed = (
    <div className="flex min-w-0 flex-1 flex-col">
      {isEmpty && (
        <div className="p-6 text-center text-sm text-ink-mute">No transcript events yet.</div>
      )}
      {view.turns.map((turn, ti) => (
        <div key={ti} id={`turn-${ti + 1}`}>
          <Turn
            turn={turn}
            turnNumber={ti + 1}
            showMarker={!embedded}
            tokensLabel={
              turn.tokensIn != null || turn.tokensOut != null
                ? `${(turn.tokensIn ?? 0).toLocaleString()} in · ${(turn.tokensOut ?? 0).toLocaleString()} out`
                : undefined
            }
            onOpenTranscript={openTranscript}
            runId={runId}
            host={host}
            self={view.self}
            highlightMsg={highlightMsg}
          />
        </div>
      ))}
    </div>
  );

  if (embedded) {
    return (
      <TranscriptViewProvider value={{ filter, expandTarget, expandNonce, live }}>
        <div className="flex flex-col text-note">{feed}</div>
      </TranscriptViewProvider>
    );
  }

  return (
    <div className="flex flex-col rounded-xl border border-border bg-bg p-3 text-note">
      {/* Run header: agent · provider · model · live · data-quality badges.
          Renders even without a `run_start` header (headerless snapshots) when
          the unparsed/partial badges have something to show. */}
      {hasChrome && (
        <div className="mb-2 flex flex-wrap items-center gap-2 border-b border-border pb-1.5 text-note text-ink-dim">
          {view.header &&
            (view.header.codename ? (
              <span className="text-ink">
                <AgentName
                  codename={view.header.codename}
                  agent={view.header.agent || undefined}
                  provider={view.header.provider || undefined}
                  model={view.header.model || undefined}
                />
              </span>
            ) : (
              <>
                <b className="text-ink">{view.header.agent || 'agent'}</b>
                {view.header.provider && <span>· {view.header.provider}</span>}
                {view.header.model && <span>· {view.header.model}</span>}
              </>
            ))}
          {live && (
            <span
              className={cn(
                'inline-flex items-center gap-1 rounded px-1.5 py-px text-[9px] font-medium',
                connected ? 'bg-status-running/10 text-status-running' : 'bg-surface text-ink-dim',
              )}
            >
              <span
                className={cn(
                  'inline-block h-1.5 w-1.5 rounded-full',
                  connected ? 'animate-pulse bg-status-running' : 'bg-ink-mute',
                )}
              />
              {connected ? 'live' : 'offline'}
            </span>
          )}
          {typeof unparsed === 'number' && unparsed > 0 && (
            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wide ring-1 ring-inset bg-warn-bg text-warn ring-warn/30">
              {unparsed} unparsed {unparsed === 1 ? 'line' : 'lines'}
            </span>
          )}
          {partial && (
            <span
              className="inline-flex items-center gap-1 rounded-md border border-warn/30 bg-warn-bg px-1.5 py-0.5 text-[11px] text-warn"
              title="The coordinator could not reach the host to collect the rest of this transcript"
            >
              <AlertTriangle className="h-3 w-3" />
              incomplete
            </span>
          )}
          {view.footer?.status && (
            <span className="ml-auto">
              {statusGlyph(view.footer.status)} {view.footer.status}
            </span>
          )}
        </div>
      )}

      {/* Metrics strip: the run at a glance. Cost only where the host page has it. */}
      {!isEmpty && (
        <div
          className="mb-2 flex flex-wrap gap-x-4 gap-y-1 text-meta tabular-nums text-ink-dim"
          data-testid="transcript-metrics"
        >
          <span>{view.turns.length} {view.turns.length === 1 ? 'turn' : 'turns'}</span>
          <span>{toolCalls} tool {toolCalls === 1 ? 'call' : 'calls'}</span>
          <span>{counts.commands} {counts.commands === 1 ? 'command' : 'commands'}</span>
          <span>{counts.findings} {counts.findings === 1 ? 'finding' : 'findings'}</span>
          {totalTokens != null && <span>{totalTokens.toLocaleString()} tokens</span>}
          {costUsd !== undefined && <span>{formatCost(costUsd)}</span>}
          {view.footer?.durationMs != null && <span>{formatDuration(view.footer.durationMs)}</span>}
          {view.footer?.error && <span className="text-err">{view.footer.error}</span>}
        </div>
      )}

      <TranscriptViewProvider value={{ filter, expandTarget, expandNonce, live }}>
        {!isEmpty && (
          <TranscriptControls
            counts={counts}
            filter={filter}
            onFilter={setFilter}
            onExpandAll={() => expandAll('open')}
            onCollapseAll={() => expandAll('closed')}
          />
        )}
        <div className="flex gap-3">
          {feed}
          {!isEmpty && <TranscriptOutline turns={view.turns} filter={filter} />}
        </div>
      </TranscriptViewProvider>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

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
