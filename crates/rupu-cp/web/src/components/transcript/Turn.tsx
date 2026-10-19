/**
 * Turn — one turn of the transcript reader's feed.
 *
 * A turn-head row (numbered spine marker, "Turn N", token label, a spine rule)
 * followed by its blocks in order, each in an `.ev` row with a gutter glyph
 * keyed by `blockEventKind`. Weights: assistant prose is loud, full thinking is
 * quiet and individually collapsible, tools/commands/findings are `ToolCard`
 * rows, messages are bubbles, everything else is a quiet one-line row. An
 * unrecognized event still renders as a labelled fallback so nothing vanishes.
 *
 * Honors the active filter from `useTranscriptView()`: non-matching blocks are
 * skipped, and a turn with no matching block renders nothing at all.
 *
 * No `any`. Static Tailwind class strings only.
 */

import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import {
  AlertTriangle,
  Brain,
  ChevronDown,
  ChevronRight,
  Info,
  MessageSquare,
  Terminal,
  User,
  Wrench,
} from 'lucide-react';
import Markdown from './Markdown';
import ToolCard from './ToolCard';
import MessageBubble, { InjectedChip } from './MessageBubble';
import type { TurnBlock, TurnView } from './transcriptView';
import { blockEventKind, blockMatchesFilter } from './eventKind';
import type { EventKind } from './eventKind';
import { useTranscriptView } from './TranscriptViewContext';
import { cn } from '../../lib/cn';

const GLYPH_TONE: Record<EventKind, string> = {
  said: 'text-brand-500',
  thinking: 'text-ink-mute',
  user: 'text-ink-dim',
  message: 'text-info',
  command: 'text-ink-dim',
  finding: 'text-warn',
  tool: 'text-ink-mute',
  quiet: 'text-ink-mute',
};

function Glyph({ kind, block }: { kind: EventKind; block: TurnBlock }) {
  const cls = 'shrink-0';
  switch (kind) {
    case 'said':
      return <MessageSquare size={13} className={cls} />;
    case 'thinking':
      return <Brain size={13} className={cls} />;
    case 'user':
      return <User size={13} className={cls} />;
    case 'message':
      return <MessageSquare size={13} className={cls} />;
    case 'command':
      return <Terminal size={13} className={cls} />;
    case 'finding':
      return <AlertTriangle size={13} className={cls} />;
    case 'tool':
      return <Wrench size={13} className={cls} />;
    case 'quiet':
      return block.kind === 'gate' || block.kind === 'outcome' ? (
        <AlertTriangle size={13} className={cls} />
      ) : (
        <Info size={13} className={cls} />
      );
  }
}

