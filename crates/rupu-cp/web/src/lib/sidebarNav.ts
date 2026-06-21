import type { LucideIcon } from 'lucide-react';
import {
  FolderGit2,
  LayoutDashboard,
  MessageSquare,
  Radio,
  Repeat,
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
export type GroupID = 'runs' | 'observe' | 'build' | 'fleet';

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
  { kind: 'leaf', item: { to: '/projects', label: 'Projects', icon: FolderGit2, enabled: true } },
  { kind: 'divider' },
  { kind: 'group', group: {
    id: 'runs', label: 'Runs', items: [
      { to: '/runs/agents',    label: 'Agents',    icon: Sparkles,      enabled: true },
      { to: '/runs/workflows', label: 'Workflows', icon: Workflow,      enabled: true },
      { to: '/runs/autoflows', label: 'Autoflows', icon: Repeat,        enabled: true },
      { to: '/sessions',       label: 'Sessions',  icon: MessageSquare, enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'observe', label: 'Observe', items: [
      { to: '/events',   label: 'Live Events', icon: Radio,       enabled: true },
      { to: '/coverage', label: 'Coverage',    icon: ShieldCheck, enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'build', label: 'Build', items: [
      { to: '/workflows', label: 'Workflows', icon: Workflow, enabled: true },
      { to: '/agents',    label: 'Agents',    icon: Sparkles, enabled: true },
      { to: '/autoflows', label: 'Autoflows', icon: Repeat,   enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'fleet', label: 'Fleet', items: [
      { to: '/workers',  label: 'Workers',  icon: Server,        enabled: true },
    ],
  }},
  { kind: 'divider' },
  { kind: 'leaf', item: { to: '/settings', label: 'Settings', icon: Settings, enabled: true } },
];
