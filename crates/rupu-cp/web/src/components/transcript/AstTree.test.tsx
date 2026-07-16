// @vitest-environment jsdom
// AstTree — lazily fetches `api.readAst` on mount and renders the CST subtree
// as a recursive, collapsible tree: named-only by default, matched node
// highlighted with its ancestor chain auto-expanded.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import AstTree from './AstTree';
import { api } from '../../lib/api';
import type { AstResponse } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// Tree shape:
//   source_file (root, unmatched)
//     +-- block "A" (named, unmatched) [collapsed by default — has 1 named child]
//     |     +-- identifier "A1" (named, unmatched)
//     +-- function_item "B" (named, MATCHED) [leaf]
//     +-- "punct" (anonymous, unmatched) [hidden by default]
const TREE: AstResponse = {
  available: true,
  language: 'rust',
  truncated: false,
  root: {
    kind: 'source_file',
    named: true,
    startLine: 1,
    startCol: 1,
    endLine: 20,
    endCol: 1,
    matched: false,
    children: [
      {
        kind: 'block',
        named: true,
        startLine: 2,
        startCol: 1,
        endLine: 5,
        endCol: 1,
        matched: false,
        children: [
          {
            kind: 'identifier',
            named: true,
            field: 'name',
            startLine: 3,
            startCol: 3,
            endLine: 3,
            endCol: 10,
            matched: false,
            children: [],
          },
        ],
      },
      {
        kind: 'function_item',
        named: true,
        startLine: 7,
        startCol: 1,
        endLine: 7,
        endCol: 20,
        matched: true,
        children: [],
      },
      {
        kind: '{',
        named: false,
        startLine: 20,
        startCol: 1,
        endLine: 20,
        endCol: 2,
        matched: false,
        children: [],
      },
    ],
  },
};

describe('AstTree', () => {
  it('shows a loading state before the fetch resolves', () => {
    vi.spyOn(api, 'readAst').mockReturnValue(new Promise(() => {}));
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);
    expect(screen.getByText('Loading AST…')).toBeInTheDocument();
  });

  it('fetches exactly once on mount with the given path/line/col', () => {
    const spy = vi.spyOn(api, 'readAst').mockReturnValue(new Promise(() => {}));
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith('r1', 'src/foo.rs', 7, 1, { host: undefined });
  });

  it('passes the host option through to api.readAst', () => {
    const spy = vi.spyOn(api, 'readAst').mockReturnValue(new Promise(() => {}));
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} host="worker-1" />);
    expect(spy).toHaveBeenCalledWith('r1', 'src/foo.rs', 7, 1, { host: 'worker-1' });
  });

  it('renders an error message when the fetch rejects', async () => {
    vi.spyOn(api, 'readAst').mockRejectedValue(new Error('network down'));
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);
    await waitFor(() =>
      expect(screen.getByText(/Could not load AST: .*network down/)).toBeInTheDocument(),
    );
  });

  it('shows the reason text when the tree is unavailable', async () => {
    vi.spyOn(api, 'readAst').mockResolvedValue({ available: false, reason: 'no syntax grammar' });
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);
    await waitFor(() => expect(screen.getByText('no syntax grammar')).toBeInTheDocument());
  });

  it('renders the tree named-only by default, with the matched node visible and highlighted', async () => {
    vi.spyOn(api, 'readAst').mockResolvedValue(TREE);
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);

    await waitFor(() => expect(screen.getByText('source_file')).toBeInTheDocument());

    // Root's named children visible (root is an ancestor of the matched node,
    // so it's auto-expanded).
    expect(screen.getByText('block')).toBeInTheDocument();
    expect(screen.getByText('function_item')).toBeInTheDocument();

    // Anonymous sibling hidden by default.
    expect(screen.queryByText('{')).not.toBeInTheDocument();

    // `block`'s own child is not shown yet — `block` is not on the matched
    // node's ancestor chain, so it starts collapsed.
    expect(screen.queryByText('identifier')).not.toBeInTheDocument();

    // The matched node is highlighted.
    const matchedRow = screen.getByText('function_item').closest('[data-matched]');
    expect(matchedRow).toHaveAttribute('data-matched', 'true');
    expect(matchedRow?.className).toMatch(/bg-warn-bg/);
  });

  it('expanding a collapsed node reveals its children', async () => {
    vi.spyOn(api, 'readAst').mockResolvedValue(TREE);
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);

    await waitFor(() => expect(screen.getByText('block')).toBeInTheDocument());
    expect(screen.queryByText('identifier')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('block'));

    expect(screen.getByText('identifier')).toBeInTheDocument();
    // Field prefix rendered for the `name` field.
    expect(screen.getByText(/name/)).toBeInTheDocument();
  });

  it('toggling "show anonymous" reveals unnamed nodes', async () => {
    vi.spyOn(api, 'readAst').mockResolvedValue(TREE);
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);

    await waitFor(() => expect(screen.getByText('source_file')).toBeInTheDocument());
    expect(screen.queryByText('{')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('checkbox', { name: /show anonymous/i }));

    expect(screen.getByText('{')).toBeInTheDocument();
  });

  it('shows a truncated note when the response is truncated', async () => {
    vi.spyOn(api, 'readAst').mockResolvedValue({ ...TREE, truncated: true });
    render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);

    await waitFor(() => expect(screen.getByText('source_file')).toBeInTheDocument());
    expect(screen.getByText(/tree truncated \(large file\)/i)).toBeInTheDocument();
  });

  it('re-fetches when path/line/col/runId change', async () => {
    const spy = vi.spyOn(api, 'readAst').mockResolvedValue(TREE);
    const { rerender } = render(<AstTree runId="r1" path="src/foo.rs" line={7} col={1} />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));

    rerender(<AstTree runId="r1" path="src/bar.rs" line={9} col={2} />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy).toHaveBeenLastCalledWith('r1', 'src/bar.rs', 9, 2, { host: undefined });
  });
});
