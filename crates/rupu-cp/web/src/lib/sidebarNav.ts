import type { LucideIcon } from 'lucide-react';
import {
  Activity,
  BookMarked,
  DollarSign,
  FolderGit2,
  LayoutDashboard,
  MessageSquare,
  Network,
  Radio,
  Repeat,
  Server,
  Settings,
  ShieldAlert,
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
export type GroupID = 'runs' | 'security' | 'build' | 'fleet';

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
  { kind: 'leaf', item: { to: '/events', label: 'Live Events', icon: Radio, enabled: true } },
  { kind: 'leaf', item: { to: '/usage', label: 'Usage', icon: DollarSign, enabled: true } },
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
    id: 'security', label: 'Security', items: [
      { to: '/coverage', label: 'Coverage', icon: ShieldCheck, enabled: true },
      { to: '/netflow',  label: 'Network',  icon: Network,     enabled: true },
      { to: '/findings', label: 'Findings', icon: ShieldAlert, enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'build', label: 'Build', items: [
      { to: '/agents',    label: 'Agents',    icon: Sparkles, enabled: true },
      { to: '/workflows', label: 'Workflows', icon: Workflow, enabled: true },
      { to: '/autoflows', label: 'Autoflows', icon: Repeat,   enabled: true },
    ],
  }},
  { kind: 'group', group: {
    id: 'fleet', label: 'Fleet', items: [
      { to: '/hosts',    label: 'Hosts',    icon: Server,        enabled: true },
      { to: '/workers',  label: 'Workers',  icon: Server,        enabled: true },
    ],
  }},
  { kind: 'divider' },
  { kind: 'leaf', item: { to: '/settings', label: 'Settings', icon: Settings, enabled: true } },
];

/* ── Shell v2 (flat rail, 7 leaves + pinned Settings) ─────────────────── */

export type NavLeafV2 = {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Which live count fills the right-aligned rail badge. Rendering is
   *  wired in Plan 3 (`/api/attention`); a leaf with no source shows no
   *  badge, and an unknown count renders nothing — never `0`. */
  badge?: 'attention' | 'running' | 'projects' | 'critical' | 'unhealthy';
};

export const sidebarNavV2: NavLeafV2[] = [
  { to: '/overview', label: 'Overview', icon: LayoutDashboard, badge: 'attention' },
  { to: '/activity', label: 'Activity', icon: Activity, badge: 'running' },
  { to: '/projects', label: 'Projects', icon: FolderGit2, badge: 'projects' },
  { to: '/security', label: 'Security', icon: ShieldCheck, badge: 'critical' },
  { to: '/library', label: 'Library', icon: BookMarked },
  { to: '/fleet', label: 'Fleet', icon: Server, badge: 'unhealthy' },
  { to: '/usage', label: 'Usage', icon: DollarSign },
];

export const settingsLeafV2: NavLeafV2 = { to: '/settings', label: 'Settings', icon: Settings };
