// Cmd-K / Ctrl-K command palette. Mounted once at the Layout level so the
// hotkey is global across every route.
//
// Global entity search: on open we fetch runs, agents, workflows, autoflows,
// sessions, projects, coverage targets, findings, issues (autoflow claims) and
// workers in parallel, map each into a uniform `PaletteItem`, and fuzzy-rank
// them grouped by kind. Enter / click navigates into the selected result.
//
// Pure ranking + the API→item mappers live in `../lib/paletteSources` so they
// can be unit-tested without React/DOM. This file owns fetching, keyboard
// model (⌘K toggle, ↑↓, Enter, Esc) and rendering, keeping the modal overlay
// and keyboard-hints footer from the original nav-jump palette.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Activity,
  AlertTriangle,
  Bug,
  FileText,
  FolderGit2,
  LayoutDashboard,
  Loader2,
  MessageSquare,
  Radio,
  Repeat,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Sparkles,
  Workflow,
  X,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../lib/cn';
import { useHotkey } from '../lib/useHotkey';
import { api } from '../lib/api';
import { fuzzyScore } from '../lib/fuzzy';
import {
  rankPalette,
  runItems,
  agentItems,
  workflowItems,
  autoflowItems,
  sessionItems,
  projectItems,
  coverageItems,
  findingItems,
  issueItems,
  workerItems,
  type EntityKind,
  type PaletteItem,
} from '../lib/paletteSources';

// ---------------------------------------------------------------------------
// Static nav pages — always present as the first (page) group.
// ---------------------------------------------------------------------------

const NAV_PAGES: PaletteItem[] = [
  { kind: 'page', id: 'dashboard', title: 'Dashboard',    to: '/dashboard' },
  { kind: 'page', id: 'runs',      title: 'Runs',          to: '/runs' },
  { kind: 'page', id: 'events',    title: 'Live Events',   to: '/events' },
  { kind: 'page', id: 'coverage',  title: 'Coverage',      to: '/coverage' },
  { kind: 'page', id: 'workflows', title: 'Workflows',     to: '/workflows' },
  { kind: 'page', id: 'agents',    title: 'Agents',        to: '/agents' },
  { kind: 'page', id: 'autoflows', title: 'Autoflows',     to: '/autoflows' },
  { kind: 'page', id: 'sessions',  title: 'Sessions',      to: '/sessions' },
  { kind: 'page', id: 'projects',  title: 'Projects',      to: '/projects' },
  { kind: 'page', id: 'findings',  title: 'Findings',      to: '/findings' },
  { kind: 'page', id: 'workers',   title: 'Workers',       to: '/workers' },
  { kind: 'page', id: 'settings',  title: 'Settings',      to: '/settings' },
];

// Per-page icon, keyed by page id (falls back to a generic glyph).
const PAGE_ICON: Record<string, LucideIcon> = {
  dashboard: LayoutDashboard,
  runs:      Activity,
  events:    Radio,
  coverage:  ShieldCheck,
  workflows: Workflow,
  agents:    Sparkles,
  autoflows: Repeat,
  sessions:  MessageSquare,
  projects:  FolderGit2,
  findings:  AlertTriangle,
  workers:   Server,
  settings:  Settings,
};

// ---------------------------------------------------------------------------
// Per-kind presentation.
// ---------------------------------------------------------------------------

const KIND_LABEL: Record<EntityKind, string> = {
  page:     'Pages',
  run:      'Runs',
  agent:    'Agents',
  workflow: 'Workflows',
  autoflow: 'Autoflows',
  session:  'Sessions',
  project:  'Projects',
  coverage: 'Coverage',
  finding:  'Findings',
  issue:    'Issues',
  worker:   'Workers',
};

const KIND_ICON: Record<EntityKind, LucideIcon> = {
  page:     FileText,
  run:      Activity,
  agent:    Sparkles,
  workflow: Workflow,
  autoflow: Repeat,
  session:  MessageSquare,
  project:  FolderGit2,
  coverage: ShieldCheck,
  finding:  AlertTriangle,
  issue:    Bug,
  worker:   Server,
};

