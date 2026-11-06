// The Messages tab (spec F1c §3.1): a space's whole log, every kind, with
// the composer under it. Source-agnostic: the page holds the space
// (`useMessageSpace`, so its tab can count unread messages) and says how to
// open a participant's transcript.

import { useMemo, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import type { MessageRoute, SpaceMessage } from '../../lib/api';
import { defaultRecipient, recipientGroups } from '../../lib/messages';
import type { MessageSpace } from '../../lib/useMessageSpace';
import { cn } from '../../lib/cn';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { Composer } from './Composer';
import { FeedView } from './FeedView';
import { withinSpan } from '../../lib/run/turnWindow';

export interface MessageFeedProps {
  route: MessageRoute;
  space: MessageSpace;
  /** Why the operator can't send now (the root isn't running), as the page
   *  knows it; the server's own refusal (409 / 501) also disables it. */
  notLiveReason?: string | null;
  /** Steps are addressable (a workflow run). */
  workflow?: boolean;
  openInTranscript?: (m: SpaceMessage) => (() => void) | null;
  highlightId?: string | null;
  /** Only messages sent inside this span (ms since the epoch): a window the
   *  run page selected on its Token flow. */
  within?: [number, number] | null;
}

export default function MessageFeed({ route, space, notLiveReason, workflow = false, openInTranscript, highlightId, within }: MessageFeedProps) {
  // Local, not `?q=`: the run detail page's Findings tab owns that param.
  const [query, setQuery] = useState('');
  const groups = useMemo(() => recipientGroups(route, space.participants, workflow), [route, space.participants, workflow]);
  const messages = useMemo(
    () => (within ? space.log.messages.filter((m) => withinSpan(within, m.ts)) : space.log.messages),
    [space.log.messages, within],
  );

  if (space.status === 'loading' || (space.status === 'ready' && space.space === 'present' && !space.loaded && space.log.messages.length === 0)) {
    return (
      <div className="flex items-center justify-center py-10">
        <Spinner label="Loading messages…" />
      </div>
    );
  }
  if (space.status === 'unavailable') {
    return <EmptyState title="Messages unavailable" hint={space.reason ?? "This host can't serve this run's messages."} />;
  }
  if (space.status === 'error') return <ErrorBanner>{space.reason}</ErrorBanner>;
  if (space.space === 'absent') {
    return <EmptyState title="No message space" hint="Nothing has been said in this run, and nothing in it can be reached." />;
  }

  return (
    <FeedView
      participants={space.participants}
      messages={messages}
      legacy={space.legacy}
      query={query}
      onQuery={setQuery}
      openInTranscript={openInTranscript}
      highlightId={highlightId}
      toolbar={
        space.refreshable ? (
          <Button variant="secondary" onClick={space.refresh} className="shrink-0 gap-1.5" title="Read the log again">
            <RefreshCw size={12} className={cn(!space.loaded && 'animate-spin')} />
            Refresh
          </Button>
        ) : (
          <span className="inline-flex shrink-0 items-center gap-1 text-meta text-ink-mute" title={space.streaming ? 'Following the log' : 'Reconnecting'}>
            <span className={cn('h-1.5 w-1.5 rounded-full', space.streaming ? 'animate-pulse bg-status-running' : 'bg-ink-mute')} />
            {space.streaming ? 'live' : 'offline'}
          </span>
        )
      }
      footer={
        <Composer
          key={route}
          groups={groups}
          defaultTo={defaultRecipient(route)}
          disabledReason={space.blocked ?? notLiveReason ?? null}
          pending={space.pending}
          error={space.sendError}
          onSend={space.send}
        />
      }
    />
  );
}
