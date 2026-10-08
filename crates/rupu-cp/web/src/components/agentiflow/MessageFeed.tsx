// MessageFeed — the engagement's message inbox, rendered as a compact group
// chat (Discord / IRC). The fleet talks to itself on a shared board: each post
// has a sender (`author`, a roster role like `recon#1` / `lead`), an optional
// recipient (`addressed_to`) — empty means a BROADCAST to the channel, a value
// is an @mention — a `kind` (question / answer / observation / note), a
// timestamp and a body. The lead's standing `directives` are a pinned channel
// and can be RETRACTED (a `{retract:<id>}` record), folded in above.
//
// It reads like a chat app: a participants bar, one compact row per message
// (avatar · sender · crew signature · @mention · kind · time, body tight
// underneath), @mention-style highlighting of every participant the fleet names
// in a body (learned from the thread — codename, crew and role tokens, tinted by
// role), day dividers, and a directed message highlighted the way an @mention
// lands in a channel.
//
// Reads `GET /api/agentiflows/:id/messages`; polls while live. Operator → lead
// steering (`rupu agentiflow send`) sits at the top while the run is live.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { RefreshCw, Megaphone, Send, Pin, Hash } from 'lucide-react';
import { api, apiErrorMessage, type AgentiflowBoardPost } from '../../lib/api';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { useThemeMode } from '../codename/useThemeMode';
import { relativeTime, absoluteTime } from '../../lib/time';
import { cn } from '../../lib/cn';

const POLL_MS = 5000;

type Mode = 'light' | 'dark';

/** Per-kind accent (semantic, not role). `note` stays neutral. */
const KIND_HEX: Record<string, string> = {
  question: '#0ea5e9',
  answer: '#16a34a',
  observation: '#8b5cf6',
  warning: '#ef4444',
  directive: '#f59e0b',
};

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}
function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// --- role identity ---------------------------------------------------------

/** The role an author plays, stripped of its `#index` (`recon#1` → `recon`). */
function roleOf(name: string): string {
  return name.replace(/#\d+$/, '').trim().toLowerCase();
}

/** A stable tint per role (lead = brand violet; known specialists get fixed
 *  hues; anything else hashes into the same palette). Mode-aware. */
function roleTint(role: string, mode: Mode): string {
  const map: Record<string, [string, string]> = {
    lead: ['#7c3aed', '#a78bfa'],
    recon: ['#2563eb', '#60a5fa'],
    'service-analyst': ['#d97706', '#fbbf24'],
    'exploit-verifier': ['#db2777', '#f472b6'],
    crawler: ['#0d9488', '#2dd4bf'],
    'appsec-tester': ['#ea580c', '#fb923c'],
    operator: ['#4b5563', '#9ca3af'],
  };
  const hit = map[role];
  if (hit) return mode === 'dark' ? hit[1] : hit[0];
  const pal = mode === 'dark'
    ? ['#a78bfa', '#60a5fa', '#fbbf24', '#f472b6', '#2dd4bf', '#fb923c']
    : ['#7c3aed', '#2563eb', '#d97706', '#db2777', '#0d9488', '#ea580c'];
  let h = 0;
  for (let i = 0; i < role.length; i++) h = (h * 31 + role.charCodeAt(i)) >>> 0;
  return pal[h % pal.length];
}

function roleInitials(role: string): string {
  const parts = role.split(/[-_ ]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return (parts[0]?.[0] ?? '?').toUpperCase();
}

function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(full, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

function Avatar({ role, mode, size = 24 }: { role: string; mode: Mode; size?: number }) {
  const tint = roleTint(role, mode);
  return (
    <div
      className="flex shrink-0 items-center justify-center rounded-full font-semibold"
      style={{ width: size, height: size, fontSize: size < 22 ? 9 : 10, background: hexA(tint, 0.18), color: tint, boxShadow: `inset 0 0 0 1px ${hexA(tint, 0.45)}` }}
      title={role}
      aria-hidden
    >
      {roleInitials(role)}
    </div>
  );
}

function KindTag({ kind }: { kind: string }) {
  const hex = KIND_HEX[kind];
  if (!hex) return <span className="rounded px-1.5 py-px text-[10px] font-medium leading-none bg-surface text-ink-mute">{kind}</span>;
  return (
    <span className="rounded px-1.5 py-px text-[10px] font-medium leading-none" style={{ background: hexA(hex, 0.16), color: hex }}>
      {kind}
    </span>
  );
}

// --- mention index (learned from the thread) -------------------------------

type MentionIndex = Map<string, string>; // lowercased token → role

/** Pull the identity tokens out of a body's leading self-tag. */
function selfTagTokens(body: string): string[] {
  const out: string[] = [];
  const push = (s: string) => out.push(...s.split(/[/,]/).map((t) => t.trim()).filter(Boolean));
  let m = body.match(/^\s*\[([^\]]{1,80})\]/);
  if (m) push(m[1]);
  m = body.match(/^\s*\(([^)]{1,80})\)/);
  if (m) push(m[1]);
  m = body.match(/^\s*([A-Za-z][\w-]*)\s*\(([^)]{1,80})\)/);
  if (m) {
    out.push(m[1]);
    push(m[2]);
  }
  return out;
}

