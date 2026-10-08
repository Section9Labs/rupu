// AgentiflowEventFeed — the agentiflow Events tab. Fetches the engagement's
// granular event feed (`GET /api/agentiflows/:id/events`: coordinator lifecycle
// + per-unit lifecycle, oldest first), maps it to the shared Situation Room
// StreamCards (`agentiflowEventCards`), and renders it through the SAME
// RunEventFeed / EventCard the run detail and the global /events wall use. Polls
// while the engagement is running (no SSE yet — a later pass).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import RunEventFeed from '../RunEventFeed';
import { agentiflowEventCards } from '../../lib/agentiflowEvents';
import { api, apiErrorMessage, type AgentiflowEvent } from '../../lib/api';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';

const POLL_MS = 5000;

export default function AgentiflowEventFeed({ id, codename, running }: { id: string; codename?: string; running?: boolean }) {
  const [events, setEvents] = useState<AgentiflowEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const load = useCallback(() => {
    const mine = ++seq.current;
    const ctl = new AbortController();
    api.getAgentiflowEvents(id, { signal: ctl.signal }).then(
      (r) => {
        if (seq.current === mine) {
          setEvents(r.events);
          setError(null);
        }
      },
      (e: unknown) => {
        // A failed poll keeps the last good feed; only a first load surfaces.
        if (seq.current === mine && !ctl.signal.aborted) setError(apiErrorMessage(e));
      },
    );
    return () => ctl.abort();
  }, [id]);

  useEffect(() => {
    const abort = load();
    return () => {
      seq.current++;
      abort();
    };
  }, [load]);

  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => load(), POLL_MS);
    return () => clearInterval(t);
  }, [running, load]);

  const cards = useMemo(() => agentiflowEventCards(events ?? [], codename), [events, codename]);

  if (events === null && !error) {
    return (
      <div className="py-10 flex items-center justify-center">
        <Spinner label="Loading events…" />
      </div>
    );
  }
  if (error && events === null) return <ErrorBanner>{error}</ErrorBanner>;
  return <RunEventFeed cards={cards} connection={running ? 'live' : undefined} />;
}
