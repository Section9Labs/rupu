// Conversation-style render of an agent run's transcript (Slice A, mockup A).
//
// All the event → render structure lives in the pure `buildTranscriptView`
// mapping (transcript/transcriptView.ts, unit-tested). This component is the
// thin React shell: fetch + (optional) live subscribe, expand toggles, and the
// bubbles/cards/chips paint. Thinking + tool I/O are collapsed by default with
// a one-click expand, matching `.superpowers/brainstorm/.../transcript-content.html`.

import { useEffect, useRef, useState } from 'react';
import { ChevronRight, Wrench, Loader2, AlertTriangle } from 'lucide-react';
import { api } from '../lib/api';
import type { TranscriptEvent } from '../lib/transcript';
import { cn } from '../lib/cn';
import {
  buildTranscriptView,
  type TranscriptItem,
  type ToolItem,
} from './transcript/transcriptView';

type LoadState = 'loading' | 'ready' | 'error';

export default function TranscriptPanel({ path, live }: { path: string; live: boolean }) {
  const [events, setEvents] = useState<TranscriptEvent[]>([]);
  const [state, setState] = useState<LoadState>('loading');
  const [errorMsg, setErrorMsg] = useState<string>('');
  const [connected, setConnected] = useState(false);
  // Expand toggles, keyed by item key. Collapsed by default.
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  function toggle(key: string) {
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));
  }

  // Fetch on mount + whenever `path` changes; reset state on path change.
  useEffect(() => {
    let cancelled = false;
    setState('loading');
    setEvents([]);
    setExpanded({});
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

  const isEmpty = !view.header && view.items.length === 0;

  // ---- ready ---------------------------------------------------------------

  return (
    <div className="flex flex-col rounded-xl border border-border bg-bg p-3 text-[11px]">
      {/* Header: agent · provider · model · live · status / tokens */}
      {view.header && (
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

      {/* Conversation */}
      <div className="flex flex-col gap-1.5">
        {view.items.map((item) => (
          <ItemView
            key={item.key}
            item={item}
            expanded={!!expanded[item.key]}
            onToggle={() => toggle(item.key)}
          />
        ))}
      </div>

      {/* Footer: status · total tokens · duration */}
      {view.footer && (
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
// Per-item render
// ---------------------------------------------------------------------------

function ItemView({
  item,
  expanded,
  onToggle,
}: {
  item: TranscriptItem;
  expanded: boolean;
  onToggle: () => void;
}) {
  switch (item.kind) {
    case 'user':
      return (
        <div>
          <div className="mb-0.5 text-[9px] font-bold uppercase tracking-wide text-ink-dim">user</div>
          <div className="whitespace-pre-wrap rounded-lg border border-border bg-bg px-2.5 py-1.5 text-ink">
            {item.content}
          </div>
        </div>
      );

    case 'assistant':
      return (
        <div>
          <div className="mb-0.5 text-[9px] font-bold uppercase tracking-wide text-brand-500">
            assistant
          </div>
          {item.thinking && <Thinking text={item.thinking} expanded={expanded} onToggle={onToggle} />}
          {item.content && (
            <div className="whitespace-pre-wrap rounded-lg border border-border bg-panel px-2.5 py-1.5 text-ink">
              {item.content}
            </div>
          )}
        </div>
      );

    case 'tool':
      return <ToolCard item={item} expanded={expanded} onToggle={onToggle} />;

    case 'finding':
      return (
        <div className="rounded-lg border border-red-200 bg-red-50 px-2.5 py-1.5 text-[10px] text-red-700">
          <span className="font-semibold">
            ⚑ {item.severity ? `${item.severity.toUpperCase()} · ` : ''}
          </span>
          {item.title}
        </div>
      );

    case 'chip':
      return (
        <div className="inline-flex w-fit items-center gap-1.5 rounded-md border border-border bg-panel px-2 py-0.5 font-mono text-[10px] text-ink-dim">
          <span className="text-ink-mute">{item.variant === 'command_run' ? '$' : '✎'}</span>
          <span className="truncate">{item.label}</span>
        </div>
      );

    case 'event':
      return (
        <div className="px-1 py-0.5 font-mono text-[10px] text-ink-mute">
          <span className="text-ink-dim">{item.type}</span> {item.label}
        </div>
      );
  }
}

function Thinking({
  text,
  expanded,
  onToggle,
}: {
  text: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className="my-0.5 block w-full border-l-2 border-border pl-2 text-left text-[10px] italic text-ink-mute"
    >
      <span className="font-medium not-italic text-ink-mute">▸ thinking</span>{' '}
      {expanded ? (
        <span className="whitespace-pre-wrap">{text}</span>
      ) : (
        <>
          {truncate(text, 90)} <span className="text-ink-mute/70">(click to expand)</span>
        </>
      )}
    </button>
  );
}

function ToolCard({
  item,
  expanded,
  onToggle,
}: {
  item: ToolItem;
  expanded: boolean;
  onToggle: () => void;
}) {
  const inputPreview = item.input === undefined ? '' : previewInput(item.input);
  const inFlight = item.result === null && item.tool !== null;
  const hasError = !!item.result?.error;

  return (
    <div
      className={cn(
        'rounded-lg border text-[10px]',
        hasError ? 'border-red-200 bg-red-50' : 'border-blue-200 bg-blue-50',
      )}
    >
      <button
        type="button"
        onClick={onToggle}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-left"
      >
        <ChevronRight
          size={11}
          className={cn('shrink-0 transition-transform', expanded && 'rotate-90')}
        />
        <Wrench size={11} className={cn('shrink-0', hasError ? 'text-red-600' : 'text-blue-600')} />
        <span className={cn('font-mono font-bold', hasError ? 'text-red-700' : 'text-blue-700')}>
          {item.tool ?? 'tool_result'}
        </span>
        {inputPreview && (
          <span className="truncate font-mono text-ink-dim">{inputPreview}</span>
        )}
        {inFlight && <Loader2 size={10} className="ml-auto shrink-0 animate-spin text-blue-500" />}
        {item.result?.durationMs != null && (
          <span className="ml-auto shrink-0 tabular-nums text-ink-mute">
            {item.result.durationMs}ms
          </span>
        )}
      </button>

      {expanded && (
        <div className="border-t border-dashed border-blue-200 px-2 py-1.5 font-mono text-ink-dim">
          {item.input !== undefined && (
            <pre className="mb-1 whitespace-pre-wrap break-words text-[10px] text-ink-dim">
              {fullInput(item.input)}
            </pre>
          )}
          {item.result && (
            <div
              className={cn(
                'whitespace-pre-wrap break-words',
                hasError ? 'text-red-700' : 'text-ink',
              )}
            >
              {item.result.error ? `✕ ${item.result.error}` : `→ ${item.result.output}`}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function truncate(s: string, max: number): string {
  const flat = s.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

function previewInput(input: unknown): string {
  if (typeof input === 'string') return truncate(input, 60);
  try {
    return truncate(JSON.stringify(input), 60);
  } catch {
    return '';
  }
}

function fullInput(input: unknown): string {
  if (typeof input === 'string') return input;
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    return String(input);
  }
}

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