/** The first self-tag an author signs with (`elk / fawn-butte` → `elk · fawn-butte`). */
function signatureOf(body: string): string | undefined {
  let m = body.match(/^\s*\[([^\]]{1,80})\]/) || body.match(/^\s*\(([^)]{1,80})\)/);
  if (m) return m[1].split(/[/,]/).map((t) => t.trim()).filter(Boolean).join(' · ');
  m = body.match(/^\s*([A-Za-z][\w-]*)\s*\(([^)]{1,80})\)/);
  if (m) return [m[1], ...m[2].split(/[/,]/)].map((t) => t.trim()).filter(Boolean).join(' · ');
  return undefined;
}

function buildMentionIndex(posts: AgentiflowBoardPost[]): MentionIndex {
  const idx: MentionIndex = new Map();
  const add = (tok: string | undefined, role: string) => {
    const t = (tok ?? '').trim().toLowerCase();
    if (t.length >= 2 && !idx.has(t)) idx.set(t, role);
  };
  for (const p of posts) {
    const author = str(p.author);
    if (author) {
      const role = roleOf(author);
      add(author, role);
      add(role, role);
      for (const tok of selfTagTokens(str(p.body) ?? '')) add(tok, role);
    }
    const to = str(p.addressed_to);
    if (to) {
      add(to, roleOf(to));
      add(roleOf(to), roleOf(to));
    }
  }
  return idx;
}

/** Highlight every known participant token (and any `@token`) in a body. */
function renderBody(body: string, idx: MentionIndex, mode: Mode): ReactNode[] {
  if (idx.size === 0) return [body];
  const keys = [...idx.keys()].sort((a, b) => b.length - a.length).map(escapeRe);
  const re = new RegExp(`(^|[^\\w#@])(@?)(${keys.join('|')})(?![\\w-])`, 'gi');
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(body))) {
    const pre = m[1];
    const at = m[2];
    const tok = m[3];
    const start = m.index + pre.length;
    if (start > last) out.push(body.slice(last, start));
    const role = idx.get(tok.toLowerCase()) ?? roleOf(tok);
    const tint = roleTint(role, mode);
    out.push(
      <span key={i++} className="rounded px-0.5 font-medium" style={{ background: hexA(tint, 0.16), color: tint }}>
        {at}
        {tok}
      </span>,
    );
    last = m.index + m[0].length;
    if (re.lastIndex === m.index) re.lastIndex++;
  }
  if (last < body.length) out.push(body.slice(last));
  return out;
}

// --- participants bar ------------------------------------------------------

interface Participant {
  name: string;
  role: string;
  sig?: string;
}

