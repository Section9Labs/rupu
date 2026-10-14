import { describe, expect, it } from 'vitest';
import type { SkillRow } from './api';
import { insertSlashSkill, matchSlashSkills, readSlashTokens, slashSkillsOf, slashTokenAt } from './slashSkills';

const row = (name: string, extra: Partial<SkillRow> = {}): SkillRow => ({
  name,
  layer: 'global',
  status: 'ok',
  description: null,
  error: null,
  warnings: [],
  stamp: null,
  source: null,
  both_layers: false,
  ...extra,
});

describe('slashSkills', () => {
  it('offers valid skills, layer-qualified when both layers install the name', () => {
    const skills = slashSkillsOf([
      row('pdf', { description: 'Read PDFs.' }),
      row('both', { both_layers: true }),
      row('both', { layer: 'project', both_layers: true }),
      row('broken', { status: 'invalid' }),
    ]);
    expect(skills).toEqual([
      { entry: 'global:both', description: null },
      { entry: 'pdf', description: 'Read PDFs.' },
      { entry: 'project:both', description: null },
    ]);
  });

  it('finds the leading / token the caret ends', () => {
    expect(slashTokenAt('/', 1)).toEqual({ start: 0, query: '' });
    expect(slashTokenAt('  /pd', 5)).toEqual({ start: 2, query: 'pd' });
    expect(slashTokenAt('/pdf /co', 8)).toEqual({ start: 5, query: 'co' });
    expect(slashTokenAt('/pdf rest', 9)).toBeNull();
    expect(slashTokenAt('look at /pd', 11)).toBeNull();
    expect(slashTokenAt('/pdf x', 2)).toBeNull();
  });

  it('ranks matches and writes the picked entry with one space after it', () => {
    const skills = [{ entry: 'code-review', description: null }, { entry: 'pdf', description: null }];
    expect(matchSlashSkills(skills, 'pd').map((s) => s.entry)).toEqual(['pdf']);
    expect(insertSlashSkill('/pd', 0, 3, 'pdf')).toEqual({ text: '/pdf ', caret: 5 });
    expect(insertSlashSkill('/pdf /c go', 5, 7, 'code-review')).toEqual({ text: '/pdf /code-review go', caret: 18 });
  });

  it('reads which leading tokens invoke a skill, as the server does', () => {
    const skills = [{ entry: 'pdf', description: null }, { entry: 'notes', description: null }];
    expect(readSlashTokens('/pdf /notes go', skills)).toEqual({ invoked: ['pdf', 'notes'], unknown: null });
    expect(readSlashTokens('/global:pdf go', skills)).toEqual({ invoked: ['global:pdf'], unknown: null });
    expect(readSlashTokens('/pdf /pdff go', skills)).toEqual({ invoked: ['pdf'], unknown: 'pdff' });
    expect(readSlashTokens('/etc/hosts is odd', skills)).toEqual({ invoked: [], unknown: null });
    expect(readSlashTokens('see /pdf', skills)).toEqual({ invoked: [], unknown: null });
  });
});
