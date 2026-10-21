// @vitest-environment jsdom
import { act, render } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useExitGhosts, type ExitGhosts } from './exitGhosts';

vi.mock('@xyflow/react', () => ({
  ViewportPortal: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

let api: ExitGhosts | undefined;

function Canvas() {
  api = useExitGhosts();
  return (
    <div className="react-flow">
      <div className="react-flow__node" data-id="a" data-testid="rf__node-a" style={{ transform: 'translate(40px, 60px)' }}>
        <span data-handleid="out">step a</span>
      </div>
      <svg>
        <g className="react-flow__edge" data-id="a->b">
          <defs>
            <linearGradient id="grad-1" />
          </defs>
          <path d="M0 0L10 10" />
        </g>
      </svg>
      {api.layer}
    </div>
  );
}

afterEach(() => {
  vi.useRealTimers();
  api = undefined;
});

describe('useExitGhosts', () => {
  it('copies a removed node and edge into the layer, inert and without ids, then drops them', () => {
    vi.useFakeTimers();
    const { container } = render(<Canvas />);
    const layer = container.querySelector('.wfx-ghosts')!;

    act(() => api!.ghost(['a', 'gone'], ['a->b']));

    const node = layer.querySelector('.wfx-ghost-node')!;
    expect(node.textContent).toBe('step a');
    expect(node.getAttribute('style')).toBe('transform: translate(40px, 60px);');
    expect(node.getAttribute('aria-hidden')).toBe('true');
    expect(node.hasAttribute('inert')).toBe(true);
    expect(layer.querySelectorAll('[data-id], [data-testid], [data-handleid]')).toHaveLength(0);
    expect(layer.querySelector('.wfx-ghost-edges .wfx-ghost-edge linearGradient')?.id).toBe('grad-1');
    expect(layer.children).toHaveLength(2);

    act(() => vi.advanceTimersByTime(250));
    expect(layer.children).toHaveLength(0);
  });

  it('copies an element once while it is already fading', () => {
    vi.useFakeTimers();
    const { container } = render(<Canvas />);
    const layer = container.querySelector('.wfx-ghosts')!;

    act(() => {
      api!.ghost(['a'], ['a->b']);
      api!.ghost([], ['a->b']);
    });

    expect(layer.querySelectorAll('.wfx-ghost-edge')).toHaveLength(1);
    expect(layer.querySelectorAll('.wfx-ghost-node')).toHaveLength(1);
  });
});
