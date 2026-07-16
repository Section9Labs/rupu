// @vitest-environment jsdom
// SourcePreview — lazily fetches `api.readSource` on mount and renders a
// line-numbered slice, the `unavailable` reason, or a loading/error state.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import SourcePreview from './SourcePreview';
import { api } from '../../lib/api';
import type { SourceSlice } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const SLICE: SourceSlice = {
  available: true,
  path: 'src/foo.rs',
  language: 'rust',
  startLine: 1,
  endLine: 3,
  targetLine: 2,
  totalLines: 100,
  lines: [
    { n: 1, text: 'fn main() {' },
    { n: 2, text: '    println!("hi");' },
    { n: 3, text: '}' },
  ],
};

describe('SourcePreview', () => {
  it('shows a loading state before the fetch resolves', () => {
    vi.spyOn(api, 'readSource').mockReturnValue(new Promise(() => {}));
    render(<SourcePreview runId="r1" path="src/foo.rs" line={2} />);
    expect(screen.getByText('Loading source…')).toBeInTheDocument();
  });

  it('does not fetch before mount and fetches exactly once on mount', () => {
    const spy = vi.spyOn(api, 'readSource').mockReturnValue(new Promise(() => {}));
    expect(spy).not.toHaveBeenCalled();
    render(<SourcePreview runId="r1" path="src/foo.rs" line={2} />);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith('r1', 'src/foo.rs', 2, { host: undefined });
  });

  it('renders a line-numbered slice with the target line emphasized once loaded', async () => {
    vi.spyOn(api, 'readSource').mockResolvedValue(SLICE);
    render(<SourcePreview runId="r1" path="src/foo.rs" line={2} />);

    await waitFor(() => expect(screen.getByText(/println/)).toBeInTheDocument());

    // All three lines present
    expect(screen.getByText('1')).toBeInTheDocument();
    expect(screen.getByText('2')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();

    // The target line's row carries the emphasis marker.
    const targetRow = screen.getByText('2').closest('div');
    expect(targetRow).toHaveAttribute('data-target', 'true');
    expect(targetRow?.className).toMatch(/bg-warn-bg/);

    // A non-target row does not carry the marker.
    const otherRow = screen.getByText('1').closest('div');
    expect(otherRow).not.toHaveAttribute('data-target');
  });

  it('renders the reason text when the slice is unavailable', async () => {
    vi.spyOn(api, 'readSource').mockResolvedValue({
      available: false,
      reason: 'file not found on host',
    });
    render(<SourcePreview runId="r1" path="missing.rs" line={1} />);

    await waitFor(() =>
      expect(screen.getByText('file not found on host')).toBeInTheDocument(),
    );
  });

  it('renders a fallback message when unavailable with no reason', async () => {
    vi.spyOn(api, 'readSource').mockResolvedValue({ available: false });
    render(<SourcePreview runId="r1" path="missing.rs" line={1} />);

    await waitFor(() =>
      expect(screen.getByText('Source not available.')).toBeInTheDocument(),
    );
  });

  it('renders an error message when the fetch rejects', async () => {
    vi.spyOn(api, 'readSource').mockRejectedValue(new Error('network down'));
    render(<SourcePreview runId="r1" path="src/foo.rs" line={2} />);

    await waitFor(() =>
      expect(screen.getByText(/Could not load source: .*network down/)).toBeInTheDocument(),
    );
  });

  it('re-fetches when path/line/runId change', async () => {
    const spy = vi.spyOn(api, 'readSource').mockResolvedValue(SLICE);
    const { rerender } = render(<SourcePreview runId="r1" path="src/foo.rs" line={2} />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));

    rerender(<SourcePreview runId="r1" path="src/bar.rs" line={5} />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy).toHaveBeenLastCalledWith('r1', 'src/bar.rs', 5, { host: undefined });
  });

  it('passes the host option through to api.readSource', () => {
    const spy = vi.spyOn(api, 'readSource').mockReturnValue(new Promise(() => {}));
    render(<SourcePreview runId="r1" path="src/foo.rs" line={2} host="worker-1" />);
    expect(spy).toHaveBeenCalledWith('r1', 'src/foo.rs', 2, { host: 'worker-1' });
  });
});
