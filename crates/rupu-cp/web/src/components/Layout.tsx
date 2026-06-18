import { Link, NavLink, Outlet, useLocation } from 'react-router-dom';
import { cn } from '../lib/cn';
import CommandPalette from './CommandPalette';
import { sidebarNav, type NavLeaf, type NavGroup } from '../lib/sidebarNav';
import SidebarGroup from './SidebarGroup';

// Pure helpers — kept outside the component so React doesn't have to
// recreate them on every render.

// True if any leaf under this group matches the current pathname.
function groupContainsActive(group: NavGroup, pathname: string): boolean {
  return group.items.some((item) => leafIsActive(item, pathname));
}

function leafIsActive(leaf: NavLeaf, pathname: string): boolean {
  if (leaf.to === '/dashboard') {
    return pathname === leaf.to;
  }
  return pathname === leaf.to || pathname.startsWith(leaf.to + '/');
}

export default function Layout() {
  const { pathname } = useLocation();

  return (
    <div className="flex h-screen overflow-hidden">
      <aside className="w-60 shrink-0 border-r border-border bg-panel flex flex-col">
        {/* Logo / brand header */}
        <Link to="/" className="px-5 py-5 flex items-center gap-2 border-b border-border">
          <div className="w-7 h-7 rounded-md bg-brand-500 flex items-center justify-center">
            <div className="w-3 h-3 rounded-full bg-white" />
          </div>
          <div>
            <div className="font-semibold text-sm leading-tight">rupu</div>
            <div className="text-[11px] text-ink-mute leading-tight">Control Plane</div>
          </div>
        </Link>

        {/* Nav */}
        <nav className="flex-1 py-3 px-2 space-y-0.5 overflow-y-auto">
          {sidebarNav.map((section, idx) => {
            if (section.kind === 'divider') {
              return <div key={`d-${idx}`} className="border-t border-border my-2" />;
            }
            if (section.kind === 'leaf') {
              const item = section.item;
              return (
                <NavLink
                  key={item.to}
                  to={item.to}
                  end={item.to === '/dashboard'}
                  className={({ isActive }) =>
                    cn(
                      'flex items-center gap-2.5 px-3 py-2 rounded-md text-sm transition-colors',
                      item.enabled
                        ? isActive
                          ? 'bg-brand-50 text-brand-700 font-medium'
                          : 'text-ink hover:bg-slate-100'
                        : 'text-ink-mute cursor-not-allowed',
                    )
                  }
                  onClick={(e) => { if (!item.enabled) e.preventDefault(); }}
                >
                  <item.icon size={16} strokeWidth={2} />
                  <span>{item.label}</span>
                  {!item.enabled && (
                    <span className="ml-auto text-[10px] uppercase tracking-wide text-ink-mute">soon</span>
                  )}
                </NavLink>
              );
            }
            // section.kind === 'group'
            const group = section.group;
            const contains = groupContainsActive(group, pathname);
            return (
              <SidebarGroup
                key={group.id}
                group={group}
                initiallyOpen={contains}
                containsActive={contains}
              />
            );
          })}
        </nav>
      </aside>

      <main className="flex-1 overflow-auto">
        <Outlet />
      </main>

      {/* Cmd-K / Ctrl-K palette — global across all routes */}
      <CommandPalette />
    </div>
  );
}
