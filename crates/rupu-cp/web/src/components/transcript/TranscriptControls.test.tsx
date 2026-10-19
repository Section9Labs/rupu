// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { it, expect, describe, afterEach, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import TranscriptControls from './TranscriptControls';
import TranscriptOutline from './TranscriptOutline';
import type { TurnView } from './transcriptView';

afterEach(cleanup);

const counts = { said: 4, thinking: 2, commands: 7, findings: 1 };

function setup(filter: 'all' | 'commands' = 'all') {
  const h = { onFilter: vi.fn(), onExpandAll: vi.fn(), onCollapseAll: vi.fn() };
  render(<TranscriptControls counts={counts} filter={filter} {...h} />);
  return h;
}

describe('TranscriptControls', () => {
  it('renders five chips with counts and marks the active one', () => {
    setup('commands');
    expect(screen.getAllByRole('button', { pressed: undefined }).length).toBeGreaterThanOrEqual(5);
    expect(screen.getByRole('button', { name: /^Commands\s*7$/ })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /^All$/ })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: /^Said\s*4$/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Thinking\s*2$/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Findings\s*1$/ })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /filter/i })).toBeInTheDocument();
  });

  it('calls onFilter with the chip kind', () => {
    const h = setup();
    fireEvent.click(screen.getByRole('button', { name: /Commands/ }));
    expect(h.onFilter).toHaveBeenCalledWith('commands');
  });

  it('calls expand / collapse callbacks', () => {
    const h = setup();
    fireEvent.click(screen.getByRole('button', { name: 'Expand all' }));
    fireEvent.click(screen.getByRole('button', { name: 'Collapse all' }));
    expect(h.onExpandAll).toHaveBeenCalledTimes(1);
    expect(h.onCollapseAll).toHaveBeenCalledTimes(1);
  });
});

describe('TranscriptOutline', () => {
  const turn = (blocks: TurnView['blocks']): TurnView => ({
    blocks,
    tokensIn: null,
    tokensOut: null,
    summary: { toolCount: 0, findingCount: 0, result: 'ok' },
  });

  it('emits one tick per turn linking to its anchor', () => {
    const turns = [
      turn([{ kind: 'assistant', content: 'a' }]),
      turn([{ kind: 'assistant', content: 'b' }]),
    ];
    render(<TranscriptOutline turns={turns} filter="all" />);
    const ticks = screen.getAllByTestId('outline-tick');
    expect(ticks).toHaveLength(2);
    expect(ticks[1]).toHaveAttribute('href', '#turn-2');
  });

  it('drops turns the filter empties', () => {
    const turns = [
      turn([{ kind: 'assistant', content: 'a' }]),
      turn([{ kind: 'thinking', text: 't', provider: 'x' }]),
    ];
    render(<TranscriptOutline turns={turns} filter="thinking" />);
    expect(screen.getAllByTestId('outline-tick')).toHaveLength(1);
  });
});
