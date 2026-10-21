// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
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
    // The dead arc is a real dash (not solid, not the draw-in's '1 1').
    const dash = dead.container.querySelector('.afx-msg-dashed')!.getAttribute('stroke-dasharray');
    expect(dash).toBe('3 5');
    expect(dead.container.querySelector('.afx-msg-base')).toBeNull();
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

  it('click_and_keyboard_select_the_arc_and_focus_shows_the_body', () => {
    const onSelect = vi.fn();
    mount('msg_005', 'x', { data: { message: find('msg_005'), highlighted: false, arriving: false, onSelect } });
    const hit = screen.getByTestId('msg-arc-msg_005');
    fireEvent.focus(hit);
    expect(screen.getByText(/reset tokens go through it/)).toBeInTheDocument();
    fireEvent.blur(hit);
    expect(screen.queryByText(/reset tokens go through it/)).toBeNull();
    fireEvent.click(hit);
    fireEvent.keyDown(hit, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledTimes(2);
    expect(onSelect).toHaveBeenCalledWith('msg_005');
  });

  it('an_arc_between_two_unit_squares_arches_from_one_to_the_other', () => {
    mount('msg_009', 'card', {
      source: 'card',
      sourceX: 40,
      sourceY: 100,
      targetX: 120,
      targetY: 100,
      data: { message: find('msg_009'), highlighted: false, arriving: false, fromUnit: true, toUnit: true },
    });
    const d = screen.getByTestId('msg-arc-msg_009').getAttribute('d')!;
    // From the first square's centre to the second's, rising over the grid.
    expect(d.startsWith('M 40 100 C 40 ')).toBe(true);
    expect(d.endsWith(', 120 100')).toBe(true);
    const top = parseFloat(d.split(' ')[5]);
    expect(top).toBeLessThan(100 - 16);
  });

  it('an_arc_from_a_unit_leaves_its_square_at_its_row', () => {
    mount('msg_009', 'other', {
      source: 'card',
      sourceX: 60,
      sourceY: 130,
      targetX: 400,
      targetY: 60,
      data: {
        message: find('msg_009'),
        highlighted: false,
        arriving: false,
        fromUnit: true,
        from: { x: 0, y: 80, width: 232, height: 100 },
        to: { x: 400, y: 40, width: 220, height: 40 },
      },
    });
    const d = screen.getByTestId('msg-arc-msg_009').getAttribute('d')!;
    // It starts in the square and runs out along the square's row.
    expect(d.startsWith('M 60 130 L ')).toBe(true);
    expect(d).toContain(' 130 ');
  });
});
