// @vitest-environment jsdom
/** Tests for SkillCard — the `skills.load` view, using the real nmap payload. */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { parseSkill, SkillSummary, SkillBody } from './SkillCard';
import type { ToolView } from './transcriptView';

afterEach(cleanup);

const OUTPUT = [
  'Skill: nmap (version 02d19b6f3731)',
  'Base directory: /Users/matt/.rupu/skills/nmap',
  'Files: references/nse.md, references/rupu.md, references/scan-types.md, scripts/nmap_to_workunits.py, scripts/parse_nmap_xml.py',
  'Compatibility: Linux/macOS with nmap >= 7.80; Python 3.8+ for scripts',
  '',
  '# Nmap — Network Scanning Methodology',
  '',
  'Nmap is the central scanner of the recon flow. Always write `-oA`.',
].join('\n');

function tv(overrides: Partial<ToolView>): ToolView {
  return { tool: 'skills.load', kind: 'skill', input: { name: 'nmap' }, ...overrides };
}

describe('parseSkill', () => {
  const v = parseSkill(tv({ output: OUTPUT }));

  it('extracts name + version', () => {
    expect(v.name).toBe('nmap');
    expect(v.version).toBe('02d19b6f3731');
  });

  it('extracts base dir, files, compatibility', () => {
    expect(v.baseDir).toBe('/Users/matt/.rupu/skills/nmap');
    expect(v.files).toHaveLength(5);
    expect(v.files[0]).toBe('references/nse.md');
    expect(v.compatibility).toContain('nmap >= 7.80');
  });

  it('pulls the heading as the title and keeps the body', () => {
    expect(v.title).toBe('Nmap — Network Scanning Methodology');
    expect(v.body).toContain('central scanner');
  });

  it('falls back to input.name when output is empty', () => {
    const e = parseSkill(tv({ output: '' }));
    expect(e.name).toBe('nmap');
    expect(e.files).toHaveLength(0);
  });
});

describe('render', () => {
  it('summary shows name, version, file count', () => {
    render(<SkillSummary view={parseSkill(tv({ output: OUTPUT }))} />);
    expect(screen.getByText('nmap')).toBeTruthy();
    expect(screen.getByText('v 02d19b6f')).toBeTruthy();
    expect(screen.getByText('5 files')).toBeTruthy();
  });

  it('body renders file chips + markdown body', () => {
    render(<SkillBody view={parseSkill(tv({ output: OUTPUT }))} />);
    expect(screen.getByText('references/nse.md')).toBeTruthy();
    expect(screen.getByText('+1 more')).toBeTruthy();
    expect(screen.getByText(/central scanner/)).toBeTruthy();
  });
});