function ParticipantsBar({ people, mode }: { people: Participant[]; mode: Mode }) {
  if (people.length === 0) return null;
  return (
    <div className="mb-3 flex flex-wrap items-center gap-1.5 rounded-xl border border-border bg-panel px-3 py-2 shadow-card">
      <span className="mr-0.5 inline-flex items-center gap-1 text-meta font-medium text-ink-mute">
        <Hash size={12} />
        channel
      </span>
      {people.map((p) => {
        const tint = roleTint(p.role, mode);
        return (
          <span key={p.name} className="inline-flex items-center gap-1.5 rounded-full py-0.5 pl-0.5 pr-2" style={{ background: hexA(tint, 0.1) }} title={p.sig ? `${p.name} · ${p.sig}` : p.name}>
            <Avatar role={p.role} mode={mode} size={18} />
            <span className="text-meta font-medium" style={{ color: tint }}>
              {p.name}
            </span>
          </span>
        );
      })}
    </div>
  );
}

// --- directives (pinned, retractable) --------------------------------------

interface Directive {
  id?: string;
  author: string;
  body: string;
  ts?: string;
  retracted: boolean;
}

function foldDirectives(dirs: AgentiflowBoardPost[]): Directive[] {
  const retracted = new Set<string>();
  for (const d of dirs) {
    const r = str((d as Record<string, unknown>).retract);
    if (r) retracted.add(r);
  }
  const out: Directive[] = [];
  for (const d of dirs) {
    const body = str(d.body);
    if (!body) continue;
    const id = str((d as Record<string, unknown>).id);
    out.push({ id, author: str(d.author) ?? 'lead', body, ts: str(d.ts), retracted: !!id && retracted.has(id) });
  }
  return out.sort((a, b) => Number(a.retracted) - Number(b.retracted) || (a.ts ?? '').localeCompare(b.ts ?? ''));
}

function DirectivesPanel({ dirs, mode, idx }: { dirs: Directive[]; mode: Mode; idx: MentionIndex }) {
  if (dirs.length === 0) return null;
  const active = dirs.filter((d) => !d.retracted).length;
  return (
    <div className="mb-3">
      <div className="mb-1.5 flex items-center gap-1.5 text-note font-medium text-ink-dim">
        <Pin size={13} className="text-amber-500" />
        Pinned · lead directives
        <span className="text-meta tabular-nums text-ink-mute">· {active} standing</span>
      </div>
      <div className="flex flex-col gap-2">
        {dirs.map((d, i) => (
          <div key={d.id ?? i} className={cn('rounded-xl border px-3 py-2', d.retracted ? 'border-border bg-transparent opacity-60' : 'border-amber-500/40 bg-amber-500/5')}>
            <div className="mb-1 flex items-center gap-2">
              <Avatar role={roleOf(d.author)} mode={mode} size={20} />
              <span className="text-note font-medium text-ink">{d.author}</span>
              {d.retracted ? <Badge tone="neutral">retracted</Badge> : <Badge tone="amber">standing directive</Badge>}
              {d.ts && (
                <span className="ml-auto text-meta text-ink-mute" title={absoluteTime(d.ts)}>
                  {relativeTime(d.ts)}
                </span>
              )}
            </div>
            <p className={cn('whitespace-pre-wrap break-words text-sm leading-relaxed text-ink-dim', d.retracted && 'line-through')}>{d.retracted ? d.body : renderBody(d.body, idx, mode)}</p>
          </div>
        ))}
      </div>
    </div>
  );
}

// --- conversation (compact chat) -------------------------------------------

/** One compact message row: avatar · sender · signature · @mention · kind ·
 *  time, with the body tight underneath. A directed message carries a left
 *  accent + tint, the way an @mention lands in a channel. */
