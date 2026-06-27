// @vitest-environment jsdom
// NodePalette — the graphical drag-source dock. No @xyflow/react dependency, so
// no mock is needed: we assert the four draggable preview cards render and that
// clicking one fires onAdd(kind); disabled drops draggability + click.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';

import NodePalette from './NodePalette';

afterEach(cleanup);

describe('NodePalette', () => {
  it('renders four draggable preview cards, one per kind', () => {
    render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} />);
    for (const label of ['step', 'for_each', 'parallel', 'panel']) {
      const card = screen.getByRole('button', { name: `Add ${label} node` });
      expect(card).toBeInTheDocument();
      expect(card).toHaveAttribute('draggable', 'true');
    }
  });

  it('clicking a card calls onAdd with that kind', () => {
    const onAdd = vi.fn();
    render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Add parallel node' }));
    expect(onAdd).toHaveBeenCalledWith('parallel');
  });

  it('disabled cards are not draggable and do not fire onAdd', () => {
    const onAdd = vi.fn();
    render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} disabled />);
    const card = screen.getByRole('button', { name: 'Add step node' });
    expect(card).toBeDisabled();
    expect(card).toHaveAttribute('draggable', 'false');
    fireEvent.click(card);
    expect(onAdd).not.toHaveBeenCalled();
  });
});
