// components/v2/Shell.tsx — Shell v2 chrome (docs/redesign/README.md §Shell).
// Rail 204px + 48px top bar; scope/range are shell-level state read by every
// page; the live pill reflects the real SSE connection state and degrades
// honestly (connecting/reconnecting take the warn tone via .sr-live[data-state]).

import { useEffect, useState } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import { Search } from 'lucide-react';
import { api } from '../../lib/api';
import type { HostView } from '../../lib/api';
import { sidebarNavV2, settingsLeafV2, type NavLeafV2 } from '../../lib/sidebarNav';
import Brand from '../Brand';
import CommandPalette, { openCommandPalette } from '../CommandPalette';
import { ThemeToggle } from '../theme/ThemeToggle';
import type { ConnectionState } from '../RunEventFeed';
import { Segmented } from '../ui/Segmented';
import { ShellStateProvider, useShellState, type ShellRange } from './shellState';

const HOST_POLL_INTERVAL_MS = 60_000;

const RANGE_OPTIONS: { value: ShellRange; label: string }[] = [
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'all' },
];

function navLeafClass(isActive: boolean): string {
  return [
    'h-[30px] rounded-[5px] px-2 flex items-center gap-2 text-[13px]',
    isActive
      ? 'bg-surface text-ink shadow-[inset_2px_0_0_rgb(var(--c-brand-500))]'
      : 'text-ink-dim hover:bg-surface-hover',
  ].join(' ');
}

function NavRow({ leaf }: { leaf: NavLeafV2 }) {
  const Icon = leaf.icon;
  return (
    <NavLink to={leaf.to} className={({ isActive }) => navLeafClass(isActive)}>
      <Icon size={15} />
      <span>{leaf.label}</span>
    </NavLink>
  );
}

function HostFooter() {
  const [hosts, setHosts] = useState<HostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const rows = await api.getHosts();
        if (!cancelled) setHosts(rows);
      } catch {
        // leave the last-known state (or unloaded) — footer degrades to `—`
      }
    }
    load();
    const interval = setInterval(load, HOST_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  if (hosts === null) {
    return (
      <div className="border-t border-border px-3 py-[9px] flex items-center gap-2 font-mono text-[10px] text-ink-mute">
        <span className="h-[6px] w-[6px] rounded-full bg-status-failed" aria-hidden />
        <span>— hosts</span>
      </div>
    );
  }

  const down = hosts.filter((h) => h.status !== 'online').length;
  const allOnline = hosts.length > 0 && down === 0;

  return (
    <div className="border-t border-border px-3 py-[9px] flex items-center gap-2 font-mono text-[10px] text-ink-mute">
      <span
        className={`h-[6px] w-[6px] rounded-full ${allOnline ? 'bg-status-done' : 'bg-status-failed'}`}
        aria-hidden
      />
      <span>
        {hosts.length} hosts
        {down > 0 && ` · ${down} down`}
      </span>
    </div>
  );
}

function Rail() {
  return (
    <aside className="flex w-[204px] shrink-0 flex-col border-r border-border bg-panel">
      <div className="h-12 px-3 border-b border-border flex items-center gap-2">
        <Brand variant="rail" />
        <span className="ml-auto font-mono text-[9px] uppercase tracking-[0.1em] text-ink-mute">cp</span>
      </div>
      <nav className="flex-1 flex flex-col gap-0.5 px-2 py-2">
        {sidebarNavV2.map((leaf) => (
          <NavRow key={leaf.to} leaf={leaf} />
        ))}
        <div className="mt-auto">
          <NavRow leaf={settingsLeafV2} />
        </div>
      </nav>
      <HostFooter />
    </aside>
  );
}

function ScopeSelect() {
  const { scope, setScope } = useShellState();
  const [projects, setProjects] = useState<{ ws_id: string; name: string }[]>([]);

  useEffect(() => {
    let cancelled = false;
    api
      .getProjects()
      .then((rows) => {
        if (!cancelled) setProjects(rows.map((r) => ({ ws_id: r.ws_id, name: r.name })));
      })
      .catch(() => {
        // leave the list empty — the "all projects" option still works
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <select
      aria-label="Project scope"
      className="h-[26px] rounded-[5px] border border-border bg-surface px-2 font-mono text-[11px] text-ink"
      value={scope ?? ''}
      onChange={(e) => setScope(e.target.value === '' ? null : e.target.value)}
    >
      <option value="">all projects</option>
      {projects.map((p) => (
        <option key={p.ws_id} value={p.ws_id}>
          {p.name}
        </option>
      ))}
    </select>
  );
}

function RangeSegmented() {
  const { range, setRange } = useShellState();
  return (
    <Segmented
      options={RANGE_OPTIONS}
      value={range}
      onChange={(v) => setRange(v as ShellRange)}
      size="sm"
      ariaLabel="Range"
    />
  );
}

function LivePill() {
  const [state, setState] = useState<ConnectionState>('connecting');
  useEffect(() => {
    setState('connecting');
    const unsub = api.subscribeEvents(
      () => setState('live'),
      undefined,
      () => setState('reconnecting'),
    );
    return unsub;
  }, []);
  return (
    <span className="sr-live" data-state={state}>
      <span className="sr-beacon" aria-hidden />
      {state === 'live' ? 'live' : state}
    </span>
  );
}

function TopBar() {
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-panel px-4">
      <span className="font-mono text-[11px] text-ink-mute">scope</span>
      <ScopeSelect />
      <span aria-hidden className="font-mono text-[11px] text-ink-mute">/</span>
      <RangeSegmented />
      <button
        onClick={openCommandPalette}
        className="flex h-7 max-w-[420px] flex-1 items-center gap-2 rounded-[5px] border border-border bg-bg px-2.5 text-left"
      >
        <Search size={13} className="text-ink-mute" />
        <span className="flex-1 truncate text-ui text-ink-mute">Jump to run, project, agent, finding…</span>
        <span className="font-mono text-[9px] rounded border border-border px-1 text-ink-mute">⌘K</span>
      </button>
      <div className="ml-auto flex items-center gap-2">
        <LivePill />
        <ThemeToggle variant="icon" />
      </div>
    </header>
  );
}

export default function Shell() {
  return (
    <ShellStateProvider>
      <div className="flex h-screen overflow-hidden">
        <Rail />
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar />
          <main className="flex-1 overflow-auto">
            <Outlet />
          </main>
        </div>
        <CommandPalette shell="v2" />
      </div>
    </ShellStateProvider>
  );
}