// Subtle per-kind icon tint (resting; active rows override to brand).
const KIND_COLOR: Record<EntityKind, string> = {
  page:     'text-ink-mute',
  run:      'text-sky-500',
  agent:    'text-violet-500',
  workflow: 'text-indigo-500',
  autoflow: 'text-teal-500',
  session:  'text-ok',
  project:  'text-warn',
  coverage: 'text-ok',
  finding:  'text-rose-500',
  issue:    'text-warn',
  worker:   'text-ink-dim',
};

function iconFor(item: PaletteItem): LucideIcon {
  if (item.kind === 'page') return PAGE_ICON[item.id] ?? FileText;
  return KIND_ICON[item.kind];
}

// ---------------------------------------------------------------------------
// Highlight helper — re-scores the rendered title so matched chars underline.
// ---------------------------------------------------------------------------

function highlight(text: string, query: string): React.ReactNode {
  const q = query.trim();
  if (!q) return text;
  const res = fuzzyScore(q, text);
  if (!res || res.matched.length === 0) return text;
  const set = new Set(res.matched);
  const parts: React.ReactNode[] = [];
  for (let i = 0; i < text.length; i++) {
    if (set.has(i)) {
      parts.push(
        <mark key={i} className="bg-transparent text-brand-700 font-semibold">
          {text[i]}
        </mark>,
      );
    } else {
      parts.push(text[i]);
    }
  }
  return <>{parts}</>;
}

function Hint({ k, label }: { k: string; label: string }) {
  return (
    <span className="inline-flex items-center gap-1">
      <kbd className="px-1.5 py-0.5 rounded border border-border bg-panel text-meta font-mono text-ink-dim">
        {k}
      </kbd>
      <span>{label}</span>
    </span>
  );
}

