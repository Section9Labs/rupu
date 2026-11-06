// One run's, session's or agentiflow's message space, as a page holds it
// (spec F1c): a cheap probe (is there a space, who is in it), the whole log
// when the page needs it (the Messages tab was opened, or the run is live
// and the tab's unread count must follow it), the operator's sends with
// their pending state, and an unread count.
//
// A local live root is followed through the log's SSE stream; anything else
// (a finished root, a remote host, a pre-log agentiflow) is read in pages of
// 200 (a remote host's in pages of REMOTE_PAGE_SIZE, so a typical log is one
// call: an SSH host runs a command per page) and re-read on `refresh`
// (remote hosts are never polled). A caller that must not hold a stream
// open (the agentiflow map's workflow frames) passes `live: false` and a
// `tick` instead: each new tick reads what was logged since the last read.

import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, apiErrorMessage, type MessageRoute, type MessageStreamFrame, type SpaceParticipant } from './api';
import { EMPTY_LOG, withFrames, withMessages, type PendingSend, type SpaceLog } from './messages';

export const PAGE_SIZE = 200;
/** A remote host's page: each page is a round trip to the host (over SSH a
 *  command), so its whole log is read in as few as the server allows. */
export const REMOTE_PAGE_SIZE = 5000;
/** How often a local live root's participants are re-read. */
export const PARTICIPANTS_POLL_MS = 10_000;
const FLUSH_MS = 60;
/** A live stream has caught up with the log once it has been quiet this long
 *  after opening (the stream replays the whole log first). */
export const CAUGHT_UP_QUIET_MS = 400;

export interface MessageSource {
  route: MessageRoute;
  id: string;
  /** A remote host id; absent or `local` for this host. */
  host?: string;
}

export interface MessageSpace {
  status: 'loading' | 'ready' | 'unavailable' | 'error';
  /** Why the space can't be shown (`unavailable` / `error`). */
  reason: string | null;
  space: 'present' | 'absent' | null;
  legacy: boolean;
  participants: SpaceParticipant[];
  log: SpaceLog;
  /** The log has been read: after the last page of a paged read, or on a
   *  live stream's first batch, which may be only part of its replay (a live
   *  stream may still add to it; `caughtUp` says when the replay is over). */
  loaded: boolean;
  /** Everything the log held when it was opened has arrived: after the last
   *  page of a paged read, or once a live stream's replay has gone quiet. A
   *  message arriving after this is new. (`loaded` turns true on a stream's
   *  first batch, which can be part of its replay.) */
  caughtUp: boolean;
  /** The live stream is connected. */
  streaming: boolean;
  /** Re-read on demand (no live stream). */
  refreshable: boolean;
  refresh: () => void;
  pending: PendingSend[];
  sendError: string | null;
  /** Why the server refused a send outright (409 not live, 501): the
   *  composer stays off with this reason. */
  blocked: string | null;
  send: (msg: { to: string; body: string; urgent: boolean }) => Promise<boolean>;
  /** Messages logged since the page opened, or since the feed was last read. */
  unread: number;
}

function isLocal(host?: string): boolean {
  return !host || host === 'local';
}

