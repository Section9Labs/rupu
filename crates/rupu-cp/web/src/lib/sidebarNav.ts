import type { LucideIcon } from 'lucide-react';
import {
  Activity,
  LayoutDashboard,
  MessageSquare,
  Radio,
  Server,
  Settings,
  ShieldCheck,
  Sparkles,
  Workflow,
} from 'lucide-react';

// One nav-leaf renders as a single <NavLink> in the sidebar. `enabled: false`
// items render greyed out and are not clickable.
export type NavLeaf = {
  to: string;
  label: string;
  icon: LucideIcon;
  enabled: boolean;
};

// String-literal union — keeps group ids type-safe and distinct from
// any NavLeaf.to (the latter are paths starting with `/`).
export type GroupID = 'observe' | 'build' | 'run';

// One nav-group renders as a collapsible section header followed by
// its `items`.
export type NavGroup = {
  id: GroupID;
  label: string;
  items: NavLeaf[];
};

// Layout walks NavSection[] and emits the right element per kind:
// 'leaf' → <NavLink>, 'group' → <SidebarGroup>, 'divider' → <hr/>.
export type NavSection =
  | { kind: 'leaf'; item: NavLeaf }
  | { kind: 'group'; group: NavGroup }
  | { kind: 'divider' };

export const sidebarNav: NavSection[] = [
  { kind: 'leaf', item: { to: '/dashboard', label: 'Dashboard', icon: LayoutDashboard, enabled: true } },
  { kind: 'divider' },
  { kind: 'group', group: {
    id: 'observe', label: 'Observe', items: [
      { to: '/runs',     label: 'Runs',        icon: Activity,   enabled: true },
      { to: '/events',   label: 'Live Events', icon: Radio,      enabled: true },
      { to: '/coverage', label: 'Coverage',    icon: ShieldCheck, enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'build', label: 'Build', items: [
      { to: '/workflows', label: 'Workflows', icon: Workflow, enabled: true },
      { to: '/agents',    label: 'Agents',    icon: Sparkles, enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'run', label: 'Run', items: [
      { to: '/sessions', label: 'Sessions', icon: MessageSquare, enabled: true },
      { to: '/workers',  label: 'Workers',  icon: Server,        enabled: true },
    ],
  }},
  { kind: 'divider' },
  { kind: 'leaf', item: { to: '/settings', label: 'Settings', icon: Settings, enabled: true } },
];
