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

  it('with no workflowEditorUi prop (default classic) the branch card is absent', () => {
    render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} />);
    expect(screen.queryByRole('button', { name: 'Add branch node' })).not.toBeInTheDocument();
  });

  it("with workflowEditorUi='classic' the branch card is absent", () => {
    render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="classic" />);
    expect(screen.queryByRole('button', { name: 'Add branch node' })).not.toBeInTheDocument();
  });

  it("with workflowEditorUi='next' the branch card renders and adds a branch node", () => {
    const onAdd = vi.fn();
    render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" />);
    const card = screen.getByRole('button', { name: 'Add branch node' });
    expect(card).toBeInTheDocument();
    fireEvent.click(card);
    expect(onAdd).toHaveBeenCalledWith('branch');
  });

  describe('classic (unchanged) dock markup', () => {
    it('renders the current dock/card classes and no .wfx-* markers', () => {
      const { container } = render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="classic" />);
      expect(container.querySelector('.rounded-lg.border.border-border.bg-panel\\/95')).toBeInTheDocument();
      expect(container.querySelector('.wfx-palette')).not.toBeInTheDocument();
      expect(container.querySelector('.wfx-pcard')).not.toBeInTheDocument();
    });
  });

  describe('next (instrument) look', () => {
    it('renders the .wfx-palette dock with a .wfx-pcard per item and a .wfx-pdot accent', () => {
      const { container } = render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="next" />);
      expect(container.querySelector('.wfx-palette')).toBeInTheDocument();
      // next also offers the branch card (step/for_each/parallel/panel/branch = 5).
      const cards = container.querySelectorAll('.wfx-pcard');
      expect(cards.length).toBe(5);
      expect(container.querySelectorAll('.wfx-pdot').length).toBe(5);
      // no classic markers leak into the next look.
      expect(container.querySelector('.rounded-lg.border.border-border.bg-panel\\/95')).not.toBeInTheDocument();
    });

    it('still fires onAdd on click and stays draggable/disabled-aware', () => {
      const onAdd = vi.fn();
      const { rerender } = render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" />);
      const card = screen.getByRole('button', { name: 'Add parallel node' });
      expect(card).toHaveAttribute('draggable', 'true');
      fireEvent.click(card);
      expect(onAdd).toHaveBeenCalledWith('parallel');

      rerender(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" disabled />);
      const disabledCard = screen.getByRole('button', { name: 'Add step node' });
      expect(disabledCard).toBeDisabled();
      expect(disabledCard).toHaveAttribute('draggable', 'false');
    });

    it('the branch card in next mode is also a .wfx-pcard', () => {
      const { container } = render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="next" />);
      const branchCard = screen.getByRole('button', { name: 'Add branch node' });
      expect(branchCard).toHaveClass('wfx-pcard');
      expect(container.querySelectorAll('.wfx-pcard').length).toBe(5);
    });
  });
});