export default function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [items, setItems] = useState<PaletteItem[]>([]);
  const [loading, setLoading] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const navigate = useNavigate();

  // Cmd-K / Ctrl-K toggle. Swallow the event so the browser default (focus
  // URL bar, etc.) doesn't fire.
  useHotkey({ key: 'k', metaOrCtrl: true }, useCallback((e) => {
    e.preventDefault();
    setOpen((prev) => !prev);
  }, []));

  // Esc closes — only bound while open.
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.preventDefault();
        setOpen(false);
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  // On open: reset input, focus, and refetch all sources in parallel. Each
  // source tolerates failure (→ []) so one dead endpoint doesn't break search.
  // A cancelled flag guards against setState after unmount/close.
  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    setTimeout(() => inputRef.current?.focus(), 0);

    let cancelled = false;
    setLoading(true);
    Promise.all([
      api.getRuns({ limit: 200 }).then(runItems).catch(() => []),
      api.getAgents().then(agentItems).catch(() => []),
      api.getWorkflows().then(workflowItems).catch(() => []),
      api.getAutoflowDefs().then(autoflowItems).catch(() => []),
      api.getSessions({ limit: 200 }).then(sessionItems).catch(() => []),
      api.getProjects().then(projectItems).catch(() => []),
      api.getCoverage().then(coverageItems).catch(() => []),
      api.getFindings().then((r) => findingItems(r.findings)).catch(() => []),
      api.getAutoflowClaims().then(issueItems).catch(() => []),
      api.getWorkers().then(workerItems).catch(() => []),
    ])
      .then((buckets) => {
        if (cancelled) return;
        setItems(buckets.flat());
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open]);

  const groups = useMemo(
    () => rankPalette(query, items, NAV_PAGES),
    [query, items],
  );

  // Flat ordered list for keyboard nav — mirrors the visual group order.
  const flat = useMemo<PaletteItem[]>(
    () => groups.flatMap((g) => g.items),
    [groups],
  );

  // Clamp active when the result set shrinks under us.
  useEffect(() => {
    if (active >= flat.length) setActive(Math.max(0, flat.length - 1));
  }, [flat.length, active]);

  function go(to: string | null) {
    if (!to) return;
    setOpen(false);
    navigate(to);
  }

  function onInputKey(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((i) => Math.min(flat.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const sel = flat[active];
      if (sel) go(sel.to);
    }
  }

  if (!open) return null;

  // Map each flat item to its index so per-row highlight calc stays O(1).
  const indexByItem = new Map<PaletteItem, number>();
  flat.forEach((it, i) => indexByItem.set(it, i));

  return (
    <div
      className="fixed inset-0 bg-black/30 flex items-start justify-center pt-[12vh] p-4 z-50"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) setOpen(false);
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        className="bg-panel border border-border rounded-xl shadow-card w-full max-w-2xl flex flex-col overflow-hidden"
      >
        <header className="px-4 py-3 border-b border-border flex items-center gap-2">
          <Search size={16} className="text-ink-mute shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => { setQuery(e.target.value); setActive(0); }}
            onKeyDown={onInputKey}
            placeholder="Search runs, agents, workflows, sessions…"
            className="flex-1 bg-transparent outline-none text-sm placeholder:text-ink-mute"
            spellCheck={false}
            autoComplete="off"
            role="combobox"
            aria-expanded
            aria-autocomplete="list"
          />
          {loading && <Loader2 size={14} className="animate-spin text-ink-mute" />}
          <button
            onClick={() => setOpen(false)}
            className="p-1 text-ink-dim hover:text-ink rounded-md"
            title="Close (Esc)"
          >
            <X size={14} />
          </button>
        </header>

        <div className="max-h-[60vh] overflow-auto" role="listbox">
          {flat.length === 0 && (
            <div className="px-4 py-6 text-xs text-ink-mute flex items-center gap-2">
              {loading ? (
                <><Loader2 size={12} className="animate-spin" /> Searching…</>
              ) : (
                'No matches.'
              )}
            </div>
          )}

          {groups.map((group) => {
            const SectionIcon = KIND_ICON[group.kind];
            return (
              <div key={group.kind} className="py-1">
                <div className="px-4 py-1 text-meta uppercase tracking-wide text-ink-mute font-medium flex items-center gap-1.5">
                  <SectionIcon size={11} className={cn('shrink-0', KIND_COLOR[group.kind])} />
                  {KIND_LABEL[group.kind]}
                </div>
                {group.items.map((it) => {
                  const idx = indexByItem.get(it) ?? -1;
                  const isActive = idx === active;
                  const Icon = iconFor(it);
                  return (
                    <button
                      key={`${it.kind}-${it.id}`}
                      role="option"
                      aria-selected={isActive}
                      onClick={() => go(it.to)}
                      onMouseEnter={() => idx >= 0 && setActive(idx)}
                      className={cn(
                        'w-full flex items-center gap-3 px-4 py-2 text-left text-sm',
                        isActive ? 'bg-brand-50 text-brand-700' : 'text-ink hover:bg-surface-hover',
                      )}
                    >
                      <Icon
                        size={14}
                        className={cn('shrink-0', isActive ? 'text-brand-700' : KIND_COLOR[it.kind])}
                      />
                      <div className="min-w-0 flex-1">
                        <div className="truncate">{highlight(it.title, query)}</div>
                        {it.subtitle && (
                          <div className={cn('text-note truncate', isActive ? 'text-brand-700/70' : 'text-ink-mute')}>
                            {it.subtitle}
                          </div>
                        )}
                      </div>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>

        <footer className="px-4 py-2 border-t border-border flex items-center gap-3 text-note text-ink-mute">
          <Hint k="↑↓" label="Navigate" />
          <Hint k="↵" label="Open" />
          <Hint k="Esc" label="Close" />
        </footer>
      </div>
    </div>
  );
}
