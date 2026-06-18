// Cmd-K / Ctrl-K navigation palette. Mounted once at the Layout level so
// the hotkey is global across every route.
//
// This is a simplified nav-jump palette: fuzzy-filters the rupu page list,
// Enter navigates. Keeps the Okesu visual style (modal overlay, search
// header, keyboard hints footer) but removes all API data fetching.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Activity,
  LayoutDashboard,
  Loader2,
  MessageSquare,
  Radio,
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

interface NavPage {
  to: string;
  label: string;
  icon: LucideIcon;
}

const NAV_PAGES: NavPage[] = [
  { to: '/dashboard', label: 'Dashboard',   icon: LayoutDashboard },
  { to: '/runs',      label: 'Runs',         icon: Activity },
  { to: '/events',    label: 'Live Events',  icon: Radio },
  { to: '/coverage',  label: 'Coverage',     icon: ShieldCheck },
  { to: '/workflows', label: 'Workflows',    icon: Workflow },
  { to: '/agents',    label: 'Agents',       icon: Sparkles },
  { to: '/sessions',  label: 'Sessions',     icon: MessageSquare },
  { to: '/workers',   label: 'Workers',      icon: Server },
  { to: '/settings',  label: 'Settings',     icon: Settings },
];

interface ScoredPage {
  page: NavPage;
  matched: number[];
}

// Simple fuzzy substring / subsequence matcher — same approach as
// Okesu's palette, but scoped to page labels only.
function fuzzyMatch(query: string, label: string): { matched: number[] } | null {
  const q = query.toLowerCase();
  const t = label.toLowerCase();

  const direct = t.indexOf(q);
  if (direct >= 0) {
    const matched: number[] = [];
    for (let i = 0; i < q.length; i++) matched.push(direct + i);
    return { matched };
  }

  // Subsequence match
  const matched: number[] = [];
  let ti = 0;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi];
    let found = -1;
    while (ti < t.length) {
      if (t[ti] === ch) { found = ti; ti++; break; }
      ti++;
    }
    if (found < 0) return null;
    matched.push(found);
  }
  return { matched };
}

function highlight(text: string, matched: number[]): React.ReactNode {
  if (!matched.length) return text;
  const set = new Set(matched);
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
      <kbd className="px-1.5 py-0.5 rounded border border-border bg-white text-[10px] font-mono text-ink-dim">
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
  const inputRef = useRef<HTMLInputElement | null>(null);
  const navigate = useNavigate();

  // Cmd-K / Ctrl-K toggle.
  useHotkey({ key: 'k', metaOrCtrl: true }, useCallback((e) => {
    e.preventDefault();
    setOpen((prev) => !prev);
  }, []));

  // Esc closes.
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

  // On open, reset state and focus input.
  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    setTimeout(() => inputRef.current?.focus(), 0);
  }, [open]);

  const results = useMemo<ScoredPage[]>(() => {
    const q = query.trim();
    if (!q) {
      return NAV_PAGES.map((page) => ({ page, matched: [] }));
    }
    const out: ScoredPage[] = [];
    for (const page of NAV_PAGES) {
      const m = fuzzyMatch(q, page.label);
      if (m) out.push({ page, matched: m.matched });
    }
    return out;
  }, [query]);

  // Clamp active when result set shrinks.
  useEffect(() => {
    if (active >= results.length) setActive(Math.max(0, results.length - 1));
  }, [results.length, active]);

  function go(to: string) {
    setOpen(false);
    navigate(to);
  }

  function onInputKey(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((i) => Math.min(results.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const sel = results[active];
      if (sel) go(sel.page.to);
    }
  }

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 bg-black/30 flex items-start justify-center pt-[12vh] p-4 z-50"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) setOpen(false);
      }}
    >
      <div className="bg-panel border border-border rounded-xl shadow-card w-full max-w-2xl flex flex-col overflow-hidden">
        <header className="px-4 py-3 border-b border-border flex items-center gap-2">
          <Search size={16} className="text-ink-mute shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => { setQuery(e.target.value); setActive(0); }}
            onKeyDown={onInputKey}
            placeholder="Go to page…"
            className="flex-1 bg-transparent outline-none text-sm placeholder:text-ink-mute"
            spellCheck={false}
            autoComplete="off"
          />
          {/* Loader shown only as a placeholder for future async commands */}
          {false && <Loader2 size={14} className="animate-spin text-ink-mute" />}
          <button
            onClick={() => setOpen(false)}
            className="p-1 text-ink-dim hover:text-ink rounded-md"
            title="Close (Esc)"
          >
            <X size={14} />
          </button>
        </header>

        <div className="max-h-[60vh] overflow-auto">
          {results.length === 0 && (
            <div className="px-4 py-6 text-xs text-ink-mute">No pages match.</div>
          )}

          {results.length > 0 && (
            <div className="py-1">
              <div className="px-4 py-1 text-[10px] uppercase tracking-wide text-ink-mute font-medium">
                Pages
              </div>
              {results.map(({ page, matched }, idx) => {
                const Icon = page.icon;
                const isActive = idx === active;
                return (
                  <button
                    key={page.to}
                    onClick={() => go(page.to)}
                    onMouseEnter={() => setActive(idx)}
                    className={cn(
                      'w-full flex items-center gap-3 px-4 py-2 text-left text-sm',
                      isActive ? 'bg-brand-50 text-brand-700' : 'text-ink hover:bg-slate-50',
                    )}
                  >
                    <Icon
                      size={14}
                      className={cn('shrink-0', isActive ? 'text-brand-700' : 'text-ink-mute')}
                    />
                    <span>{highlight(page.label, matched)}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <footer className="px-4 py-2 border-t border-border flex items-center gap-3 text-[11px] text-ink-mute">
          <Hint k="↑↓" label="Navigate" />
          <Hint k="↵" label="Open" />
          <Hint k="Esc" label="Close" />
        </footer>
      </div>
    </div>
  );
}
