import { it, expect, describe } from 'vitest';
import { sidebarNav, type NavGroup, sidebarNavV2, settingsLeafV2 } from './sidebarNav';

function findGroup(id: string): NavGroup {
  const section = sidebarNav.find((s) => s.kind === 'group' && s.group.id === id);
  if (!section || section.kind !== 'group') throw new Error(`group not found: ${id}`);
  return section.group;
}

it('has a Security group with Coverage, Network and Findings', () => {
  const security = findGroup('security');
  expect(security.label).toBe('Security');
  expect(security.items.map((i) => i.to)).toEqual(['/coverage', '/netflow', '/findings']);
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

describe('sidebarNavV2', () => {
  it('has exactly the seven v2 destinations in order', () => {
    expect(sidebarNavV2.map((l) => l.to)).toEqual([
      '/overview', '/activity', '/projects', '/security', '/library', '/fleet', '/usage',
    ]);
  });
  it('pins settings separately', () => {
    expect(settingsLeafV2.to).toBe('/settings');
  });
  it('declares badge sources only where the spec assigns one', () => {
    const badges = Object.fromEntries(sidebarNavV2.map((l) => [l.to, l.badge]));
    expect(badges['/overview']).toBe('attention');
    expect(badges['/activity']).toBe('running');
    expect(badges['/projects']).toBe('projects');
    expect(badges['/security']).toBe('critical');
    expect(badges['/fleet']).toBe('unhealthy');
    expect(badges['/library']).toBeUndefined();
    expect(badges['/usage']).toBeUndefined();
  });
});
