// @vitest-environment jsdom
// EditableStepNode — the editor card mirrors the Runs cards (LR handles, per-kind
// body). @xyflow/react is mocked because jsdom lacks the layout/ResizeObserver
// APIs the real handles need; the mock stubs Handle/Position so the card DOM
// mounts and we can assert the per-kind body content.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import type { Node, NodeProps } from '@xyflow/react';

vi.mock('@xyflow/react', () => ({
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
}));

import EditableStepNode, { type NodeData } from './EditableStepNode';
import type { GraphNode, StepNodeData } from '../../../lib/workflowGraph';
import type { WorkflowEditorUi } from '../../../hooks/useWorkflowEditorUi';

afterEach(cleanup);

function renderNode(
  data: StepNodeData,
  problems: string[] = [],
  selected = false,
  workflowEditorUi?: WorkflowEditorUi,
) {
  const node: GraphNode = { id: data.id, data, position: { x: 0, y: 0 } };
  const props = { data: { node, problems, workflowEditorUi }, selected } as unknown as NodeProps<
    Node<NodeData, 'editable'>
  >;
  return render(<EditableStepNode {...props} />);
}

describe('EditableStepNode', () => {
  it('a step shows its id, kind chip, and agent', () => {
    renderNode({ id: 'build', kind: 'step', agent: 'coder' });
    expect(screen.getByText('build')).toBeInTheDocument();
    expect(screen.getByText('step')).toBeInTheDocument();
    expect(screen.getByText('coder')).toBeInTheDocument();
  });

  it('a step with no agent reads "(no agent)"', () => {
    renderNode({ id: 'x', kind: 'step' });
    expect(screen.getByText('(no agent)')).toBeInTheDocument();
  });

  it('a parallel node renders a stacked row per sub-step', () => {
    renderNode({
      id: 'fan',
      kind: 'parallel',
      parallel: [
        { id: 'lint', agent: 'a', prompt: 'p' },
        { id: 'test', agent: 'b', prompt: 'q' },
      ],
    });
    expect(screen.getByText('parallel')).toBeInTheDocument();
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('test')).toBeInTheDocument();
  });

  it('a parallel node with no sub-steps shows the empty placeholder', () => {
    renderNode({ id: 'fan', kind: 'parallel', parallel: [] });
    expect(screen.getByText('no sub-steps')).toBeInTheDocument();
  });

  it('a panel node shows a gate block when a gate is set', () => {
    renderNode({
      id: 'rev',
      kind: 'panel',
      panel: {
        panelists: ['a', 'b'],
        subject: 's',
        gate: { until_no_findings_at_severity_or_above: 'high' },
      },
    });
    expect(screen.getByText('panel')).toBeInTheDocument();
    expect(screen.getByText('· 2 panelists')).toBeInTheDocument();
    expect(screen.getByText(/gate ≥ high/)).toBeInTheDocument();
  });

  it('renders the problem dot when problems are present', () => {
    renderNode({ id: 'x', kind: 'step' }, ['needs an agent']);
    expect(screen.getByLabelText('has problems')).toBeInTheDocument();
  });

  it('a for_each node shows the for_each expression', () => {
    renderNode({ id: 'each', kind: 'for_each', agent: 'a', for_each: 'inputs.files' });
    expect(screen.getByText(/for_each: inputs.files/)).toBeInTheDocument();
  });

  describe('legacy inline-approval badge (Task 6)', () => {
    it('a step with approval.required shows the dashed gate badge', () => {
      renderNode({ id: 'ship', kind: 'step', agent: 'x', approvalRequired: true });
      expect(screen.getByLabelText('has an approval gate')).toBeInTheDocument();
    });

    it('a plain step (no inline approval) shows no badge', () => {
      renderNode({ id: 'build', kind: 'step', agent: 'coder' });
      expect(screen.queryByLabelText('has an approval gate')).not.toBeInTheDocument();
    });

    it('a standalone approval_gate node shows no badge (it IS the gate, not a legacy marker)', () => {
      renderNode({ id: 'gate', kind: 'approval_gate', approvalRequired: true });
      expect(screen.queryByLabelText('has an approval gate')).not.toBeInTheDocument();
    });

    it('next look: the badge renders as .wfx-approval-badge', () => {
      const { container } = renderNode(
        { id: 'ship', kind: 'step', agent: 'x', approvalRequired: true },
        [],
        false,
        'next',
      );
      expect(container.querySelector('.wfx-approval-badge')).toBeInTheDocument();
      expect(screen.getByLabelText('has an approval gate')).toBeInTheDocument();
    });
  });

  it('carries data-ui="next" on the outer node when workflowEditorUi is "next"', () => {
    const { container } = renderNode({ id: 'x', kind: 'step' }, [], false, 'next');
    expect(container.querySelector('[data-ui="next"]')).toBeInTheDocument();
  });

  it('defaults to data-ui="classic" when workflowEditorUi is unset', () => {
    const { container } = renderNode({ id: 'x', kind: 'step' });
    expect(container.querySelector('[data-ui="classic"]')).toBeInTheDocument();
  });

  describe('classic look (workflowEditorUi unset)', () => {
    it('renders the current id span and kind chip, and no .wfx-* markers', () => {
      const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' });
      expect(container.querySelector('.text-ui.font-semibold')).toHaveTextContent('build');
      expect(screen.getByText('step')).toBeInTheDocument();
      expect(container.querySelector('.wfx-node')).not.toBeInTheDocument();
      expect(container.querySelector('.wfx-kindpill')).not.toBeInTheDocument();
      expect(container.querySelector('.wfx-nid')).not.toBeInTheDocument();
    });

    it('renders no kind icon (Task 3: icons are next-only)', () => {
      const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' });
      expect(container.querySelector('.wfx-kindicon')).not.toBeInTheDocument();
      expect(container.querySelector('svg')).not.toBeInTheDocument();
    });
  });

  describe('next (instrument) look', () => {
    it('renders a .wfx-node with a .wfx-kindpill (uppercase kind) and a mono .wfx-nid', () => {
      const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
      const wfxNode = container.querySelector('.wfx-node');
      expect(wfxNode).toBeInTheDocument();
      expect(wfxNode).toHaveAttribute('data-ui', 'next');

      const pill = container.querySelector('.wfx-kindpill');
      expect(pill).toBeInTheDocument();
      expect(pill).toHaveTextContent('step');
      // CSS handles the visual uppercase transform — assert it's declared.
      expect(pill).toHaveClass('wfx-kindpill');

      const nid = container.querySelector('.wfx-nid');
      expect(nid).toBeInTheDocument();
      expect(nid).toHaveTextContent('build');

      // no classic markers leak into the next look.
      expect(container.querySelector('.text-ui.font-semibold')).not.toBeInTheDocument();
    });

    it('an approval_gate node\'s kind pill reads "gate", not "approval_gate" — the full string would eat most of the trapezoid\'s safe width', () => {
      const { container } = renderNode({ id: 'approve-deploy', kind: 'approval_gate' }, [], false, 'next');
      const pill = container.querySelector('.wfx-kindpill');
      expect(pill).toHaveTextContent('gate');
      expect(pill?.textContent).not.toContain('approval_gate');
      // the node id itself is untouched by the label change.
      expect(container.querySelector('.wfx-nid')).toHaveTextContent('approve-deploy');
    });

    it('every other kind keeps its label verbatim in the kind pill', () => {
      const kinds: Array<[StepNodeData, string]> = [
        [{ id: 'x', kind: 'step', agent: 'a' }, 'step'],
        [{ id: 'x', kind: 'for_each', agent: 'a', for_each: 'items' }, 'for_each'],
        [{ id: 'x', kind: 'parallel', parallel: [] }, 'parallel'],
        [{ id: 'x', kind: 'panel', panel: { panelists: [], subject: '' } }, 'panel'],
        [{ id: 'x', kind: 'branch', condition: 'c' }, 'branch'],
        [{ id: 'x', kind: 'action', action: 'scm.prs.create' }, 'action'],
      ];
      for (const [data, label] of kinds) {
        const { container } = renderNode(data, [], false, 'next');
        expect(container.querySelector('.wfx-kindpill')).toHaveTextContent(label);
        cleanup();
      }
    });

    it('a step shows the agent as a mono expr line', () => {
      renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
      expect(screen.getByText(/coder/)).toBeInTheDocument();
    });

    it('a for_each node shows a .wfx-expr chip with the for_each expression', () => {
      const { container } = renderNode(
        { id: 'each', kind: 'for_each', agent: 'a', for_each: 'inputs.files' },
        [],
        false,
        'next',
      );
      expect(container.querySelector('.wfx-expr')).toHaveTextContent('for_each: inputs.files');
    });

    it('a parallel node renders a .wfx-subrow per sub-step', () => {
      const { container } = renderNode(
        {
          id: 'fan',
          kind: 'parallel',
          parallel: [
            { id: 'lint', agent: 'a', prompt: 'p' },
            { id: 'test', agent: 'b', prompt: 'q' },
          ],
        },
        [],
        false,
        'next',
      );
      expect(container.querySelectorAll('.wfx-subrow')).toHaveLength(2);
      expect(screen.getByText('lint')).toBeInTheDocument();
      expect(screen.getByText('test')).toBeInTheDocument();
    });

    it('a panel node renders .wfx-port pills per panelist and a .wfx-gate chip', () => {
      const { container } = renderNode(
        {
          id: 'rev',
          kind: 'panel',
          panel: {
            panelists: ['a', 'b'],
            subject: 's',
            gate: { until_no_findings_at_severity_or_above: 'high' },
          },
        },
        [],
        false,
        'next',
      );
      expect(container.querySelectorAll('.wfx-port')).toHaveLength(2);
      expect(container.querySelector('.wfx-gate')).toHaveTextContent(/gate ≥ high/);
    });

    it('a branch node renders true/false .wfx-port pills', () => {
      const { container } = renderNode(
        { id: 'route', kind: 'branch', condition: 'x == 1', thenTargets: ['a'], elseTargets: ['b'] },
        [],
        false,
        'next',
      );
      expect(container.querySelector('.wfx-port-true')).toHaveTextContent('true');
      expect(container.querySelector('.wfx-port-false')).toHaveTextContent('false');
      expect(container.querySelector('.wfx-expr')).toHaveTextContent('if x == 1');
    });

    it('renders the problem dot as .wfx-problem when problems are present', () => {
      const { container } = renderNode({ id: 'x', kind: 'step' }, ['needs an agent'], false, 'next');
      expect(container.querySelector('.wfx-problem')).toBeInTheDocument();
      expect(screen.getByLabelText('has problems')).toBeInTheDocument();
    });

    it.each(['step', 'for_each', 'parallel', 'panel', 'branch'] as const)(
      'renders a .wfx-kindicon svg inside the .wfx-kindpill for kind=%s (Task 3)',
      (kind) => {
        const data =
          kind === 'parallel'
            ? { id: 'x', kind, parallel: [] }
            : kind === 'panel'
              ? { id: 'x', kind, panel: { panelists: [], subject: '' } }
              : kind === 'branch'
                ? { id: 'x', kind, condition: '', thenTargets: [], elseTargets: [] }
                : { id: 'x', kind, agent: 'a' };
        const { container } = renderNode(data as StepNodeData, [], false, 'next');
        const pill = container.querySelector('.wfx-kindpill');
        const icon = pill?.querySelector('.wfx-kindicon');
        expect(icon).toBeInTheDocument();
        expect(icon?.tagName.toLowerCase()).toBe('svg');
      },
    );

    describe('card chrome (Task 3: SVG silhouette shell, no CSS-drawn card)', () => {
      it('wraps .wfx-head and .wfx-body inside .wfx-safe — the shape-derived content box', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
        const safe = container.querySelector('.wfx-safe');
        expect(safe).toBeInTheDocument();

        const head = safe?.querySelector('.wfx-head');
        const body = safe?.querySelector('.wfx-body');
        expect(head).toBeInTheDocument();
        expect(body).toBeInTheDocument();

        // exactly head/body live in the safe box — the accent bar is gone (the
        // silhouette is the kind signal now).
        expect(safe?.children).toHaveLength(2);
        expect(safe?.children[0]).toBe(head);
        expect(safe?.children[1]).toBe(body);
        expect(container.querySelector('.wfx-bar')).not.toBeInTheDocument();
      });

      it('paints the silhouette as an SVG layer, a direct child of .wfx-node', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
        const node = container.querySelector('.wfx-node');
        expect(node).toBeInTheDocument();

        const sil = node?.querySelector(':scope > .wfx-sil');
        expect(sil).toBeInTheDocument();
        expect(sil?.tagName.toLowerCase()).toBe('svg');
        expect(sil?.querySelector('path')?.getAttribute('d')).toMatch(/^M /);

        // Handle is mocked to `() => null`, so assert structurally: .wfx-node's
        // element children are the silhouette + the safe box, nothing else.
        expect(node?.querySelector(':scope > .wfx-safe')).toBeInTheDocument();
        expect(node?.querySelector(':scope > .wfx-head')).not.toBeInTheDocument();
        expect(node?.querySelector(':scope > .wfx-body')).not.toBeInTheDocument();
      });

      it('positions the safe box at the shape safe rect, centring a branch only', () => {
        const { container: step } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], false, 'next');
        const stepSafe = step.querySelector('.wfx-safe') as HTMLElement;
        expect(stepSafe.className).not.toContain('wfx-safe-mid');
        expect(stepSafe.style.left).toBe('15px');

        const { container: br } = renderNode(
          { id: 'route', kind: 'branch', condition: 'x == 1' },
          [],
          false,
          'next',
        );
        const brSafe = br.querySelector('.wfx-safe') as HTMLElement;
        expect(brSafe.className).toContain('wfx-safe-mid');
        // 280 * 0.25 = 70
        expect(brSafe.style.left).toBe('70px');
      });

      it('strokes the silhouette with the kind accent when selected', () => {
        const { container: idle } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], false, 'next');
        const { container: sel } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], true, 'next');
        // `.wfx-sil-shape` is the silhouette's own filled+stroked path — stable
        // regardless of paint order or how many `extra` rails/layers a kind
        // paints on top of it (unlike `path:last-of-type`, which resolves to
        // an `extra` rail for parallel/panel since fix round 1 moved `extra`
        // to paint last).
        const strokeOf = (c: HTMLElement) => c.querySelector('.wfx-sil-shape')?.getAttribute('stroke');
        expect(strokeOf(idle as HTMLElement)).not.toBe(strokeOf(sel as HTMLElement));
      });

      it('a branch node still renders two labeled ports outside the safe box (unaffected by the silhouette)', () => {
        const { container } = renderNode(
          { id: 'route', kind: 'branch', condition: 'x == 1', thenTargets: ['a'], elseTargets: ['b'] },
          [],
          false,
          'next',
        );
        // Handle is mocked away, but the surrounding structure (safe box present,
        // node still renders) must be unaffected by the branch's extra handle.
        expect(container.querySelector('.wfx-safe')).toBeInTheDocument();
        expect(container.querySelector('.wfx-port-true')).toBeInTheDocument();
        expect(container.querySelector('.wfx-port-false')).toBeInTheDocument();
      });

      it('an unselected next card has no inline boxShadow, borderColor, or .wfx-sel class — the shell paints nothing', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
        const node = container.querySelector('.wfx-node') as HTMLElement;
        expect(node.classList.contains('wfx-sel')).toBe(false);
        expect(node.style.boxShadow).toBe('');
        expect(node.style.borderColor).toBe('');
      });

      it('a selected next card still has no inline boxShadow/borderColor on .wfx-node — the glow lives on the SVG silhouette', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], true, 'next');
        const node = container.querySelector('.wfx-node') as HTMLElement;
        // the accent border/glow moved into the SVG silhouette (see "strokes
        // the silhouette..." above) — the shell div itself paints nothing.
        expect(node.classList.contains('wfx-sel')).toBe(false);
        expect(node.style.boxShadow).toBe('');
        expect(node.style.borderColor).toBe('');
      });
    });
  });
});
