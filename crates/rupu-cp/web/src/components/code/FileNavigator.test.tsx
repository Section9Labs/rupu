// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import FileNavigator from './FileNavigator';
import { api } from '../../lib/api';
import type { FindingRecord, TreeResult, FileListResult } from '../../lib/api';

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

const FINDINGS = [
  {
    id: 'f1',
    file_path: 'src/billing.rs',
    line_range: [2, 2],
    severity: 'high',
    summary: 's1',
    evidence: { rationale: '', references: [] },
  },
  {
    id: 'f2',
    file_path: 'src/util.rs',
    line_range: [4, 4],
    severity: 'low',
    summary: 's2',
    evidence: { rationale: '', references: [] },
  },
  {
    id: 'f3',
    file_path: null,
    line_range: null,
    severity: 'info',
    summary: 's3',
    evidence: { rationale: '', references: [] },
  },
] as unknown as FindingRecord[];

const ALL_FILES: FileListResult = {
  files: ['README.md', 'src/billing.rs', 'src/util.rs', 'src/deep/nested/module.rs'],
  truncated: false,
};

describe('FileNavigator', () => {
  it('defaults to All mode and renders the lazy FileTree', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'All' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('toggling to Findings mode shows only finding-bearing files, flat', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Findings' }));

    expect(screen.getByTestId('nav-row-src/billing.rs')).toBeInTheDocument();
    expect(screen.getByTestId('nav-row-src/util.rs')).toBeInTheDocument();
    // Only the two distinct finding files, nothing else (e.g. README.md not present).
    expect(screen.queryByTestId('nav-row-README.md')).not.toBeInTheDocument();
  });

  it('shows the empty-findings message when there are no findings', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileNavigator wsId="ws1" findings={[]} selectedPath={null} onSelect={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Findings' }));
    expect(screen.getByText('No findings in this project.')).toBeInTheDocument();
  });

  it('typing in search filters the Findings-mode list', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Findings' }));
    fireEvent.change(screen.getByPlaceholderText('Filter files…'), { target: { value: 'billing' } });

    expect(screen.getByTestId('nav-row-src/billing.rs')).toBeInTheDocument();
    expect(screen.queryByTestId('nav-row-src/util.rs')).not.toBeInTheDocument();
  });

  it('in All mode with a search, fetches getProjectFiles once and filters the flat result', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    const filesSpy = vi.spyOn(api, 'getProjectFiles').mockResolvedValue(ALL_FILES);
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);

    // Not fetched yet — All mode with empty search renders the tree instead.
    expect(filesSpy).not.toHaveBeenCalled();

    fireEvent.change(screen.getByPlaceholderText('Filter files…'), { target: { value: 'deep' } });

    await waitFor(() => expect(filesSpy).toHaveBeenCalledTimes(1));
    expect(filesSpy).toHaveBeenCalledWith('ws1');
    await waitFor(() =>
      expect(screen.getByTestId('nav-row-src/deep/nested/module.rs')).toBeInTheDocument(),
    );
    expect(screen.queryByTestId('nav-row-src/billing.rs')).not.toBeInTheDocument();
    expect(screen.queryByTestId('nav-row-README.md')).not.toBeInTheDocument();

    // Typing again (still non-empty) must not refetch — cached in state.
    fireEvent.change(screen.getByPlaceholderText('Filter files…'), { target: { value: 'rs' } });
    await waitFor(() => expect(screen.getByTestId('nav-row-src/util.rs')).toBeInTheDocument());
    expect(screen.queryByTestId('nav-row-README.md')).not.toBeInTheDocument();
    expect(filesSpy).toHaveBeenCalledTimes(1);
  });

  it('shows the no-matches message when a project-wide search has no hits', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    vi.spyOn(api, 'getProjectFiles').mockResolvedValue(ALL_FILES);
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText('Filter files…'), {
      target: { value: 'zzz-nope' },
    });
    await waitFor(() =>
      expect(screen.getByText("No files match 'zzz-nope'.")).toBeInTheDocument(),
    );
  });

  it('recovers after toggling away and back mid-flight instead of getting stuck on the spinner', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    let resolveFiles: (v: FileListResult) => void = () => {};
    const pending = new Promise<FileListResult>((resolve) => {
      resolveFiles = resolve;
    });
    // Every call returns the SAME (still-pending) promise, so "resolve the
    // original fetch" below resolves whichever effect instance(s) are still
    // listening — reproducing the real trigger without needing to assert an
    // exact call count.
    const filesSpy = vi.spyOn(api, 'getProjectFiles').mockReturnValue(pending);

    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);

    // All mode + a search starts the lazy fetch, which is left unresolved.
    fireEvent.change(screen.getByPlaceholderText('Filter files…'), { target: { value: 'deep' } });
    await waitFor(() => expect(filesSpy).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText('Searching…')).toBeInTheDocument());

    // Ordinary interaction, not an edge case: flip to Findings mode while
    // the fetch is still in flight, then back to All — search text
    // unchanged throughout.
    fireEvent.click(screen.getByRole('button', { name: 'Findings' }));
    fireEvent.click(screen.getByRole('button', { name: 'All' }));

    // Only now does the original (in-flight) request resolve.
    resolveFiles!(ALL_FILES);

    // Must converge to the loaded list, not get stuck on "Searching…".
    await waitFor(() =>
      expect(screen.getByTestId('nav-row-src/deep/nested/module.rs')).toBeInTheDocument(),
    );
    expect(screen.queryByText('Searching…')).not.toBeInTheDocument();
  });

  it('selecting a row calls onSelect with the file path', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    const onSelect = vi.fn();
    render(<FileNavigator wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={onSelect} />);
    fireEvent.click(screen.getByRole('button', { name: 'Findings' }));
    fireEvent.click(screen.getByTestId('nav-row-src/billing.rs'));
    expect(onSelect).toHaveBeenCalledWith('src/billing.rs');
  });
});
