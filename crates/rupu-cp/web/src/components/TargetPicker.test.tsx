import { describe, it, expect } from 'vitest';
import { rankAndGroup } from './TargetPicker';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';

const items: TargetItem[] = [
  WORKSPACE_ITEM,
  { kind: 'project', label: 'rupu', sublabel: '/Code/rupu', resolved: { working_dir: '/Code/rupu' } },
  { kind: 'project', label: 'okesu', sublabel: '/Code/Okesu', resolved: { working_dir: '/Code/Okesu' } },
  { kind: 'repo', label: 'github:acme/api', sublabel: 'main', resolved: { target: 'github:acme/api' } },
];

describe('rankAndGroup', () => {
  it('empty query keeps all, workspace group first', () => {
    const groups = rankAndGroup(items, '');
    expect(groups[0].kind).toBe('workspace');
    expect(groups.map((g) => g.kind)).toEqual(['workspace', 'project', 'repo']);
  });
  it('filters by fuzzy query across label/sublabel', () => {
    const groups = rankAndGroup(items, 'rupu');
    const flat = groups.flatMap((g) => g.items.map((x) => x.item.label));
    expect(flat).toContain('rupu');
    expect(flat).not.toContain('okesu');
  });
});
