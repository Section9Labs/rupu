import { it, expect } from 'vitest';
import { sidebarNav, type NavGroup } from './sidebarNav';

function findGroup(id: string): NavGroup {
  const section = sidebarNav.find((s) => s.kind === 'group' && s.group.id === id);
  if (!section || section.kind !== 'group') throw new Error(`group not found: ${id}`);
  return section.group;
}

it('has a Security group with Coverage and Findings', () => {
  const security = findGroup('security');
  expect(security.label).toBe('Security');
  expect(security.items.map((i) => i.to)).toEqual(['/coverage', '/findings']);
});

it('Observe no longer contains Coverage or Findings', () => {
  const observe = findGroup('observe');
  expect(observe.items.map((i) => i.to)).toEqual(['/events']);
});

it('routes are unchanged (Coverage/Findings paths still present exactly once each)', () => {
  const allLeaves = sidebarNav.flatMap((s) =>
    s.kind === 'leaf' ? [s.item] : s.kind === 'group' ? s.group.items : [],
  );
  const paths = allLeaves.map((l) => l.to);
  expect(paths.filter((p) => p === '/coverage')).toHaveLength(1);
  expect(paths.filter((p) => p === '/findings')).toHaveLength(1);
});
