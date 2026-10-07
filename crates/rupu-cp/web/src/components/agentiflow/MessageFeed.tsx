// MessageFeed — the engagement's board: the fire-and-forget messages the fleet
// posts to each other (notes, observations, the lead's directives), oldest
// first. This is the agentiflow "message inbox" — what the agents are saying to
// one another. Reads `GET /api/agentiflows/:id/messages`; polls while live.

import { useCallback, useEffect, useRef, useState } from 'react';
import { RefreshCw, Megaphone } from 'lucide-react';
import { api, apiErrorMessage, type AgentiflowBoardPost } from '../../lib/api';
import { Badge, type BadgeTone } from '../ui/Badge';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { relativeTime, absoluteTime } from '../../lib/time';
import { cn } from '../../lib/cn';

const POLL_MS = 5000;

const KIND_TONE: Record<string, BadgeTone> = {
  note: 'neutral',
  observation: 'violet',
  directive: 'amber',
  warning: 'red',
  question: 'green',
  answer: 'green',
};

interface Msg extends AgentiflowBoardPost {
  channel: 'post' | 'directive';
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}

function MsgRow({ m }: { m: Msg }) {
  const author = str(m.author) ?? 'unknown';
  const to = str(m.addressed_to);
  const kind = str(m.kind) ?? m.channel;
  const ts = str(m.ts);
  const body = str(m.body) ?? '';
  return (
    <div className="flex gap-3 border-b border-border px-1 py-3 last:border-b-0">
      <div className="w-44 shrink-0">
        <div className="truncate font-mono text-note font-medium text-ink" title={author}>
          {author}
        </div>
        <div className="truncate text-meta text-ink-mute">→ {to ?? 'all'}</div>
        {ts && (
          <div className="text-meta text-ink-mute" title={absoluteTime(ts)}>
            {relativeTime(ts)}
          </div>
        )}
      </div>
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex items-center gap-1.5">
          {m.channel === 'directive' && <Megaphone size={12} className="text-warn" aria-label="directive" />}
          <Badge tone={KIND_TONE[kind] ?? 'neutral'}>{kind}</Badge>
        </div>
        <pre className="whitespace-pre-wrap break-words font-sans text-sm text-ink-dim">{body}</pre>
      </div>
    </div>
  );
}

export default function MessageFeed({ id, live }: { id: string; live?: boolean }) {
  const [msgs, setMsgs] = useState<Msg[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);

  const load = useCallback(
    (silent: boolean) => {
      const mine = ++seq.current;
      const ctl = new AbortController();
      if (!silent) setLoading(true);
      api
        .getAgentiflowMessages(id, { signal: ctl.signal })
        .then((r) => {
          if (seq.current !== mine) return;
          const merged: Msg[] = [
            ...r.posts.map((p) => ({ ...p, channel: 'post' as const })),
            ...r.directives.map((p) => ({ ...p, channel: 'directive' as const })),
          ];
          // Oldest first, like a chat log.
          merged.sort((a, b) => (str(a.ts) ?? '').localeCompare(str(b.ts) ?? ''));
          setMsgs(merged);
          setError(null);
        })
        .catch((e: unknown) => {
          if (seq.current !== mine || ctl.signal.aborted) return;
          setError(apiErrorMessage(e));
        })
        .finally(() => {
          if (seq.current === mine) setLoading(false);
        });
      return () => ctl.abort();
    },
    [id],
  );

  useEffect(() => {
    const abort = load(false);
    return () => {
      seq.current++;
      abort();
    };
  }, [load]);

  useEffect(() => {
    if (!live) return;
    const t = setInterval(() => load(true), POLL_MS);
    return () => clearInterval(t);
  }, [live, load]);

  if (msgs === null && loading) {
    return (
      <div className="py-10 flex items-center justify-center">
        <Spinner label="Loading messages…" />
      </div>
    );
  }
  if (error) return <ErrorBanner>{error}</ErrorBanner>;
  if (!msgs || msgs.length === 0) {
    return (
      <EmptyState
        title="No messages yet"
        hint="The fleet posts here as it coordinates — the lead's directives and the agents' observations to each other. A single-agent round may never post."
      />
    );
  }

  return (
    <div>
      <div className="mb-2 flex items-center justify-between">
        <span className="text-note tabular-nums text-ink-dim">{msgs.length} messages</span>
        <Button variant="secondary" onClick={() => load(false)} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </div>
      <div className="rounded-xl border border-border bg-panel px-3 shadow-card">
        {msgs.map((m, i) => (
          <MsgRow key={`${m.channel}-${i}`} m={m} />
        ))}
      </div>
    </div>
  );
}
