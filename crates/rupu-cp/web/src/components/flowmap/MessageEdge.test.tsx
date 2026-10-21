// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Position, type EdgeProps } from '@xyflow/react';
import MessageEdge, { type MessageFlowEdge } from './MessageEdge';
import { ThemeProvider } from '../theme/ThemeProvider';
import { buildFlowModel } from '../../lib/flow/flowModel';
import { fixtureInput } from '../../lib/flow/fixtures';

afterEach(cleanup);

const model = buildFlowModel(fixtureInput());
const find = (id: string) => model.messages.find((m) => m.id === id)!;

function mount(id: string, target: string, over: Partial<EdgeProps<MessageFlowEdge>> = {}) {
  const message = find(id);
  const props = {
    id: `e-${id}`,
    source: message.from,
    target,
    sourceX: 10,
    sourceY: 10,
    targetX: 210,
    targetY: 90,
    sourcePosition: Position.Right,
    targetPosition: Position.Left,
    data: { message, highlighted: false, arriving: false },
    ...over,
  } as EdgeProps<MessageFlowEdge>;
  return render(
    <ThemeProvider>
      <svg>
        <MessageEdge {...props} />
      </svg>
    </ThemeProvider>,
  );
}

describe('MessageEdge', () => {
  it('arc_shows_body_on_hover_and_dead_end_for_undelivered', () => {
    const { container, unmount } = mount('msg_005', 'x');
    const hit = screen.getByTestId('msg-arc-msg_005');
    expect(screen.queryByText(/reset tokens go through it/)).toBeNull();
    fireEvent.mouseEnter(hit);
    expect(screen.getByText(/reset tokens go through it/)).toBeInTheDocument();
    fireEvent.mouseLeave(hit);
    expect(screen.queryByText(/reset tokens go through it/)).toBeNull();
    expect(container.querySelector('.afx-msg-dead')).toBeNull();
    unmount();

    // A message to a participant that was gone: the arc ends in a dead end
    // that says why.
    const undelivered = model.messages.find((m) => m.undelivered.length > 0)!;
    expect(undelivered).toBeDefined();
    const u = undelivered.undelivered[0];
    const dead = mount(undelivered.id, u.to);
    expect(dead.container.querySelector('.afx-msg-dead')).not.toBeNull();
    fireEvent.mouseEnter(screen.getByTestId(`msg-arc-${undelivered.id}`));
    expect(screen.getByTestId('msg-reason')).toHaveTextContent(/\S/);
  });

  it('a_self_arc_is_a_loop_and_arriving_runs_a_packet', () => {
    const { container } = mount('msg_005', 'x', {
      source: 'a',
      target: 'a',
      sourceX: 50,
      sourceY: 50,
      targetX: 50,
      targetY: 50,
      data: { message: find('msg_005'), highlighted: true, arriving: true },
    });
    const d = container.querySelector('.afx-msg-base')!.getAttribute('d')!;
    expect(d).toMatch(/^M ?50[ ,]50 C/);
    expect(container.querySelector('.afx-msg-packet')).not.toBeNull();
    expect(container.querySelector('.afx-msg')).toHaveAttribute('data-highlighted', 'true');
  });
});
