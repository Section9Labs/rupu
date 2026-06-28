import { useId, useMemo, useRef, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { NavLink } from 'react-router-dom';
import type { GroupID, NavGroup } from '../lib/sidebarNav';
import { cn } from '../lib/cn';

// ── localStorage helpers ────────────────────────────────────────────
//
// One global key holds open/closed for every group. The reader returns
// `Partial` so callers fold in the smart default for any group without
// a stored value: `stored[groupID] ?? smartDefault`.

const KEY = 'rupu.sidebar.groups';

export type GroupOpenState = Record<GroupID, boolean>;

export function readGroupState(): Partial<GroupOpenState> {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === 'object' && parsed !== null ? parsed : {};
  } catch (e) {
    console.warn('sidebar: malformed group state, falling back to defaults', e);
    return {};
  }
}

export function writeGroupState(s: Partial<GroupOpenState>): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // private mode / quota — fail silently; in-memory state still works
  }
}

// ── prefers-reduced-motion ──────────────────────────────────────────

function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

// ── SidebarGroup ────────────────────────────────────────────────────

interface Props {
  group: NavGroup;
  // Smart-default seed — only used on first mount when localStorage
  // has no entry for this group.
  initiallyOpen: boolean;
  // True when one of this group's items is the current route. Drives
  // the brand-color header treatment as a "you are here" breadcrumb.
  containsActive: boolean;
}

export default function SidebarGroup({ group, initiallyOpen, containsActive }: Props) {
  const [open, setOpen] = useState<boolean>(() => {
    const stored = readGroupState();
    return stored[group.id] ?? initiallyOpen;
  });

  const reduceMotion = useMemo(prefersReducedMotion, []);

  const headerId = useId();
  const panelId = useId();

  function toggle() {
    const next = !open;
    const merged: Partial<GroupOpenState> = {
      ...readGroupState(),
      [group.id]: next,
    };
    writeGroupState(merged);
    setOpen(next);
  }

  const panelRef = useRef<HTMLDivElement>(null);

  const headerTone = containsActive
    ? 'text-brand-700 hover:text-brand-700'
    : 'text-ink-mute hover:text-ink-dim';

  return (
    <div className="select-none">
      <button
        id={headerId}
        type="button"
        onClick={toggle}
        aria-expanded={open}
        aria-controls={panelId}
        className={cn(
          'w-full flex items-center justify-between px-4 pt-3 pb-1',
          'text-meta uppercase tracking-wide font-semibold',
          'transition-colors',
          headerTone,
        )}
      >
        <span>{group.label}</span>
        <ChevronDown
          size={10}
          className={cn(
            'transition-transform duration-150',
            !open && '-rotate-90',
            reduceMotion && '!transition-none',
          )}
        />
      </button>

      <div
        id={panelId}
        role="region"
        aria-labelledby={headerId}
        ref={panelRef}
        style={
          reduceMotion
            ? { display: open ? 'block' : 'none' }
            : {
                maxHeight: open ? `${panelRef.current?.scrollHeight ?? 999}px` : 0,
                overflow: 'hidden',
                transition: 'max-height 150ms ease-out',
              }
        }
      >
        {group.items.map((item) => (
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
                    : 'text-ink hover:bg-surface-hover'
                  : 'text-ink-mute cursor-not-allowed',
              )
            }
            onClick={(e) => { if (!item.enabled) e.preventDefault(); }}
          >
            <item.icon size={16} strokeWidth={2} />
            <span>{item.label}</span>
            {!item.enabled && (
              <span className="ml-auto text-meta uppercase tracking-wide text-ink-mute">soon</span>
            )}
          </NavLink>
        ))}
      </div>
    </div>
  );
}
