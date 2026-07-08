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

it('has a top-level Live Events leaf right after Projects', () => {
  const projectsIndex = sidebarNav.findIndex(
    (s) => s.kind === 'leaf' && s.item.to === '/projects',
  );
  const nextSection = sidebarNav[projectsIndex + 1];
  expect(nextSection.kind).toBe('leaf');
  if (nextSection.kind === 'leaf') {
    expect(nextSection.item.to).toBe('/events');
    expect(nextSection.item.label).toBe('Live Events');
  }
});

it('no longer has an Observe group', () => {
  const observe = sidebarNav.find((s) => s.kind === 'group' && s.group.label === 'Observe');
  expect(observe).toBeUndefined();
});

it('routes are unchanged (Coverage/Findings paths still present exactly once each)', () => {
  const allLeaves = sidebarNav.flatMap((s) =>
    s.kind === 'leaf' ? [s.item] : s.kind === 'group' ? s.group.items : [],
  );
  const paths = allLeaves.map((l) => l.to);
  expect(paths.filter((p) => p === '/coverage')).toHaveLength(1);
  expect(paths.filter((p) => p === '/findings')).toHaveLength(1);
});