function Message({ m, sig, idx, mode }: { m: AgentiflowBoardPost; sig?: string; idx: MentionIndex; mode: Mode }) {
  const [expanded, setExpanded] = useState(false);
  const author = str(m.author) ?? 'unknown';
  const role = roleOf(author);
  const tint = roleTint(role, mode);
  const to = str(m.addressed_to);
  const kind = str(m.kind);
  const ts = str(m.ts);
  const body = str(m.body) ?? '';
  const long = body.length > 360;
  const rTint = to ? roleTint(roleOf(to), mode) : null;

  return (
    <div
      className={cn('group/msg relative flex gap-2.5 rounded-md px-2 py-1.5 transition-colors', to ? '' : 'hover:bg-surface/40')}
      style={to ? { borderLeft: `3px solid ${rTint!}`, background: hexA(rTint!, 0.06) } : undefined}
    >
      <div className="pt-0.5">
        <Avatar role={role} mode={mode} size={24} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
          <span className="text-note font-semibold" style={{ color: tint }}>
            {author}
          </span>
          {sig && (
            <span className="text-meta font-medium" style={{ color: hexA(tint, 0.85) }}>
              {sig}
            </span>
          )}
          {to && (
            <span className="inline-flex items-center gap-0.5 text-meta" style={{ color: rTint! }}>
              →
              <span className="rounded px-1 font-medium" style={{ background: hexA(rTint!, 0.16), color: rTint! }}>
                @{to}
              </span>
            </span>
          )}
          {kind && <KindTag kind={kind} />}
          {ts && (
            <span className="ml-auto text-meta tabular-nums text-ink-mute" title={absoluteTime(ts)}>
              {relativeTime(ts)}
            </span>
          )}
        </div>
        <p className={cn('mt-0.5 whitespace-pre-wrap break-words text-sm leading-relaxed text-ink-dim', long && !expanded && 'line-clamp-4')}>{renderBody(body, idx, mode)}</p>
        {long && (
          <button type="button" onClick={() => setExpanded((v) => !v)} className="mt-0.5 text-meta font-medium text-brand-600 hover:text-brand-700">
            {expanded ? 'show less' : 'show more'}
          </button>
        )}
      </div>
    </div>
  );
}

type FeedItem = { type: 'divider'; key: string; label: string } | { type: 'msg'; post: AgentiflowBoardPost };

function buildFeed(posts: AgentiflowBoardPost[]): FeedItem[] {
  const items: FeedItem[] = [];
  let prevDay: string | null = null;
  for (const p of posts) {
    const ts = str(p.ts);
    const day = ts ? new Date(ts).toDateString() : '';
    if (day !== prevDay) {
      items.push({ type: 'divider', key: `d-${day}-${items.length}`, label: ts ? new Date(ts).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' }) : 'earlier' });
      prevDay = day;
    }
    items.push({ type: 'msg', post: p });
  }
  return items;
}

function DayDivider({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-3 py-1.5">
      <div className="h-px flex-1 bg-border" />
      <span className="text-meta font-medium text-ink-mute">{label}</span>
      <div className="h-px flex-1 bg-border" />
    </div>
  );
}

/** Operator → lead steering: the `rupu agentiflow send` channel, in the UI. */
function SteeringBox({ id, onSent }: { id: string; onSent: () => void }) {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function send(now: boolean) {
    const message = draft.trim();
    if (!message) return;
    setSending(true);
    setErr(null);
    setNote(null);
    try {
      await api.steerAgentiflow(id, { message, now });
      setDraft('');
      setNote(now ? 'Sent — the lead picks it up mid-round.' : 'Queued — the lead reads it at the next round boundary.');
      onSent();
    } catch (e) {
      setErr(apiErrorMessage(e));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="mb-3 rounded-xl border border-border bg-panel p-3 shadow-card">
      <div className="mb-1.5 flex items-center gap-1.5 text-note font-medium text-ink-dim">
        <Megaphone size={12} className="text-brand-500" />
        Steer the lead
      </div>
      <div className="flex items-center gap-2">
        <input
          className="min-w-0 flex-1 rounded-lg border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute"
          placeholder="Tell the lead what to focus on, re-scope, or prioritize…"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send(false);
            }
          }}
          disabled={sending}
        />
        <Button variant="secondary" onClick={() => void send(true)} disabled={sending || !draft.trim()} title="Deliver mid-round (interrupt)">
          Now
        </Button>
        <Button onClick={() => void send(false)} disabled={sending || !draft.trim()} className="gap-1.5">
          <Send size={12} />
          Send
        </Button>
      </div>
      {note && <div className="mt-1.5 text-meta text-ok">{note}</div>}
      {err && <div className="mt-1.5 text-meta text-err">{err}</div>}
    </div>
  );
}

