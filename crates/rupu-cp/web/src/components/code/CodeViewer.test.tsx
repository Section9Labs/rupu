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

  it('shows a placeholder when the file is unavailable', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue({ available: false, reason: 'file too large to display' });
    view(<CodeViewer wsId="ws1" path="x" findings={[]} />);
    await waitFor(() => expect(screen.getByText(/too large/)).toBeInTheDocument());
  });
});
