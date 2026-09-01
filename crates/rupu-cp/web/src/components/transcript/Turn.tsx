/**
 * Turn — one collapsible ordered-blocks group in the transcript.
 *
 * Collapsed (default for all but the last turn): a single summary header row —
 * an assistant-snippet (first ~100 chars, plain text, from the turn's first
 * assistant block) plus pills for tool count, finding count (only when > 0),
 * a per-turn tokens pill (when known), and a result pill (ok / error / running).
 *
 * Expanded: the same header (chevron down) then the body — every block in
 * `turn.blocks`, in order, dispatched by kind through `TurnBlockView`:
 * assistant/user markdown, a collapsible thinking block (or a redacted
 * marker), `ToolCard`s, gate/seed/notice/compaction rows, and an `unknown`
 * fallback row so nothing silently vanishes.
 *
 * No `any`. Static Tailwind class strings only.
 */

import { useState } from 'react';
import { ChevronRight, ChevronDown, Wrench, AlertTriangle } from 'lucide-react';
import Markdown from './Markdown';
import ToolCard from './ToolCard';
import type { TurnBlock, TurnView } from './transcriptView';
import { cn } from '../../lib/cn';

// ---------------------------------------------------------------------------
// Static class maps (no interpolation)
// ---------------------------------------------------------------------------

const RESULT_PILL: Record<TurnView['summary']['result'], string> = {
  ok: 'bg-ok-bg text-ok ring-ok/30',
  error: 'bg-err-bg text-err ring-err/30',
  running: 'bg-warn-bg text-warn ring-warn/30',
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
  runId,
  host,
}: {
  turn: TurnView;
  defaultOpen: boolean;
  onOpenTranscript?: (path: string) => void;
  /** Run id threaded down to `ToolCard` for the source-preview affordance
   *  (ast_grep match headers, finding location chips). */
  runId?: string;
  /** Remote host id, threaded alongside `runId`. */
  host?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);

  const { toolCount, findingCount, result } = turn.summary;
  const firstAssistant = turn.blocks.find(
    (b): b is Extract<TurnBlock, { kind: 'assistant' }> => b.kind === 'assistant',
  );

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

        <span className="min-w-0 flex-1 truncate text-note text-ink-dim">
          {snippet(firstAssistant?.content) || (
            <span className="italic text-ink-mute">no assistant message</span>
          )}
        </span>

        {/* Pills */}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          {toolCount > 0 && (
            <span className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-medium bg-surface text-ink">
              <Wrench size={9} />
              {toolCount} {toolCount === 1 ? 'tool' : 'tools'}
            </span>
          )}
          {findingCount > 0 && (
            <span className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-medium ring-1 ring-inset bg-warn-bg text-sev-medium ring-warn/30">
              <AlertTriangle size={9} />
              {findingCount} {findingCount === 1 ? 'finding' : 'findings'}
            </span>
          )}
          {turn.tokensIn !== null && turn.tokensOut !== null && (
            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[9px] font-medium bg-surface text-ink-mute">
              {turn.tokensIn} in · {turn.tokensOut} out
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
          {turn.blocks.map((block, i) => (
            <TurnBlockView
              key={i}
              block={block}
              onOpenTranscript={onOpenTranscript}
              runId={runId}
              host={host}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Per-block rendering
// ---------------------------------------------------------------------------

function TurnBlockView({
  block,
  onOpenTranscript,
  runId,
  host,
}: {
  block: TurnBlock;
  onOpenTranscript?: (path: string) => void;
  runId?: string;
  host?: string;
}) {
  switch (block.kind) {
    case 'assistant':
      return (
        <div>
          <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-brand-500">
            assistant
          </div>
          <Markdown text={block.content} />
        </div>
      );
    case 'thinking':
      return <ThinkingBlock text={block.text} provider={block.provider} />;
    case 'user':
      return (
        <div>
          <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-ink-mute">
            prompt
          </div>
          <Markdown text={block.content} />
        </div>
      );
    case 'tool':
      return (
        <ToolCard tool={block.view} onOpenTranscript={onOpenTranscript} runId={runId} host={host} />
      );
    case 'gate':
      return (
        <div className="rounded-lg border-l-2 border-warn bg-warn-bg/40 px-2.5 py-2">
          <div className="flex items-center gap-1.5 text-[9px] font-bold uppercase tracking-wide text-warn">
            gate <span className="font-normal normal-case text-ink-mute">#{block.gateId}</span>
          </div>
          <div className="text-note text-ink">{block.prompt}</div>
          {block.decision && (
            <div className="text-meta text-ink-dim">
              {block.decidedBy ? `${block.decision} by ${block.decidedBy}` : block.decision}
            </div>
          )}
        </div>
      );
    case 'seed':
      return (
        <div className="text-meta text-ink-mute">
          {block.messageCount} prior messages seed this run (stored once at the source)
          {block.sourceTranscript && onOpenTranscript && (
            <button
              type="button"
              onClick={() => onOpenTranscript(block.sourceTranscript as string)}
              className="ml-1.5 text-brand-500 underline-offset-2 hover:underline"
            >
              view source transcript
            </button>
          )}
        </div>
      );
    case 'notice':
      return (
        <div className="text-meta text-ink-mute">
          <span className="font-medium">{block.noticeKind}</span> · {block.message}
        </div>
      );
    case 'compaction':
      return (
        <div className="text-meta text-ink-mute">
          context compacted · seq {block.seq} · summarized {block.summarized} messages
        </div>
      );
    case 'unknown':
      return (
        <div className="text-meta italic text-ink-mute">
          unrecognized event: {block.type} (newer rupu wrote this transcript)
        </div>
      );
  }
}

function ThinkingBlock({ text, provider }: { text: string | null; provider: string }) {
  const [show, setShow] = useState(false);
  if (text === null || text === '') {
    return (
      <div className="text-meta italic text-ink-mute">
        [redacted reasoning{provider ? ` · ${provider}` : ''}]
      </div>
    );
  }
  return (
    <div>
      <button
        type="button"
        onClick={() => setShow((v) => !v)}
        className="flex items-center gap-1 text-meta font-medium text-ink-mute"
      >
        {show ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        thinking
      </button>
      {show && (
        <div className="mt-1 border-l-2 border-border pl-2 text-ink-mute opacity-80">
          <Markdown text={text} />
        </div>
      )}
    </div>
  );
}
