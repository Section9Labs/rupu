/**
 * Turn — one collapsible assistant→tools group in the transcript.
 *
 * Collapsed (default for all but the last turn): a single summary header row —
 * an assistant-snippet (first ~100 chars, plain text) plus pills for tool count,
 * finding count (only when > 0), and a result pill (ok / error / running).
 *
 * Expanded: the same header (chevron down) then the body — the assistant message
 * rendered as markdown, an optional collapsible thinking block, then each tool
 * dispatched through `ToolCard`.
 *
 * No `any`. Static Tailwind class strings only.
 */

import { useState } from 'react';
import { ChevronRight, ChevronDown, Wrench, AlertTriangle } from 'lucide-react';
import Markdown from './Markdown';
import ToolCard from './ToolCard';
import type { TurnView } from './transcriptView';
import { cn } from '../../lib/cn';

// ---------------------------------------------------------------------------
// Static class maps (no interpolation)
// ---------------------------------------------------------------------------

const RESULT_PILL: Record<TurnView['summary']['result'], string> = {
  ok: 'bg-green-100 text-green-700 ring-green-200',
  error: 'bg-red-100 text-red-700 ring-red-200',
  running: 'bg-amber-100 text-amber-700 ring-amber-200',
};

const RESULT_LABEL: Record<TurnView['summary']['result'], string> = {
  ok: 'ok',
  error: 'error',
  running: 'running',
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** First ~100 chars of the assistant content, whitespace-flattened. */
function snippet(content: string | undefined): string {
  if (!content) return '';
  const flat = content.replace(/\s+/g, ' ').trim();
  return flat.length > 100 ? `${flat.slice(0, 99)}…` : flat;
}

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

export default function Turn({
  turn,
  defaultOpen,
  onOpenTranscript,
}: {
  turn: TurnView;
  defaultOpen: boolean;
  onOpenTranscript?: (path: string) => void;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [showThinking, setShowThinking] = useState(false);

  const { toolCount, findingCount, result } = turn.summary;
  const content = turn.assistant?.content ?? '';
  const thinking = turn.assistant?.thinking;

  return (
    <div className="rounded-xl border border-border bg-panel">
      {/* Summary header row — always rendered; toggles open */}
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        {open ? (
          <ChevronDown size={13} className="shrink-0 text-ink-mute" />
        ) : (
          <ChevronRight size={13} className="shrink-0 text-ink-mute" />
        )}

        <span className="min-w-0 flex-1 truncate text-[11px] text-ink-dim">
          {snippet(content) || (
            <span className="italic text-ink-mute">no assistant message</span>
          )}
        </span>

        {/* Pills */}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          {toolCount > 0 && (
            <span className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-medium bg-slate-100 text-slate-600">
              <Wrench size={9} />
              {toolCount} {toolCount === 1 ? 'tool' : 'tools'}
            </span>
          )}
          {findingCount > 0 && (
            <span className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-medium ring-1 ring-inset bg-orange-50 text-sev-medium ring-orange-200">
              <AlertTriangle size={9} />
              {findingCount} {findingCount === 1 ? 'finding' : 'findings'}
            </span>
          )}
          <span
            className={cn(
              'inline-flex items-center rounded px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wide ring-1 ring-inset',
              RESULT_PILL[result],
            )}
          >
            {RESULT_LABEL[result]}
          </span>
        </span>
      </button>

      {/* Body — only when expanded */}
      {open && (
        <div className="flex flex-col gap-1.5 border-t border-border px-3 pb-3 pt-2">
          {/* Assistant message */}
          {content && (
            <div>
              <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-brand-500">
                assistant
              </div>
              <Markdown text={content} />
            </div>
          )}

          {/* Thinking — collapsible, dim */}
          {thinking && (
            <div>
              <button
                type="button"
                onClick={() => setShowThinking((v) => !v)}
                className="flex items-center gap-1 text-[10px] font-medium text-ink-mute"
              >
                {showThinking ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                thinking
              </button>
              {showThinking && (
                <div className="mt-1 border-l-2 border-border pl-2 text-ink-mute opacity-80">
                  <Markdown text={thinking} />
                </div>
              )}
            </div>
          )}

          {/* Tools */}
          {turn.tools.map((tool, i) => (
            <ToolCard key={i} tool={tool} onOpenTranscript={onOpenTranscript} />
          ))}
        </div>
      )}
    </div>
  );
}
