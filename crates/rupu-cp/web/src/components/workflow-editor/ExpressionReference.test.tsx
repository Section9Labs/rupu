// @vitest-environment jsdom
// ExpressionReference — searchable, grouped expression reference. Renders the
// real `expressionReference()` vocabulary; clicking an entry inserts (when
// `onInsert` is given) or copies to the clipboard otherwise.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import ExpressionReference from './ExpressionReference';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ExpressionReference', () => {
  it('renders the grouped vocabulary headers', () => {
    render(<ExpressionReference />);
    for (const group of ['Inputs', 'Steps', 'Loop (for_each)', 'Event', 'Issue', 'Functions', 'Filters']) {
      expect(screen.getByRole('heading', { name: group })).toBeInTheDocument();
    }
  });

  it('filters entries by the search query (case-insensitive)', () => {
    render(<ExpressionReference />);
    expect(screen.getByText('event.action')).toBeInTheDocument();
    expect(screen.getByText('inputs.subject')).toBeInTheDocument();

    fireEvent.change(screen.getByRole('searchbox', { name: 'Search expressions' }), {
      target: { value: 'EVENT' },
    });

    expect(screen.getByText('event.action')).toBeInTheDocument();
    expect(screen.queryByText('inputs.subject')).not.toBeInTheDocument();
    // Group with no matches is dropped.
    expect(screen.queryByRole('heading', { name: 'Filters' })).not.toBeInTheDocument();
  });

  it('shows an empty state when nothing matches', () => {
    render(<ExpressionReference />);
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search expressions' }), {
      target: { value: 'zzznope' },
    });
    expect(screen.getByText(/No expressions match/)).toBeInTheDocument();
  });

  it('calls onInsert with the entry insert text when provided', () => {
    const onInsert = vi.fn();
    render(<ExpressionReference onInsert={onInsert} />);
    fireEvent.click(screen.getByText('event.action'));
    expect(onInsert).toHaveBeenCalledWith('event.action');
  });

  it('copies the insert text to the clipboard when no onInsert', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<ExpressionReference />);
    fireEvent.click(screen.getByText('event.action'));
    expect(writeText).toHaveBeenCalledWith('event.action');
    // Transient "copied" indicator appears once the promise resolves.
    expect(await screen.findByText('copied')).toBeInTheDocument();
  });
});
