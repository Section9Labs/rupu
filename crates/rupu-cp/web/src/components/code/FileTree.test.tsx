// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import FileTree from './FileTree';
import { api } from '../../lib/api';
import type { FindingRecord, TreeResult } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ROOT: TreeResult = {
  path: '',
  parent: null,
  entries: [
    { name: 'src', path: 'src', kind: 'dir' },
    { name: 'README.md', path: 'README.md', kind: 'file' },
  ],
};
const SRC: TreeResult = {
  path: 'src',
  parent: '',
  entries: [{ name: 'billing.rs', path: 'src/billing.rs', kind: 'file' }],
};

const FINDINGS = [
  { id: 'f1', file_path: 'src/billing.rs', line_range: [2, 2], severity: 'high', summary: 's', evidence: { rationale: '', references: [] } },
] as unknown as FindingRecord[];

describe('FileTree', () => {
  it('renders root entries and a folder rollup badge', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileTree wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    // src rolls up billing.rs's high finding
    expect(screen.getByTestId('badge-src')).toHaveTextContent('1');
  });

  it('lazy-loads a folder on expand and selects a file', async () => {
    const spy = vi.spyOn(api, 'getProjectTree');
    spy.mockResolvedValueOnce(ROOT).mockResolvedValueOnce(SRC);
    const onSelect = vi.fn();
    render(<FileTree wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={onSelect} />);
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    fireEvent.click(screen.getByText('src'));
    await waitFor(() => expect(screen.getByText('billing.rs')).toBeInTheDocument());
    fireEvent.click(screen.getByText('billing.rs'));
    expect(onSelect).toHaveBeenCalledWith('src/billing.rs');
  });
});