export default function Turn({
  turn,
  turnNumber,
  showMarker,
  tokensLabel,
  onOpenTranscript,
  runId,
  host,
  self,
  highlightMsg,
}: {
  turn: TurnView;
  turnNumber: number;
  showMarker: boolean;
  tokensLabel?: string;
  onOpenTranscript?: (path: string) => void;
  runId?: string;
  host?: string;
  /** The run's address in its message space (`TranscriptView.self`). */
  self?: string | null;
  /** The `#msg-` anchor: the bubble rings. */
  highlightMsg?: string | null;
}) {
  const { filter } = useTranscriptView();
  const blocks = turn.blocks.filter((b) => blockMatchesFilter(b, filter));
  if (blocks.length === 0) return null;

  return (
    <section className="relative" data-testid="turn">
      <div className="flex items-center gap-2 py-1.5">
        {showMarker && (
          <span
            aria-hidden
            className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-border bg-panel text-[10px] font-semibold text-ink-dim"
          >
            {turnNumber}
          </span>
        )}
        {showMarker && <span className="text-note font-medium text-ink-dim">Turn {turnNumber}</span>}
        {tokensLabel && <span className="text-meta text-ink-mute">{tokensLabel}</span>}
        <span className="h-px flex-1 bg-border" />
      </div>
      <div className={cn('flex flex-col gap-1.5', showMarker && 'border-l border-border pl-3')}>
        {blocks.map((block, i) => {
          const kind = blockEventKind(block);
          return (
            <div key={i} className="ev flex items-start gap-2">
              <span className={cn('mt-0.5 flex w-4 justify-center', GLYPH_TONE[kind])}>
                <Glyph kind={kind} block={block} />
              </span>
              <div className="min-w-0 flex-1">
                <TurnBlockView
                  block={block}
                  onOpenTranscript={onOpenTranscript}
                  runId={runId}
                  host={host}
                  self={self}
                  highlightMsg={highlightMsg}
                />
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function Quiet({ children }: { children: ReactNode }) {
  return <div className="text-meta text-ink-mute">{children}</div>;
}

function TurnBlockView({
  block,
  onOpenTranscript,
  runId,
  host,
  self,
  highlightMsg,
}: {
  block: TurnBlock;
  onOpenTranscript?: (path: string) => void;
  runId?: string;
  host?: string;
  self?: string | null;
  highlightMsg?: string | null;
}) {
  switch (block.kind) {
    case 'message':
      return (
        <MessageBubble
          message={block.message}
          self={self}
          highlight={highlightMsg === block.message.id}
          raw={
            block.message.raw ? (
              <ToolCard tool={block.message.raw} onOpenTranscript={onOpenTranscript} runId={runId} host={host} />
            ) : undefined
          }
        />
      );
    case 'injected':
      return (
        <InjectedChip source={block.source} kind={block.injectedKind} cadence={block.cadence} content={block.content} />
      );
    case 'assistant':
      return (
        <div className="text-ink">
          <Markdown text={block.content} />
        </div>
      );
    case 'thinking':
      return <ThinkingBlock text={block.text} provider={block.provider} />;
    case 'user':
      return (
        <div>
          <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-ink-mute">prompt</div>
          <Markdown text={block.content} />
        </div>
      );
    case 'tool':
      return <ToolCard tool={block.view} onOpenTranscript={onOpenTranscript} runId={runId} host={host} />;
    case 'gate':
      return (
        <div className="border-l-2 border-warn pl-2">
          <div className="text-meta text-warn">
            gate <span className="text-ink-mute">#{block.gateId}</span>
          </div>
          <div className="text-note text-ink-dim">{block.prompt}</div>
          {block.decision && (
            <div className="text-meta text-ink-mute">
              {block.decidedBy ? `${block.decision} by ${block.decidedBy}` : block.decision}
            </div>
          )}
        </div>
      );
    case 'seed':
      return (
        <Quiet>
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
        </Quiet>
      );
    case 'notice':
      return (
        <Quiet>
          <span className="font-medium">{block.noticeKind}</span> · {block.message}
        </Quiet>
      );
    case 'meta':
      return (
        <Quiet>
          <span className="font-medium">{block.label}</span> · {block.text}
        </Quiet>
      );
    case 'compaction':
      return (
        <Quiet>
          context compacted · seq {block.seq} · summarized {block.summarized} messages
        </Quiet>
      );
    case 'outcome': {
      const tone =
        block.severity === 'error' ? 'border-err' : block.severity === 'warning' ? 'border-warn' : 'border-info';
      return (
        <div className={`border-l-2 pl-2 ${tone}`}>
          <div className="text-note font-medium text-ink-dim">{block.title}</div>
          {block.detail && <div className="text-meta text-ink-mute">{block.detail}</div>}
        </div>
      );
    }
    case 'recovery':
      return <Quiet>{block.text}</Quiet>;
    case 'assistant_block':
      return (
        <details className="text-meta text-ink-mute">
          <summary className="cursor-pointer">{block.text}</summary>
          <pre className="mt-1 max-h-40 overflow-auto rounded bg-surface p-1.5 font-mono">
            {JSON.stringify(block.raw, null, 2)}
          </pre>
        </details>
      );
    case 'unknown':
      return (
        <div className="text-meta italic text-ink-mute">
          unrecognized event: {block.type} (newer rupu wrote this transcript)
          {block.data !== undefined && (
            <pre className="mt-1 max-h-40 overflow-auto rounded bg-surface p-1.5 font-mono not-italic">
              {JSON.stringify(block.data, null, 2)}
            </pre>
          )}
        </div>
      );
  }
}

/** Full thinking text, quiet; open by default, folds on collapse-all. */
function ThinkingBlock({ text, provider }: { text: string | null; provider: string }) {
  const { expandTarget, expandNonce } = useTranscriptView();
  const [show, setShow] = useState(expandTarget !== 'closed');
  useEffect(() => {
    if (expandTarget === 'closed') setShow(false);
    else if (expandTarget === 'open') setShow(true);
  }, [expandTarget, expandNonce]);

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
        aria-expanded={show}
        onClick={() => setShow((v) => !v)}
        className="flex items-center gap-1 text-meta font-medium text-ink-mute"
      >
        {show ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        thinking
      </button>
      {show && (
        <div className="mt-1 border-l-2 border-border pl-2 text-ink-dim">
          <Markdown text={text} />
        </div>
      )}
    </div>
  );
}