export default function MessageFeed({ id, live }: { id: string; live?: boolean }) {
  const mode = useThemeMode();
  const [posts, setPosts] = useState<AgentiflowBoardPost[] | null>(null);
  const [dirs, setDirs] = useState<Directive[]>([]);
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
          const sorted = [...r.posts].sort((a, b) => (str(a.ts) ?? '').localeCompare(str(b.ts) ?? ''));
          setPosts(sorted);
          setDirs(foldDirectives(r.directives));
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

  const idx = useMemo(() => (posts ? buildMentionIndex([...posts, ...dirs.map((d) => ({ author: d.author, body: d.body }))]) : new Map<string, string>()), [posts, dirs]);
  const sigs = useMemo(() => {
    const m = new Map<string, string>();
    for (const p of posts ?? []) {
      const a = str(p.author);
      if (a && !m.has(a)) {
        const s = signatureOf(str(p.body) ?? '');
        if (s) m.set(a, s);
      }
    }
    return m;
  }, [posts]);
  const people = useMemo<Participant[]>(() => {
    const seen = new Set<string>();
    const out: Participant[] = [];
    for (const p of posts ?? []) {
      const a = str(p.author);
      if (a && !seen.has(a)) {
        seen.add(a);
        out.push({ name: a, role: roleOf(a), sig: sigs.get(a) });
      }
    }
    return out;
  }, [posts, sigs]);

  const empty = (!posts || posts.length === 0) && dirs.length === 0;

  let body: ReactNode;
  if (posts === null && loading) {
    body = (
      <div className="py-10 flex items-center justify-center">
        <Spinner label="Loading messages…" />
      </div>
    );
  } else if (error) {
    body = <ErrorBanner>{error}</ErrorBanner>;
  } else if (empty) {
    body = (
      <EmptyState
        title="No messages yet"
        hint="The fleet posts here as it coordinates — the lead's directives and the agents' questions, answers and observations to each other. A single-agent round may never post."
      />
    );
  } else {
    const feed = posts ? buildFeed(posts) : [];
    body = (
      <div>
        <ParticipantsBar people={people} mode={mode} />
        <DirectivesPanel dirs={dirs} mode={mode} idx={idx} />
        <div className="mb-1 flex items-center justify-between">
          <span className="text-note tabular-nums text-ink-dim">
            {posts?.length ?? 0} {(posts?.length ?? 0) === 1 ? 'message' : 'messages'}
          </span>
          <Button variant="secondary" onClick={() => load(false)} className="gap-1.5">
            <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
            Refresh
          </Button>
        </div>
        {posts && posts.length > 0 ? (
          <div className="rounded-xl border border-border bg-panel px-2 py-1 shadow-card">
            {feed.map((it) => (it.type === 'divider' ? <DayDivider key={it.key} label={it.label} /> : <Message key={`${str(it.post.ts) ?? ''}-${str(it.post.author) ?? ''}-${it.post.body?.slice(0, 16)}`} m={it.post} sig={sigs.get(str(it.post.author) ?? '')} idx={idx} mode={mode} />))}
          </div>
        ) : (
          <p className="text-meta text-ink-mute">No conversation posts yet — only the lead's directives above.</p>
        )}
      </div>
    );
  }

  return (
    <div>
      {live && <SteeringBox id={id} onSent={() => load(true)} />}
      {body}
    </div>
  );
}