export function useMessageSpace(
  source: MessageSource,
  opts: {
    /** The root is running: follow it live. */
    live: boolean;
    /** Read the whole log (the tab is, or was, open; or `live`). */
    load: boolean;
    /** The feed is on screen: nothing in it is unread. */
    reading: boolean;
    /** Without a stream: each new value reads what was logged since the
     *  last read (the caller's poll, e.g. its detail tick). */
    tick?: unknown;
  },
): MessageSpace {
  const { route, id, host } = source;
  const { live, load, reading, tick } = opts;
  const local = isLocal(host);
  const remoteHost = local ? undefined : host;
  const pageSize = local ? PAGE_SIZE : REMOTE_PAGE_SIZE;
  /** The id of the last message a paged read returned (`after` for a tick). */
  const cursor = useRef<string | undefined>(undefined);

  const [status, setStatus] = useState<MessageSpace['status']>('loading');
  const [reason, setReason] = useState<string | null>(null);
  const [space, setSpace] = useState<MessageSpace['space']>(null);
  const [legacy, setLegacy] = useState(false);
  const [participants, setParticipants] = useState<SpaceParticipant[]>([]);
  const [log, setLog] = useState<SpaceLog>(EMPTY_LOG);
  const [loaded, setLoaded] = useState(false);
  const [caughtUp, setCaughtUp] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [generation, setGeneration] = useState(0);
  /** Bumped by a tick that finds no space yet: the probe asks again. */
  const [probeGen, setProbeGen] = useState(0);
  const [pending, setPending] = useState<PendingSend[]>([]);
  const [sendError, setSendError] = useState<string | null>(null);
  const [blocked, setBlocked] = useState<string | null>(null);
  const [readAt, setReadAt] = useState(() => Date.now());

  useEffect(() => {
    setStatus('loading');
    setReason(null);
    setSpace(null);
    setLegacy(false);
    setParticipants([]);
    setLog(EMPTY_LOG);
    setLoaded(false);
    setCaughtUp(false);
    setPending([]);
    setSendError(null);
    setBlocked(null);
    setReadAt(Date.now());
    cursor.current = undefined;
  }, [route, id, host]);

  // The probe: whether a space exists, who is in it. Re-read while a local
  // root is live (participants join and leave), and on refresh.
  useEffect(() => {
    const ctl = new AbortController();
    const probe = () =>
      api
        .getMessages(route, id, { host: remoteHost, limit: 1, signal: ctl.signal })
        .then((r) => {
          if (ctl.signal.aborted) return;
          if (r.kind === 'unavailable') {
            setStatus('unavailable');
            setReason(r.reason);
            return;
          }
          setParticipants(r.view.participants);
          setLegacy(r.view.legacy);
          setSpace(r.view.space);
          setStatus('ready');
          setReason(null);
        })
        .catch((e: unknown) => {
          if (ctl.signal.aborted) return;
          setStatus((s) => (s === 'ready' ? s : 'error'));
          setReason(apiErrorMessage(e));
        });
    void probe();
    const timer = live && local ? setInterval(() => void probe(), PARTICIPANTS_POLL_MS) : null;
    return () => {
      ctl.abort();
      if (timer) clearInterval(timer);
    };
  }, [route, id, remoteHost, local, live, generation, probeGen]);

  const follow = load && space === 'present' && live && local && !legacy;

  // The live tail: frames are batched so a long log's replay costs one
  // render per batch, not per line.
  useEffect(() => {
    if (!follow) return;
    const buffer: MessageStreamFrame[] = [];
    let timer: ReturnType<typeof setTimeout> | null = null;
    let quiet: ReturnType<typeof setTimeout> | null = null;
    const flush = () => {
      timer = null;
      const frames = buffer.splice(0);
      setLog((l) => withFrames(l, frames));
      setLoaded(true);
    };
    // Each frame (and the open) restarts the quiet timer; when it runs out,
    // the replay is over.
    const settle = () => {
      if (quiet) clearTimeout(quiet);
      quiet = setTimeout(() => setCaughtUp(true), CAUGHT_UP_QUIET_MS);
    };
    const close = api.subscribeMessages(route, id, {
      onFrame: (f) => {
        buffer.push(f);
        if (!timer) timer = setTimeout(flush, FLUSH_MS);
        settle();
      },
      onOpen: () => {
        setStreaming(true);
        // An empty log sends no frame: the open is what says it was read.
        if (!timer) timer = setTimeout(flush, FLUSH_MS);
        settle();
      },
      onError: () => setStreaming(false),
    });
    return () => {
      close();
      if (timer) clearTimeout(timer);
      if (quiet) clearTimeout(quiet);
      setStreaming(false);
    };
  }, [follow, route, id]);

  // Everything else reads the log in pages, oldest first.
  useEffect(() => {
    if (!load || space !== 'present' || follow) return;
    const ctl = new AbortController();
    (async () => {
      let after: string | undefined;
      for (;;) {
        const r = await api.getMessages(route, id, { host: remoteHost, after, limit: pageSize, signal: ctl.signal });
        if (ctl.signal.aborted) return;
        if (r.kind === 'unavailable') {
          setStatus('unavailable');
          setReason(r.reason);
          return;
        }
        setParticipants(r.view.participants);
        setLog((l) => withMessages(l, r.view.messages));
        const last = r.view.messages[r.view.messages.length - 1];
        if (last) cursor.current = last.id;
        if (!r.view.next) break;
        after = r.view.next;
      }
      setLoaded(true);
      setCaughtUp(true);
    })().catch((e: unknown) => {
      if (ctl.signal.aborted) return;
      setStatus('error');
      setReason(apiErrorMessage(e));
    });
    return () => ctl.abort();
  }, [load, space, follow, route, id, remoteHost, pageSize, generation]);

  // A tick (no stream): what was logged since the last read.
  const loadedRef = useRef(loaded);
  loadedRef.current = loaded;
  const readable = load && space === 'present' && !follow;
  const readableRef = useRef(readable);
  readableRef.current = readable;
  const spaceRef = useRef(space);
  spaceRef.current = space;
  const tickSeen = useRef(tick);
  useEffect(() => {
    if (Object.is(tickSeen.current, tick)) return;
    tickSeen.current = tick;
    // No space yet (the run is still starting, or wasn't there when the
    // probe asked): ask again; once it is there, the paged read follows.
    if (spaceRef.current !== 'present' && !live) {
      setProbeGen((g) => g + 1);
      return;
    }
    // Before the first read is done, that read brings everything.
    if (!readableRef.current || !loadedRef.current) return;
    const ctl = new AbortController();
    (async () => {
      let after = cursor.current;
      for (;;) {
        const r = await api.getMessages(route, id, { host: remoteHost, after, limit: pageSize, signal: ctl.signal });
        if (ctl.signal.aborted || r.kind === 'unavailable') return;
        const next = r.view.participants;
        // The same roster keeps its array, so a quiet tick redraws nothing.
        setParticipants((p) => (JSON.stringify(p) === JSON.stringify(next) ? p : next));
        if (r.view.messages.length > 0) setLog((l) => withMessages(l, r.view.messages));
        const last = r.view.messages[r.view.messages.length - 1];
        if (last) cursor.current = last.id;
        if (!r.view.next) break;
        after = r.view.next;
      }
    })().catch(() => {
      // A failed tick keeps what was read; the next one tries again.
    });
    return () => ctl.abort();
  }, [tick, route, id, remoteHost, pageSize, live]);

  // A pending send is done when its line is in the log.
  useEffect(() => {
    setPending((p) => {
      const left = p.filter((x) => !x.id || !log.index.has(x.id));
      return left.length === p.length ? p : left;
    });
  }, [log]);

  useEffect(() => {
    if (reading) setReadAt(Date.now());
  }, [reading, log]);

  const refresh = useCallback(() => setGeneration((g) => g + 1), []);
  const followRef = useRef(follow);
  followRef.current = follow;

  const send = useCallback(
    async (msg: { to: string; body: string; urgent: boolean }) => {
      const key = `p-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      setSendError(null);
      setPending((p) => [...p, { key, ...msg }]);
      const drop = () => setPending((p) => p.filter((x) => x.key !== key));
      try {
        const r = await api.sendMessage(route, id, msg, { host: remoteHost });
        if (r.kind === 'unavailable') {
          drop();
          setBlocked(r.reason);
          return false;
        }
        setPending((p) => p.map((x) => (x.key === key ? { ...x, id: r.reply.id } : x)));
        // Without a live stream nothing would bring the line in.
        if (!followRef.current) refresh();
        return true;
      } catch (e) {
        drop();
        if (e instanceof ApiError && e.status === 409) setBlocked(apiErrorMessage(e));
        else setSendError(apiErrorMessage(e));
        return false;
      }
    },
    [route, id, remoteHost, refresh],
  );

  let unread = 0;
  if (!reading) {
    for (let i = log.messages.length - 1; i >= 0; i--) {
      const t = Date.parse(log.messages[i].ts);
      if (!(t > readAt)) break;
      unread++;
    }
  }

  return {
    status,
    reason,
    space,
    legacy,
    participants,
    log,
    loaded,
    caughtUp,
    streaming,
    refreshable: !follow,
    refresh,
    pending,
    sendError,
    blocked,
    send,
    unread,
  };
}
