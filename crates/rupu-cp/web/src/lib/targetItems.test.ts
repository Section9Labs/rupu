import { describe, it, expect } from 'vitest';
import {
  WORKSPACE_ITEM, projectItems, repoItems, dirItems, inferFreeTextItem, looksLikePath,
} from './targetItems';

describe('targetItems', () => {
  it('workspace resolves to neither field', () => {
    expect(WORKSPACE_ITEM.kind).toBe('workspace');
    expect(WORKSPACE_ITEM.resolved).toEqual({});
  });
  it('project → working_dir = path', () => {
    const [it0] = projectItems([{ ws_id: 'w', name: 'rupu', path: '/Code/rupu' } as any]);
    expect(it0).toMatchObject({ kind: 'project', label: 'rupu', sublabel: '/Code/rupu', resolved: { working_dir: '/Code/rupu' } });
  });
  it('repo → target = platform:repo', () => {
    const [it0] = repoItems([{ platform: 'github', repo: 'o/r', default_branch: 'main', private: false }]);
    expect(it0).toMatchObject({ kind: 'repo', label: 'github:o/r', resolved: { target: 'github:o/r' } });
  });
  it('dir → working_dir = path', () => {
    const [it0] = dirItems([{ name: 'crates', path: '/x/crates' }]);
    expect(it0).toMatchObject({ kind: 'directory', label: 'crates', sublabel: '/x/crates', resolved: { working_dir: '/x/crates' } });
  });
  it('free text: platform:owner/repo → repo target', () => {
    expect(inferFreeTextItem('github:acme/api')).toMatchObject({ kind: 'repo', resolved: { target: 'github:acme/api' } });
  });
  it('free text: paths → directory working_dir', () => {
    for (const p of ['/abs/x', '~/y', './rel', 'a/b']) {
      expect(inferFreeTextItem(p)).toMatchObject({ kind: 'directory', resolved: { working_dir: p } });
    }
    expect(looksLikePath('/abs')).toBe(true);
    expect(looksLikePath('plainword')).toBe(false);
  });
  it('free text: empty → null', () => {
    expect(inferFreeTextItem('  ')).toBeNull();
  });
});
