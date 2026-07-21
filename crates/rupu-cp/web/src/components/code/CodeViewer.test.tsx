// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { ThemeProvider } from '../theme/ThemeProvider';
import CodeViewer from './CodeViewer';
import { api } from '../../lib/api';
import type { FindingRecord, FileContent } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const FILE: FileContent = {
  available: true,
  path: 'src/billing.rs',
  language: 'rust',
  totalLines: 3,
  lines: [
    { n: 1, text: 'fn read(org_id: u64) {' },
    { n: 2, text: '  let bill = db.get(org_id);' },
    { n: 3, text: '}' },
  ],
};

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [2, 2],
  summary: 'Missing tenant check',
  severity: 'high',
  evidence: { code_excerpt: 'let bill = db.get(org_id);', rationale: 'no userId check', references: [] },
} as unknown as FindingRecord;

function view(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('CodeViewer', () => {
  it('renders the file and an inline finding marker at its line', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(FILE);
    view(<CodeViewer wsId="ws1" path="src/billing.rs" findings={[FINDING]} />);
    await waitFor(() => expect(screen.getByText('Missing tenant check')).toBeInTheDocument());
    // the anchored line carries a finding marker (data-finding-line=2)
    expect(document.querySelector('[data-finding-line="2"]')).not.toBeNull();
  });

  it('stacks multiple findings on the same line', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(FILE);
    const second = { ...FINDING, id: 'f2', summary: 'Also logs PII' } as FindingRecord;
    view(<CodeViewer wsId="ws1" path="src/billing.rs" findings={[FINDING, second]} />);
    await waitFor(() => expect(screen.getByText('Missing tenant check')).toBeInTheDocument());
    expect(screen.getByText('Also logs PII')).toBeInTheDocument();
  });

  it('renders findings the navigator counts but that cannot anchor to a line (no line_range or out-of-range)', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(FILE); // 3 lines
    const fileScoped = {
      id: 'f-file',
      file_path: 'src/billing.rs',
      line_range: null,
      summary: 'Repo-wide secret handling',
      severity: 'medium',
      evidence: { rationale: 'no line', references: [] },
    } as unknown as FindingRecord;
    const outOfRange = {
      id: 'f-oor',
      file_path: 'src/billing.rs',
      line_range: [999, 1000], // past EOF (file shrank since finding)
      summary: 'Anchor past end of file',
      severity: 'low',
      evidence: { rationale: 'drifted', references: [] },
    } as unknown as FindingRecord;
    view(
      <CodeViewer wsId="ws1" path="src/billing.rs" findings={[FINDING, fileScoped, outOfRange]} />,
    );
    // The anchored one still renders inline…
    await waitFor(() => expect(screen.getByText('Missing tenant check')).toBeInTheDocument());
    // …and the two that can't anchor still render (in the file-level block), so
    // the viewer never shows fewer findings than the tree badge counts.
    expect(screen.getByText('Repo-wide secret handling')).toBeInTheDocument();
    expect(screen.getByText('Anchor past end of file')).toBeInTheDocument();
    expect(screen.getByText(/File-level findings/)).toBeInTheDocument();
  });

  it('shows a placeholder when the file is unavailable', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue({ available: false, reason: 'file too large to display' });
    view(<CodeViewer wsId="ws1" path="x" findings={[]} />);
    await waitFor(() => expect(screen.getByText(/too large/)).toBeInTheDocument());
  });

  it('renders exactly one line-row per source line, with no phantom rows, for a multi-line Go-like file', async () => {
    const GO_FILE: FileContent = {
      available: true,
      path: 'main.go',
      language: 'go',
      totalLines: 6,
      lines: [
        { n: 1, text: 'package main' },
        { n: 2, text: '' },
        { n: 3, text: 'import "fmt"' },
        { n: 4, text: '' },
        { n: 5, text: 'func main() {' },
        { n: 6, text: '}' },
      ],
    };
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(GO_FILE);
    view(<CodeViewer wsId="ws1" path="main.go" findings={[]} />);
    await waitFor(() => expect(document.querySelectorAll('[data-line-row]')).toHaveLength(6));

    const rows = Array.from(document.querySelectorAll('[data-line-row]'));
    expect(rows.map((r) => r.getAttribute('data-line-row'))).toEqual(['1', '2', '3', '4', '5', '6']);
    // Every row is a direct, single grid row — no nested block wrapper per
    // line inflating the row count or adding stray empty rows between them.
    for (const row of rows) {
      expect(row.tagName).toBe('DIV');
      expect(row.children).toHaveLength(2); // gutter cell + code cell only
    }
  });
});
