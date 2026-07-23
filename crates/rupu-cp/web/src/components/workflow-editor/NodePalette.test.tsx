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
    it('renders the .wfx-palette dock with a .wfx-pcard per item and a .wfx-picon accent icon', () => {
      const { container } = render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="next" />);
      expect(container.querySelector('.wfx-palette')).toBeInTheDocument();
      // next offers the branch + gate cards too
      // (step/for_each/parallel/panel/branch/gate = 6).
      const cards = container.querySelectorAll('.wfx-pcard');
      expect(cards.length).toBe(6);
      const icons = container.querySelectorAll('.wfx-picon');
      expect(icons.length).toBe(6);
      for (const icon of icons) expect(icon.tagName.toLowerCase()).toBe('svg');
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
      expect(container.querySelectorAll('.wfx-pcard').length).toBe(6);
    });
  });

  describe('gate + connector cards (Task 5, next only)', () => {
    const TOOLS = [
      { name: 'scm.prs.create', description: 'Open a PR', input_schema: {}, kind: 'write' as const },
      { name: 'scm.prs.comment', description: 'Comment on a PR', input_schema: {}, kind: 'write' as const },
      { name: 'issues.comment', description: 'Comment on an issue', input_schema: {}, kind: 'write' as const },
    ];

    it('next offers a static Gate card that adds an approval_gate node', () => {
      const onAdd = vi.fn();
      render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" />);
      const card = screen.getByRole('button', { name: 'Add gate node' });
      expect(card).toBeInTheDocument();
      fireEvent.click(card);
      expect(onAdd).toHaveBeenCalledWith('approval_gate');
    });

    it('classic shows neither the Gate card nor any connector card', () => {
      render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} tools={TOOLS} />);
      expect(screen.queryByRole('button', { name: 'Add gate node' })).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Add scm.prs.create action' })).not.toBeInTheDocument();
    });

    it('next renders one connector card per tool, grouped by prefix', () => {
      const { container } = render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="next" tools={TOOLS} />,
      );
      for (const t of TOOLS) {
        expect(screen.getByRole('button', { name: `Add ${t.name} action` })).toBeInTheDocument();
      }
      // grouped by first dotted segment: scm (2) + issues (1) = 2 groups.
      const labels = [...container.querySelectorAll('.wfx-palette-group-label')].map((n) => n.textContent);
      expect(labels).toEqual(['scm', 'issues']);
    });

    it('clicking a connector card adds an action node seeded with that tool name', () => {
      const onAdd = vi.fn();
      render(
        <NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" tools={TOOLS} />,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Add scm.prs.create action' }));
      expect(onAdd).toHaveBeenCalledWith('action', { action: 'scm.prs.create' });
    });
  });

  describe('variant="rail" (Task 1: inspector-rail dock)', () => {
    it('renders a non-absolute .wfx-palette-rail block with all kind cards', () => {
      const { container } = render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" />,
      );
      const rail = container.querySelector('.wfx-palette-rail');
      expect(rail).toBeInTheDocument();
      expect(rail).not.toHaveClass('absolute');
      for (const label of ['step', 'for_each', 'parallel', 'panel']) {
        expect(screen.getByRole('button', { name: `Add ${label} node` })).toBeInTheDocument();
      }
      // the rail variant never also renders the floating dock.
      expect(container.querySelector('.wfx-palette')).not.toBeInTheDocument();
    });

    it('rail cards are .wfx-pcard with a .wfx-picon accent icon, same as the float next look', () => {
      const { container } = render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" />,
      );
      expect(container.querySelectorAll('.wfx-pcard').length).toBe(4);
      expect(container.querySelectorAll('.wfx-picon').length).toBe(4);
    });

    it('clicking a rail card SELECTS it (shows a detail card) rather than instantly adding', () => {
      const onAdd = vi.fn();
      render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} variant="rail" />);
      fireEvent.click(screen.getByRole('button', { name: 'Add parallel node' }));
      expect(onAdd).not.toHaveBeenCalled();
      expect(screen.getByRole('region', { name: 'parallel details' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Add to canvas' })).toBeInTheDocument();
    });

    it('the detail card shows the block blurb, `*`-marked required fields, and a YAML example', () => {
      render(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" />);
      fireEvent.click(screen.getByRole('button', { name: 'Add step node' }));
      const detail = screen.getByRole('region', { name: 'step details' });
      expect(detail).toHaveTextContent(/runs one agent/i);
      expect(detail.querySelector('code')).toHaveTextContent('agent');
      expect(detail.querySelectorAll('.wfx-detail-req-star').length).toBeGreaterThan(0);
      expect(detail.querySelector('pre')).toHaveTextContent('agent: code-reviewer');
    });

    it('the gate block has NO required fields (approval: is entirely optional per workflow.rs) — no `*` list renders', () => {
      render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" workflowEditorUi="next" />,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Add gate node' }));
      const detail = screen.getByRole('region', { name: 'gate details' });
      expect(detail.querySelector('.wfx-detail-reqs')).not.toBeInTheDocument();
      expect(detail.querySelectorAll('.wfx-detail-req-star').length).toBe(0);
      expect(detail).toHaveTextContent(/optional/i);
    });

    it('the Add-to-canvas button respects `disabled` (paused editor) instead of silently no-op-ing', () => {
      const onAdd = vi.fn();
      // Select a card while enabled, then flip the whole rail `disabled`
      // (mirrors the editor pausing on unparseable YAML while a card is
      // already selected) — the CTA must disable along with everything else.
      const { rerender } = render(
        <NodePalette onAdd={onAdd} onDragStartKind={() => {}} variant="rail" />,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Add parallel node' }));
      rerender(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} variant="rail" disabled />);
      const cta = screen.getByRole('button', { name: 'Add to canvas' });
      expect(cta).toBeDisabled();
      fireEvent.click(cta);
      expect(onAdd).not.toHaveBeenCalled();
    });

    it('"Add to canvas" calls onAdd with the selected block kind and clears the selection', () => {
      const onAdd = vi.fn();
      render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} variant="rail" />);
      fireEvent.click(screen.getByRole('button', { name: 'Add parallel node' }));
      fireEvent.click(screen.getByRole('button', { name: 'Add to canvas' }));
      expect(onAdd).toHaveBeenCalledWith('parallel');
      expect(screen.queryByRole('region', { name: 'parallel details' })).not.toBeInTheDocument();
    });

    it('rail cards stay draggable and disabled-aware', () => {
      const { rerender } = render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" />,
      );
      expect(screen.getByRole('button', { name: 'Add step node' })).toHaveAttribute('draggable', 'true');

      rerender(<NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" disabled />);
      const card = screen.getByRole('button', { name: 'Add step node' });
      expect(card).toBeDisabled();
      expect(card).toHaveAttribute('draggable', 'false');
    });

    it('dragging a rail card still sets the node-kind DnD mime (drag-to-place is unchanged)', () => {
      const onDragStartKind = vi.fn();
      render(<NodePalette onAdd={() => {}} onDragStartKind={onDragStartKind} variant="rail" />);
      const card = screen.getByRole('button', { name: 'Add step node' });
      const dataTransfer = {
        setData: vi.fn(),
        effectAllowed: '',
      };
      fireEvent.dragStart(card, { dataTransfer });
      expect(dataTransfer.setData).toHaveBeenCalledWith('application/rupu-node-kind', 'step');
      expect(onDragStartKind).toHaveBeenCalledWith('step');
    });

    it("with workflowEditorUi='next' the rail variant also offers the branch card, select-then-add", () => {
      const onAdd = vi.fn();
      render(
        <NodePalette onAdd={onAdd} onDragStartKind={() => {}} variant="rail" workflowEditorUi="next" />,
      );
      const card = screen.getByRole('button', { name: 'Add branch node' });
      expect(card).toBeInTheDocument();
      fireEvent.click(card);
      expect(onAdd).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole('button', { name: 'Add to canvas' }));
      expect(onAdd).toHaveBeenCalledWith('branch');
    });

    it('a filter input narrows the visible block chips by label (case-insensitive)', () => {
      render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" workflowEditorUi="next" />,
      );
      const filter = screen.getByRole('searchbox', { name: 'Filter blocks and actions' });
      fireEvent.change(filter, { target: { value: 'BRANCH' } });
      expect(screen.getByRole('button', { name: 'Add branch node' })).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Add step node' })).not.toBeInTheDocument();
    });

    describe('connector detail card (parsed from ToolSpec.input_schema)', () => {
      const TOOL_WITH_SCHEMA = {
        name: 'scm.prs.create',
        description: 'Open a pull request.',
        input_schema: {
          type: 'object',
          properties: {
            title: { type: 'string', description: 'PR title' },
            base: { type: 'string', description: 'Base branch' },
          },
          required: ['title', 'base'],
        },
        kind: 'write' as const,
      };
      const TOOL_WITHOUT_SCHEMA = {
        name: 'issues.comment',
        description: 'Comment on an issue.',
        input_schema: {},
        kind: 'write' as const,
      };

      it('clicking a connector chip SELECTS it and renders required params parsed from input_schema', () => {
        const onAdd = vi.fn();
        render(
          <NodePalette
            onAdd={onAdd}
            onDragStartKind={() => {}}
            variant="rail"
            workflowEditorUi="next"
            tools={[TOOL_WITH_SCHEMA]}
          />,
        );
        fireEvent.click(screen.getByRole('button', { name: 'Add scm.prs.create action' }));
        expect(onAdd).not.toHaveBeenCalled();
        const detail = screen.getByRole('region', { name: 'scm.prs.create details' });
        expect(detail).toHaveTextContent('Open a pull request.');
        expect(detail).toHaveTextContent('title');
        expect(detail).toHaveTextContent('base');
        expect(detail).toHaveTextContent('PR title');

        fireEvent.click(screen.getByRole('button', { name: 'Add to canvas' }));
        expect(onAdd).toHaveBeenCalledWith('action', { action: 'scm.prs.create' });
      });

      it('a connector with no required[] in its schema shows the description + a fallback note instead of a required-fields list', () => {
        render(
          <NodePalette
            onAdd={() => {}}
            onDragStartKind={() => {}}
            variant="rail"
            workflowEditorUi="next"
            tools={[TOOL_WITHOUT_SCHEMA]}
          />,
        );
        fireEvent.click(screen.getByRole('button', { name: 'Add issues.comment action' }));
        const detail = screen.getByRole('region', { name: 'issues.comment details' });
        expect(detail).toHaveTextContent('Comment on an issue.');
        expect(detail.querySelector('.wfx-detail-reqs')).not.toBeInTheDocument();
        expect(detail).toHaveTextContent(/parameters come from the tool schema/i);
      });
    });

    it('default (float) variant is unaffected by the rail addition', () => {
      const { container } = render(
        <NodePalette onAdd={() => {}} onDragStartKind={() => {}} workflowEditorUi="next" />,
      );
      expect(container.querySelector('.wfx-palette')).toBeInTheDocument();
      expect(container.querySelector('.wfx-palette-rail')).not.toBeInTheDocument();
    });

    it('the float ("next" instrument) variant is unchanged: click still instantly adds', () => {
      const onAdd = vi.fn();
      render(<NodePalette onAdd={onAdd} onDragStartKind={() => {}} workflowEditorUi="next" />);
      fireEvent.click(screen.getByRole('button', { name: 'Add parallel node' }));
      expect(onAdd).toHaveBeenCalledWith('parallel');
    });
  });
});
